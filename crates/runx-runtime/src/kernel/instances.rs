//! Connector instances: what a connector requirement resolves to.
//!
//! An instance is confirmed during setup (`runx connector use`) and selected
//! as its connector's default. A call on a resource a purchase holds runs
//! under that hold; any other call resolves to exactly one instance, from one
//! of three sources:
//!
//! - **Stated:** a run's host (the hosted worker, an embedder) names, in the
//!   run authority, the hosted grant a connector's calls run under. The grant
//!   is read through the host, never asserted. It stands only for a hosted
//!   placement, and a selection of another instance is refused.
//! - **Selected:** the instance the operator selected for the connector.
//! - **Environment:** with none selected, the credential whose variable is
//!   set, bound to that variable exactly as `runx connector use` would bind
//!   it, derived for the run and never written. Two set at once refuse.
//! - **Public:** a connector whose only authority is `none` has exactly one
//!   instance, which needs no setup.
//!
//! There is no fallback. A stated or selected instance that is missing or
//! revoked refuses, naming itself; work never moves to a sibling instance or
//! another account.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use runx_contracts::connector::{
    ConnectorName, CredentialName, EnvName, InstanceId, LocalAuth, Placement,
    credential_declaration_digest,
};
use runx_contracts::connector_instance::ConnectorInstanceSchema;
use runx_contracts::schema::IsoDateTime;
use runx_contracts::{ConnectorInstance, InstanceAuthority};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::connectors::LoadedConnector;
use super::store::{Append, Expected, KernelStore, KernelStoreError, StreamEvent};

const PAGE: usize = 500;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum InstanceEvent {
    Confirmed { instance: Box<ConnectorInstance> },
    Revoked,
}

impl StreamEvent for InstanceEvent {
    const KIND: &'static str = "instance";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Confirmed { .. } => "confirmed",
            Self::Revoked => "revoked",
        }
    }
}

/// A connector's default instance. Keyed by connector id.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum SelectionEvent {
    Selected { instance_id: InstanceId },
}

impl StreamEvent for SelectionEvent {
    const KIND: &'static str = "instance_selection";

    fn event_type(&self) -> &'static str {
        "selected"
    }
}

/// An instance's current state.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceRecord {
    pub instance: ConnectorInstance,
    pub revoked: bool,
    pub selected: bool,
}

#[derive(Debug, Error)]
pub enum InstanceError {
    #[error(
        "no instance is selected for connector {connector}, and none of its credentials' variables is set; set one, or `runx connector inspect {connector}` shows every way to set it up"
    )]
    NotSelected { connector: ConnectorName },
    #[error(
        "connector {connector} has no instance selected and the variables of several of its credentials are set ({variables}); bind the one it runs as with `runx connector use {connector} --credential <name>`"
    )]
    SeveralVariables {
        connector: ConnectorName,
        variables: String,
    },
    #[error("connector {connector} cannot run as {variable}: {reason}")]
    EnvironmentUnusable {
        connector: ConnectorName,
        variable: EnvName,
        reason: String,
    },
    #[error("instance {instance_id} does not exist")]
    Missing { instance_id: String },
    #[error("instance {instance_id} was revoked")]
    Revoked { instance_id: String },
    #[error("instance {instance_id} belongs to connector {actual}, not {expected}")]
    WrongConnector {
        instance_id: String,
        expected: ConnectorName,
        actual: ConnectorName,
    },
    #[error(
        "instance {instance_id} uses the {placement} placement, which connector {connector} does not support"
    )]
    PlacementUnsupported {
        instance_id: String,
        connector: ConnectorName,
        placement: &'static str,
    },
    #[error(
        "instance {instance_id} holds {authority} authority, which the {placement} placement of connector {connector} does not accept"
    )]
    AuthorityUnsupported {
        instance_id: String,
        connector: ConnectorName,
        placement: &'static str,
        authority: &'static str,
    },
    #[error(
        "the run's host states grant {grant_id} for connector {connector}, which is unavailable: {reason}"
    )]
    StatedUnavailable {
        grant_id: String,
        connector: ConnectorName,
        reason: String,
    },
    #[error(
        "the run's host states grant {grant_id} for connector {connector}, but instance {selected} is selected; a run never chooses between them"
    )]
    StatedConflict {
        grant_id: String,
        connector: ConnectorName,
        selected: InstanceId,
    },
    #[error(transparent)]
    Store(#[from] KernelStoreError),
}

