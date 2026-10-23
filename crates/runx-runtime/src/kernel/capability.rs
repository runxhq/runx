//! Capability evidence: what an instance's principal can do at the provider.
//!
//! Each instance authority has one source of capability:
//!
//! - a hosted grant: the grant runx Connect lists for the authenticated
//!   principal;
//! - a local credential or session: the scopes the operator granted when
//!   confirming the instance; the provider enforces the account's own
//!   permissions;
//! - public: the connector's reads, and its writes too when it takes no
//!   authority at all, since each of those requires exact approval.
//!
//! Only a hosted grant is a grant: a local instance's evidence is the
//! operator's confirmation of that instance, and is named by it. Evidence is
//! read at admission, never cached across runs, and expressed as an
//! [`AuthorityTerm`] that admission proves the call against.

use std::collections::BTreeSet;

use runx_contracts::connector::{ConnectorOperation, Implementation, OperationVerb};
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    AuthorityBounds, AuthorityResourceFamily, AuthorityTerm, ConnectorInstance, InstanceAuthority,
    Reference, ReferenceType,
};

use super::connectors::LoadedConnector;

/// Capability evidence for one call.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Capability {
    /// The runx Connect grant the evidence is, for a hosted instance.
    pub grant_id: Option<String>,
    /// The evidence as admission names it: the grant, or the local instance
    /// the operator confirmed.
    pub source: String,
    pub scopes: Vec<String>,
    pub term: AuthorityTerm,
}

/// What capability evidence needs from the caller.
pub(crate) struct CapabilityRequest<'a> {
    pub connector: &'a LoadedConnector,
    pub instance: &'a ConnectorInstance,
    pub hosted: &'a super::operation::hosted::HostedClient,
    pub env: &'a std::collections::BTreeMap<String, String>,
    pub cwd: &'a std::path::Path,
    /// Public operations have no account holder; bind their capability to
    /// the declared caller. Held authority keeps its own principal.
    pub actor_ref: Option<&'a Reference>,
}

