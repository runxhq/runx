//! Capability evidence: what an instance's principal can do at the provider.
//!
//! Each instance authority has one source of capability:
//!
//! - a hosted grant: the grant runx Connect lists for the authenticated
//!   principal, or the grant a host injected;
//! - a local credential: the scopes the operator granted when binding it;
//! - a `gh` session: the signed-in account's permission on the target
//!   repository;
//! - public: the connector's reads, and its writes too when it takes no
//!   authority at all, since each of those requires exact approval.
//!
//! Evidence is read at admission, never cached across runs, and expressed as
//! an [`AuthorityTerm`] that admission proves the call against.

use std::collections::BTreeSet;

use runx_contracts::connector::OperationVerb;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    AuthorityBounds, AuthorityResourceFamily, AuthorityTerm, AuthorityVerb, ConnectorInstance,
    InstanceAuthority, Reference, ReferenceType,
};

use super::connectors::LoadedConnector;
use super::instances::{InstanceSource, ResolvedInstance};

/// Capability evidence for one call.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Capability {
    /// The grant the evidence names: a runx Connect grant id, or the local
    /// credential, session or public grant it stands for.
    pub grant_id: String,
    pub scopes: Vec<String>,
    pub term: AuthorityTerm,
}

/// What capability evidence needs from the caller.
pub(crate) struct CapabilityRequest<'a> {
    pub connector: &'a LoadedConnector,
    pub instance: &'a ResolvedInstance,
    pub hosted: &'a super::operation::hosted::HostedClient,
    pub env: &'a std::collections::BTreeMap<String, String>,
    pub cwd: &'a std::path::Path,
}

pub(crate) fn evidence(request: &CapabilityRequest<'_>) -> Result<Capability, String> {
    let instance = &request.instance.instance;
    let connector = &request.connector.manifest;
    match (&instance.authority, &request.instance.source) {
        (
            InstanceAuthority::HostedGrant { grant_id },
            InstanceSource::Injected { granted_scopes },
        ) => Ok(capability(
            instance,
            request.connector,
            grant_id.as_str().to_owned(),
            granted_scopes.clone(),
            Vec::new(),
        )),
        (InstanceAuthority::HostedGrant { grant_id }, _) => {
            let grant = request
                .hosted
                .grant(request.env, request.cwd, grant_id.as_str())
                .map_err(|error| format!("hosted grant {grant_id} is unavailable: {error}"))?;
            if grant.provider != connector.id.as_str() {
                return Err(format!(
                    "hosted grant {grant_id} belongs to {}, not {}",
                    grant.provider, connector.id
                ));
            }
            if grant.status != "active" {
                return Err(format!("hosted grant {grant_id} is {}", grant.status));
            }
            if grant.principal_ref != instance.principal_ref.as_str() {
                return Err(format!(
                    "hosted grant {grant_id} is held by another principal than instance {}",
                    instance.instance_id
                ));
            }
            Ok(capability(
                instance,
                request.connector,
                grant.grant_id,
                grant.scopes,
                grant.target_locator.into_iter().collect(),
            ))
        }
        (
            InstanceAuthority::Credential {
                profile,
                credential,
                scopes,
                material_ref,
            },
            _,
        ) => {
            super::operation::credentials::check_profile(
                request.env,
                request.cwd,
                connector,
                &super::operation::credentials::ProfileBinding {
                    profile,
                    credential,
                    material_ref,
                },
            )?;
            Ok(capability(
                instance,
                request.connector,
                format!("local-credential:{}:{profile}", connector.id),
                scopes
                    .iter()
                    .map(|scope| scope.as_str().to_owned())
                    .collect(),
                Vec::new(),
            ))
        }
        // A local session's account is checked where its authority is read:
        // the command session's token before each call, the browser by the
        // host tool that drives it.
        (InstanceAuthority::Session { .. }, _) => Ok(capability(
            instance,
            request.connector,
            format!("local-session:{}:{}", connector.id, instance.instance_id),
            session_scopes(request.connector, true),
            Vec::new(),
        )),
        (InstanceAuthority::Public, _) => Ok(capability(
            instance,
            request.connector,
            format!("public:{}", connector.id),
            session_scopes(request.connector, connector.auth.unauthenticated()),
            Vec::new(),
        )),
    }
}

/// The scopes of every operation a session or public instance can reach:
/// all of them, or only the reads.
fn session_scopes(connector: &LoadedConnector, writes: bool) -> Vec<String> {
    connector
        .manifest
        .operations
        .values()
        .filter(|operation| writes || operation.verb == OperationVerb::Read)
        .flat_map(|operation| {
            operation
                .scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The capability as a term: every verb the connector declares (reads only
/// for a public instance of a connector that takes authority), its
/// operations, the granted scopes and, when the grant pins one, its target.
fn capability(
    instance: &ConnectorInstance,
    connector: &LoadedConnector,
    grant_id: String,
    scopes: Vec<String>,
    targets: Vec<String>,
) -> Capability {
    let reads_only = matches!(instance.authority, InstanceAuthority::Public)
        && !connector.manifest.auth.unauthenticated();
    let verbs = connector
        .manifest
        .operations
        .values()
        .map(|operation| operation.verb.authority_verb())
        .filter(|verb| !reads_only || *verb == AuthorityVerb::Read)
        .collect::<Vec<_>>();
    let mut unique_verbs = Vec::new();
    for verb in verbs {
        if !unique_verbs.contains(&verb) {
            unique_verbs.push(verb);
        }
    }
    let term = AuthorityTerm {
        term_id: NonEmptyString::from(format!("capability:{grant_id}")),
        principal_ref: principal_reference(instance),
        resource_ref: instance_reference(instance),
        resource_family: AuthorityResourceFamily::Connector,
        verbs: unique_verbs,
        bounds: AuthorityBounds {
            operations: vec![NonEmptyString::from(format!("{}/*", connector.manifest.id))],
            scopes: scopes
                .iter()
                .map(|scope| NonEmptyString::from(scope.as_str()))
                .collect(),
            targets: if targets.is_empty() {
                vec![NonEmptyString::from("*")]
            } else {
                targets
                    .iter()
                    .map(|target| NonEmptyString::from(target.as_str()))
                    .collect()
            },
            ..AuthorityBounds::default()
        },
        conditions: Vec::new(),
        approvals: Vec::new(),
        capabilities: Vec::new(),
        expires_at: None,
        issued_by_ref: Reference::runx(ReferenceType::Grant, &grant_id),
        credential_ref: None,
    };
    Capability {
        grant_id,
        scopes,
        term,
    }
}

pub(crate) fn principal_reference(instance: &ConnectorInstance) -> Reference {
    Reference::with_uri(ReferenceType::Principal, instance.principal_ref.as_str())
}

pub(crate) fn instance_reference(instance: &ConnectorInstance) -> Reference {
    Reference::runx(
        ReferenceType::ConnectorInstance,
        instance.instance_id.as_str(),
    )
}