/// The instances recorded in one kernel store.
#[derive(Clone)]
pub struct Instances {
    store: Arc<KernelStore>,
}

impl Instances {
    #[must_use]
    pub fn new(store: Arc<KernelStore>) -> Self {
        Self { store }
    }

    /// Record a confirmed instance, and optionally select it as its
    /// connector's default, in one commit. Confirming an existing id again
    /// replaces its pins; the id cannot move to another connector.
    pub fn confirm(&self, instance: ConnectorInstance, select: bool) -> Result<(), InstanceError> {
        let key = instance.instance_id.as_str().to_owned();
        let current = self.store.read::<InstanceEvent>(&key)?;
        if let Some(existing) = latest_instance(&current)
            && existing.connector != instance.connector
        {
            return Err(InstanceError::WrongConnector {
                instance_id: key,
                expected: instance.connector,
                actual: existing.connector.clone(),
            });
        }
        let mut batch = Vec::with_capacity(2);
        if select {
            batch.push(self.selection_append(&instance.connector, &instance.instance_id)?);
        }
        batch.push(Append::kernel(
            key,
            expected(current.len()),
            &InstanceEvent::Confirmed {
                instance: Box::new(instance),
            },
        )?);
        self.store.append(batch)?;
        Ok(())
    }

    /// Revoke an instance. Work admitted against it is held, never moved.
    pub fn revoke(&self, instance_id: &str) -> Result<(), InstanceError> {
        let current = self.store.read::<InstanceEvent>(instance_id)?;
        if latest_instance(&current).is_none() {
            return Err(InstanceError::Missing {
                instance_id: instance_id.to_owned(),
            });
        }
        self.store.append(vec![Append::kernel(
            instance_id,
            expected(current.len()),
            &InstanceEvent::Revoked,
        )?])?;
        Ok(())
    }

    /// Select an existing, unrevoked instance as its connector's default.
    pub fn select(&self, instance_id: &str) -> Result<ConnectorInstance, InstanceError> {
        let record = self
            .get(instance_id)?
            .ok_or_else(|| InstanceError::Missing {
                instance_id: instance_id.to_owned(),
            })?;
        if record.revoked {
            return Err(InstanceError::Revoked {
                instance_id: instance_id.to_owned(),
            });
        }
        self.store.append(vec![self.selection_append(
            &record.instance.connector,
            &record.instance.instance_id,
        )?])?;
        Ok(record.instance)
    }

    pub fn get(&self, instance_id: &str) -> Result<Option<InstanceRecord>, InstanceError> {
        let events = self.store.read::<InstanceEvent>(instance_id)?;
        let Some(instance) = latest_instance(&events).cloned() else {
            return Ok(None);
        };
        let revoked = matches!(
            events.last().map(|recorded| &recorded.event),
            Some(InstanceEvent::Revoked)
        );
        let selected =
            self.selected(instance.connector.as_str())?.as_ref() == Some(&instance.instance_id);
        Ok(Some(InstanceRecord {
            instance,
            revoked,
            selected,
        }))
    }

    /// The selected instance id for a connector.
    pub fn selected(&self, connector: &str) -> Result<Option<InstanceId>, InstanceError> {
        Ok(self
            .store
            .read::<SelectionEvent>(connector)?
            .pop()
            .map(|recorded| match recorded.event {
                SelectionEvent::Selected { instance_id } => instance_id,
            }))
    }