pub(crate) fn evidence(request: &CapabilityRequest<'_>) -> Result<Capability, String> {
    let instance = request.instance;
    match &instance.authority {
        InstanceAuthority::HostedGrant { grant_id } => {
            let grant = request
                .hosted
                .grant(request.env, request.cwd, grant_id.as_str())
                .map_err(|error| format!("hosted grant {grant_id} is unavailable: {error}"))?;
            grant.check_binding(request.connector)?;
            if grant.status != "active" {
                return Err(format!("hosted grant {grant_id} is {}", grant.status));
            }
            if !grant.credential_material_bound {
                return Err(format!(
                    "hosted grant {grant_id} has no usable credential for its connector declaration; reconnect it"
                ));
            }
            if grant.principal_uri != instance.principal_uri.as_str() {
                return Err(format!(
                    "hosted grant {grant_id} is held by another principal than instance {}",
                    instance.instance_id
                ));
            }
            Ok(capability(
                instance,
                request.connector,
                Some(grant.grant_id),
                grant.scopes,
                grant.target_locator.into_iter().collect(),
            ))
        }
        // A secret is read before each call, never to admit one; admission
        // takes only a binding this runx home made, as it stands, to a
        // secret that is set.
        InstanceAuthority::Secret { scopes, .. } => {
            super::operation::credentials::check_secret(request.env, request.cwd, instance)?;
            Ok(capability(
                instance,
                request.connector,
                None,
                scopes
                    .iter()
                    .map(|scope| scope.as_str().to_owned())
                    .collect(),
                Vec::new(),
            ))
        }
        InstanceAuthority::Environment {
            credential,
            variable,
            scopes,
        } => {
            super::operation::credentials::from_environment(
                request.env,
                request.connector,
                credential,
                variable,
            )?;
            Ok(capability(
                instance,
                request.connector,
                None,
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
        InstanceAuthority::Session { scopes, .. } => Ok(capability(
            instance,
            request.connector,
            None,
            scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
                .collect(),
            Vec::new(),
        )),
        InstanceAuthority::Public => {
            let mut capability = capability(
                instance,
                request.connector,
                None,
                reachable_scopes(request.connector, public_may),
                Vec::new(),
            );
            if let Some(actor_ref) = request.actor_ref {
                capability.term.principal_ref = actor_ref.clone();
            }
            Ok(capability)
        }
        // What a purchase holds serves the operations on its resource, and
        // only that resource; runx Connect refuses a target it holds nothing
        // for before anything is sent.
        InstanceAuthority::Held { credential, target } => Ok(capability(
            instance,
            request.connector,
            None,
            reachable_scopes(request.connector, |operation| {
                operation.credentials.contains(credential)
            }),
            vec![target.as_str().to_owned()],
        )),
    }
}

/// Whether an instance with no authority may call `operation`: a read, or a
/// write to storage runx holds for the caller (the store the operator
/// configured, the signed-in account's artifact space). An approval alone
/// never authorizes a write: a connector that writes to a provider takes a
/// credential, a session or a grant.
fn public_may(operation: &ConnectorOperation) -> bool {
    operation.verb == OperationVerb::Read
        || operation
            .implementation
            .as_ref()
            .is_some_and(Implementation::writes_runx_storage)
}

/// The scopes of every operation an instance can reach.
fn reachable_scopes(
    connector: &LoadedConnector,
    reaches: impl Fn(&ConnectorOperation) -> bool,
) -> Vec<String> {
    connector
        .manifest
        .operations
        .values()
        .filter(|operation| reaches(operation))
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
/// A hosted grant issues it; a local instance is its own issuer, as the
/// operator confirmed it.
fn capability(
    instance: &ConnectorInstance,
    connector: &LoadedConnector,
    grant_id: Option<String>,
    scopes: Vec<String>,
    targets: Vec<String>,
) -> Capability {
    let (term_id, issued_by_ref, source) = match &grant_id {
        Some(grant_id) => (
            format!("capability:{grant_id}"),
            Reference::runx(ReferenceType::Grant, grant_id),
            format!("grant {grant_id}"),
        ),
        None => (
            format!("capability:{}", instance.instance_id),
            instance_reference(instance),
            format!("instance {}", instance.instance_id),
        ),
    };
    let public = matches!(instance.authority, InstanceAuthority::Public);
    let verbs = connector
        .manifest
        .operations
        .values()
        .filter(|operation| !public || public_may(operation))
        .map(|operation| operation.verb.authority_verb())
        .collect::<Vec<_>>();
    let mut unique_verbs = Vec::new();
    for verb in verbs {
        if !unique_verbs.contains(&verb) {
            unique_verbs.push(verb);
        }
    }
    let term = AuthorityTerm {
        term_id: NonEmptyString::from(term_id),
        principal_ref: principal_reference(instance),
        resource_ref: instance_reference(instance),
        resource_family: AuthorityResourceFamily::Connector,
        verbs: unique_verbs,
        bounds: AuthorityBounds {
            operations: vec![NonEmptyString::from(format!("{}/*", connector.name))],
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
            uses: None,
        },
        approvals: Vec::new(),
        expires_at: None,
        issued_by_ref,
        credential_ref: None,
    };
    Capability {
        grant_id,
        source,
        scopes,
        term,
    }
}

pub(crate) fn principal_reference(instance: &ConnectorInstance) -> Reference {
    Reference::with_uri(ReferenceType::Principal, instance.principal_uri.as_str())
}

pub(crate) fn instance_reference(instance: &ConnectorInstance) -> Reference {
    Reference::runx(
        ReferenceType::ConnectorInstance,
        instance.instance_id.as_str(),
    )
}

// A command session runs a shell tool, so these run where one does.
#[cfg(all(test, unix))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use runx_contracts::connector::SessionKind;
    use runx_contracts::{InstanceAuthority, ReferenceType};

    use super::{CapabilityRequest, evidence, instance_reference};
    use crate::http::{
        RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
    };
    use crate::kernel::operation::hosted::HostedClient;

    const MANIFEST: &str = r#"schema: runx.connector.v1
id: forge
name: Forge
description: A forge reached with the token its CLI prints.
runtime_capability: false
auth:
  credentials:
    token:
      kind: api_key
      fields: [{name: token}]
      delivery: {env: FORGE_TOKEN}
      audience: [api.forge.test]
  session:
    kind: command
    command: [forge-cli, auth, token]
    credential: token
    principal: {url: "https://api.forge.test/user", id: id, login: login}
placements:
  local: {auth: [session, credentials]}
operations:
  user.read:
    description: Read the signed-in user.
    verb: read
    scopes: [user.read]
    credentials: [token]
    target: {template: "https://api.forge.test/user"}
  repo.write:
    description: Write to a repository.
    verb: write
    scopes: [repo.write]
    credentials: [token]
    target: {template: "https://api.forge.test/repo"}
"#;

    /// The forge answers its user endpoint for the CLI's token.
    struct Account;

    impl RuntimeHttpTransport for Account {
        fn send(
            &self,
            _request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
            Ok(RuntimeHttpResponse::new(
                200,
                r#"{"id": 7, "login": "octo"}"#,
            ))
        }
    }

    /// A plugin holding the forge connector, and a signed-in `forge-cli`.
    fn plugin(root: &Path) -> BTreeMap<String, String> {
        std::fs::write(root.join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
        let connector = root.join("connectors/forge");
        std::fs::create_dir_all(&connector).unwrap();
        std::fs::write(connector.join("connector.yaml"), MANIFEST).unwrap();
        let tool = root.join("bin/forge-cli");
        std::fs::create_dir_all(tool.parent().unwrap()).unwrap();
        std::fs::write(&tool, "#!/bin/sh\necho tok-123\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        BTreeMap::from([
            (
                "PATH".to_owned(),
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            ),
            ("RUNX_CWD".to_owned(), root.display().to_string()),
            (
                "RUNX_HOME".to_owned(),
                root.join("home").display().to_string(),
            ),
        ])
    }

    #[test]
    fn a_session_serves_only_the_scopes_the_operator_confirmed_and_names_no_grant() {
        let root = tempfile::tempdir().unwrap();
        let env = plugin(root.path());
        let instances = crate::Instances::new(
            crate::KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, root.path()))
                .unwrap(),
        );
        let connector = crate::ConnectorScope::at(root.path())
            .unwrap()
            .connector("forge")
            .unwrap();
        let confirm = |scopes: &[String]| {
            crate::kernel::setup::confirm_session_over(
                &instances,
                &connector,
                scopes,
                runx_contracts::JsonObject::new(),
                &crate::kernel::setup::AccountReaders {
                    transport: &Account,
                    tools: &crate::kernel::operation::NoTools,
                },
                &env,
                root.path(),
            )
        };

        let narrowed = confirm(&["user.read".to_owned()]).expect("a served scope narrows");
        assert_eq!(
            narrowed.authority,
            InstanceAuthority::Session {
                session: SessionKind::Command,
                scopes: vec!["user.read".into()],
            }
        );
        let capability = evidence(&CapabilityRequest {
            connector: &connector,
            instance: &narrowed,
            hosted: &HostedClient::default(),
            env: &env,
            cwd: root.path(),
            actor_ref: Some(&runx_contracts::Reference::runx(
                ReferenceType::Principal,
                "another-actor",
            )),
        })
        .expect("evidence");
        assert_eq!(capability.scopes, vec!["user.read".to_owned()]);
        assert_eq!(capability.grant_id, None);
        assert_eq!(
            capability.term.principal_ref,
            super::principal_reference(&narrowed)
        );
        assert_eq!(
            capability.source,
            format!("instance {}", narrowed.instance_id)
        );
        assert_eq!(capability.term.issued_by_ref, instance_reference(&narrowed));
        assert_ne!(
            capability.term.issued_by_ref.reference_type,
            ReferenceType::Grant
        );

        let every = confirm(&[]).expect("no scope grants every served scope");
        let scopes = match &every.authority {
            InstanceAuthority::Session { scopes, .. } => scopes
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>(),
            other => vec![other.kind()],
        };
        assert_eq!(scopes, ["repo.write", "user.read"]);

        assert!(
            confirm(&["admin".to_owned()])
                .unwrap_err()
                .contains("serves no operation with scope admin")
        );
    }
}