    /// Every instance, in id order.
    pub fn list(&self) -> Result<Vec<InstanceRecord>, InstanceError> {
        let mut records = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let heads = self.store.heads::<InstanceEvent>(after.as_deref(), PAGE)?;
            let full = heads.len() == PAGE;
            for head in &heads {
                if let Some(record) = self.get(&head.key)? {
                    records.push(record);
                }
            }
            match heads.into_iter().last() {
                Some(last) if full => after = Some(last.key),
                _ => return Ok(records),
            }
        }
    }

    /// Resolve the instance a call on `connector` runs under: the one the
    /// operator selected; with none selected, the credential whose variable
    /// `env` sets; else its public instance.
    pub(crate) fn resolve(
        &self,
        connector: &LoadedConnector,
        env: &BTreeMap<String, String>,
    ) -> Result<ConnectorInstance, InstanceError> {
        let Some(instance_id) = self.selected(connector.name.as_str())? else {
            if let Some(instance) = from_environment(connector, env)? {
                return Ok(instance);
            }
            return public(connector).ok_or_else(|| InstanceError::NotSelected {
                connector: connector.name.clone(),
            });
        };
        let record = self
            .get(instance_id.as_str())?
            .ok_or_else(|| InstanceError::Missing {
                instance_id: instance_id.as_str().to_owned(),
            })?;
        if record.revoked {
            return Err(InstanceError::Revoked {
                instance_id: instance_id.as_str().to_owned(),
            });
        }
        check_supported(connector, &record.instance)?;
        Ok(record.instance)
    }

    fn selection_append(
        &self,
        connector: &ConnectorName,
        instance_id: &InstanceId,
    ) -> Result<Append, InstanceError> {
        let version = self.store.version::<SelectionEvent>(connector.as_str())?;
        Ok(Append::kernel(
            connector.as_str(),
            if version == 0 {
                Expected::Absent
            } else {
                Expected::Version(version)
            },
            &SelectionEvent::Selected {
                instance_id: instance_id.clone(),
            },
        )?)
    }
}

/// The instance a run's host states for `connector`: the hosted grant it
/// names, read through the host and pinned to the connector as loaded. It
/// stands only for a hosted placement, and a selection of another instance
/// is refused, never used.
pub(crate) fn stated(
    instances: &Instances,
    connector: &LoadedConnector,
    grant_id: &str,
    hosted: &super::operation::hosted::HostedClient,
    env: &BTreeMap<String, String>,
    cwd: &std::path::Path,
) -> Result<ConnectorInstance, InstanceError> {
    let instance = super::setup::hosted_grant_instance(connector, hosted, grant_id, env, cwd)
        .map_err(|reason| InstanceError::StatedUnavailable {
            grant_id: grant_id.to_owned(),
            connector: connector.name.clone(),
            reason,
        })?;
    check_supported(connector, &instance)?;
    if let Some(selected) = instances.selected(connector.name.as_str())?
        && selected != instance.instance_id
    {
        return Err(InstanceError::StatedConflict {
            grant_id: grant_id.to_owned(),
            connector: connector.name.clone(),
            selected,
        });
    }
    Ok(instance)
}

/// Whether the connector, as currently loaded, still supports the instance's
/// placement and authority.
pub(crate) fn check_supported(
    connector: &LoadedConnector,
    instance: &ConnectorInstance,
) -> Result<(), InstanceError> {
    let manifest = &connector.manifest;
    if instance.connector != connector.name {
        return Err(InstanceError::WrongConnector {
            instance_id: instance.instance_id.as_str().to_owned(),
            expected: connector.name.clone(),
            actual: instance.connector.clone(),
        });
    }
    let unsupported_placement = || InstanceError::PlacementUnsupported {
        instance_id: instance.instance_id.as_str().to_owned(),
        connector: connector.name.clone(),
        placement: instance.placement.as_str(),
    };
    let accepted = match instance.placement {
        Placement::Hosted => {
            let hosted = manifest
                .placements
                .hosted
                .as_ref()
                .ok_or_else(unsupported_placement)?;
            matches!(
                (&instance.authority, hosted.auth),
                (
                    InstanceAuthority::HostedGrant { .. },
                    runx_contracts::connector::HostedAuth::Delegated
                ) | (
                    InstanceAuthority::HostedGrant { .. },
                    runx_contracts::connector::HostedAuth::Credential
                )
            )
        }
        Placement::Local => {
            let local = manifest
                .placements
                .local
                .as_ref()
                .ok_or_else(unsupported_placement)?;
            local.auth.iter().any(|auth| {
                matches!(
                    (&instance.authority, auth),
                    (
                        InstanceAuthority::Secret { .. }
                            | InstanceAuthority::Environment { .. }
                            | InstanceAuthority::Credential { .. }
                            | InstanceAuthority::Reference { .. },
                        runx_contracts::connector::LocalAuth::Credentials
                    ) | (
                        InstanceAuthority::Session { .. },
                        runx_contracts::connector::LocalAuth::Session
                    ) | (
                        InstanceAuthority::Public,
                        runx_contracts::connector::LocalAuth::None
                    )
                )
            })
        }
    };
    if accepted {
        Ok(())
    } else {
        Err(InstanceError::AuthorityUnsupported {
            instance_id: instance.instance_id.as_str().to_owned(),
            connector: connector.name.clone(),
            placement: instance.placement.as_str(),
            authority: instance.authority.kind(),
        })
    }
}

/// The instance a connector runs under when none is selected: its public
/// one, when its local placement admits acting with no authority. The store's
/// is the workspace store.
fn public(connector: &LoadedConnector) -> Option<ConnectorInstance> {
    let manifest = &connector.manifest;
    let admits_none = manifest.auth.none.is_some()
        && manifest.placements.local.as_ref().is_some_and(|local| {
            local
                .auth
                .contains(&runx_contracts::connector::LocalAuth::None)
        });
    if !admits_none {
        return None;
    }
    let instance_id = instance_id(&["public", connector.name.as_str()]).ok()?;
    let mut instance = pinned(
        connector,
        instance_id,
        Placement::Local,
        "runx:public".to_owned(),
        None,
        InstanceAuthority::Public,
    );
    instance.confirmed_at = IsoDateTime::from(NEVER_CONFIRMED.to_owned());
    Some(instance)
}

/// When an instance derived for a run, and never confirmed, says it was.
const NEVER_CONFIRMED: &str = "1970-01-01T00:00:00Z";

/// The instance a connector with none selected runs under when its local
/// placement takes credentials and exactly one of its credentials' variables
/// is set: that variable, bound as `runx connector use` would bind it. A
/// connector that pins parameters is set up once, with them.
fn from_environment(
    connector: &LoadedConnector,
    env: &BTreeMap<String, String>,
) -> Result<Option<ConnectorInstance>, InstanceError> {
    let manifest = &connector.manifest;
    let takes_credentials = manifest
        .placements
        .local
        .as_ref()
        .is_some_and(|local| local.auth.contains(&LocalAuth::Credentials));
    if !takes_credentials || manifest.parameters.is_some() {
        return Ok(None);
    }
    let set = manifest
        .auth
        .credentials
        .iter()
        .filter_map(|(credential, declaration)| {
            let variable = declaration.env()?;
            env.get(variable.as_str())
                .filter(|value| !value.trim().is_empty())?;
            Some((credential, variable))
        })
        .collect::<Vec<_>>();
    let (credential, variable) = match set.as_slice() {
        [] => return Ok(None),
        [only] => *only,
        several => {
            return Err(InstanceError::SeveralVariables {
                connector: connector.name.clone(),
                variables: several
                    .iter()
                    .map(|(_, variable)| variable.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
    };
    let unusable = |reason: String| InstanceError::EnvironmentUnusable {
        connector: connector.name.clone(),
        variable: variable.clone(),
        reason,
    };
    let scopes = served_scopes(connector, credential, &[]).map_err(unusable)?;
    let mut instance =
        environment_instance(connector, credential, variable.clone(), scopes).map_err(unusable)?;
    instance.confirmed_at = IsoDateTime::from(NEVER_CONFIRMED.to_owned());
    Ok(Some(instance))
}

/// The instance that holds `connector`'s `credential` in `variable`,
/// granted `scopes`: the variable's name, never its value, read again on
/// every call.
pub(crate) fn environment_instance(
    connector: &LoadedConnector,
    credential: &CredentialName,
    variable: EnvName,
    scopes: BTreeSet<String>,
) -> Result<ConnectorInstance, String> {
    Ok(pinned(
        connector,
        instance_id(&[
            "environment",
            connector.name.as_str(),
            credential.as_str(),
            variable.as_str(),
        ])?,
        Placement::Local,
        format!(
            "runx:principal:local-environment:{}:{variable}",
            connector.manifest.id
        ),
        None,
        InstanceAuthority::Environment {
            credential: credential.clone(),
            variable,
            scopes: scopes.into_iter().map(Into::into).collect(),
        },
    ))
}

/// What the operator grants an instance holding `credential`: the requested
/// scopes, each served by an operation that takes the credential, or every
/// such scope when none is requested.
pub(crate) fn served_scopes(
    connector: &LoadedConnector,
    credential: &CredentialName,
    scopes: &[String],
) -> Result<BTreeSet<String>, String> {
    granted_scopes(
        connector,
        |operation| operation.credentials.contains(credential),
        scopes,
        &format!(
            "credential {credential} of connector {}",
            connector.manifest.id
        ),
    )
}

/// The scopes `scopes` grants of the operations `serves` admits: each one
/// such an operation declares, or all of theirs when none is named.
pub(crate) fn granted_scopes(
    connector: &LoadedConnector,
    serves: impl Fn(&runx_contracts::connector::ConnectorOperation) -> bool,
    scopes: &[String],
    what: &str,
) -> Result<BTreeSet<String>, String> {
    let servable = connector
        .manifest
        .operations
        .values()
        .filter(|operation| serves(operation))
        .flat_map(|operation| {
            operation
                .scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
        })
        .collect::<BTreeSet<_>>();
    if let Some(scope) = scopes
        .iter()
        .find(|scope| !servable.contains(scope.as_str()))
    {
        return Err(format!("{what} serves no operation with scope {scope}"));
    }
    Ok(if scopes.is_empty() {
        servable
    } else {
        scopes.iter().cloned().collect()
    })
}

/// The credential an instance delivers to implementations: its secret's or
/// environment variable's, or the one its command session's token stands in
/// for. A binding recorded before `runx secret` names the credential it
/// held, and delivers nothing.
pub(crate) fn delivered_credential<'a>(
    manifest: &'a runx_contracts::connector::ConnectorManifest,
    authority: &'a InstanceAuthority,
) -> Option<&'a runx_contracts::connector::CredentialName> {
    match authority {
        InstanceAuthority::Secret { credential, .. }
        | InstanceAuthority::Environment { credential, .. }
        | InstanceAuthority::Credential { credential, .. }
        | InstanceAuthority::Reference { credential, .. } => Some(credential),
        InstanceAuthority::Session {
            session: runx_contracts::connector::SessionKind::Command,
            ..
        } => match manifest.auth.session.as_ref()? {
            runx_contracts::connector::SessionAuth::Command { credential, .. } => Some(credential),
            _ => None,
        },
        _ => None,
    }
}

/// An instance of `connector` as it is loaded now: its operations, and the
/// declaration of the credential it holds, are pinned. Confirming it again
/// after the connector changes re-pins them.
pub(crate) fn pinned(
    connector: &LoadedConnector,
    instance_id: InstanceId,
    placement: Placement,
    principal_ref: String,
    account_ref: Option<String>,
    authority: InstanceAuthority,
) -> ConnectorInstance {
    let credential_declaration_digest = delivered_credential(&connector.manifest, &authority)
        .and_then(|credential| connector.manifest.auth.credentials.get(credential))
        .and_then(|declaration| credential_declaration_digest(declaration).ok());
    ConnectorInstance {
        schema: ConnectorInstanceSchema::V1,
        instance_id,
        connector: connector.name.clone(),
        manifest_digest: connector.manifest_digest.clone(),
        connector_digest: connector.connector_digest.clone(),
        placement,
        principal_ref: principal_ref.into(),
        account_ref: account_ref.map(Into::into),
        authority,
        parameters: BTreeMap::new(),
        credential_declaration_digest,
        operation_digests: connector.operation_digests.clone(),
        confirmed_at: IsoDateTime::from(crate::time::now_iso8601()),
    }
}

fn latest_instance(events: &[super::store::Recorded<InstanceEvent>]) -> Option<&ConnectorInstance> {
    events
        .iter()
        .rev()
        .find_map(|recorded| match &recorded.event {
            InstanceEvent::Confirmed { instance } => Some(instance.as_ref()),
            InstanceEvent::Revoked => None,
        })
}

fn expected(len: usize) -> Expected {
    if len == 0 {
        Expected::Absent
    } else {
        Expected::Version(len as u64)
    }
}

#[cfg(test)]
mod tests;

/// Instance keys bind exact components without lossy normalization or leaking
/// account labels. Namespace separators cannot collide with connector names.
pub fn instance_id(parts: &[&str]) -> Result<InstanceId, String> {
    let kind = parts.first().ok_or("instance kind is missing")?;
    let digest =
        runx_contracts::connector::stable_digest(&parts).map_err(|error| error.to_string())?;
    InstanceId::new(format!("{kind}.{}", &digest.as_str()[7..]))
        .ok_or_else(|| "instance kind is invalid".to_owned())
}
