//! Connector instances: what a connector requirement resolves to.
//!
//! An instance is confirmed during setup (`runx connect`, `runx credential
//! set`, `runx connector use`) and selected as its connector's default. A
//! call resolves to exactly one instance, from one of three sources:
//!
//! - **Injected:** a host (the hosted worker, an embedder) injects a complete
//!   hosted grant through the environment. It wins over any selection for a
//!   connector with a hosted placement, and cannot stand for any other.
//! - **Selected:** the instance the operator selected for the connector.
//! - **Public:** a connector whose only authority is `none` has exactly one
//!   instance, which needs no setup.
//!
//! There is no fallback. A selected instance that is missing or revoked
//! refuses, naming itself; work never moves to a sibling instance or another
//! account.

use std::collections::BTreeMap;
use std::sync::Arc;

use runx_contracts::connector::{
    ConnectorRef, InstanceId, Placement, credential_declaration_digest,
};
use runx_contracts::connector_instance::ConnectorInstanceSchema;
use runx_contracts::schema::IsoDateTime;
use runx_contracts::{ConnectorInstance, InstanceAuthority};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::connectors::LoadedConnector;
use super::store::{Append, Expected, KernelStore, KernelStoreError, StreamEvent};

/// A host-injected hosted grant: its id, its scopes and its principal. All
/// three or none.
pub const INJECTED_GRANT_ID_ENV: &str = "RUNX_PROVIDER_PERMISSION_GRANT_ID";
pub const INJECTED_GRANTED_SCOPES_ENV: &str = "RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES";
pub const INJECTED_PRINCIPAL_REF_ENV: &str = "RUNX_PROVIDER_PERMISSION_PRINCIPAL_REF";
pub const INJECTED_CONNECTOR_BINDING_ENV: &str = "RUNX_PROVIDER_PERMISSION_CONNECTOR_BINDING";

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
        "no instance is selected for connector {connector}; `runx connector inspect {connector}` shows how to set one up"
    )]
    NotSelected { connector: ConnectorRef },
    #[error("instance {instance_id} does not exist")]
    Missing { instance_id: String },
    #[error("instance {instance_id} was revoked")]
    Revoked { instance_id: String },
    #[error("instance {instance_id} belongs to connector {actual}, not {expected}")]
    WrongConnector {
        instance_id: String,
        expected: ConnectorRef,
        actual: ConnectorRef,
    },
    #[error(
        "instance {instance_id} uses the {placement} placement, which connector {connector} does not support"
    )]
    PlacementUnsupported {
        instance_id: String,
        connector: ConnectorRef,
        placement: &'static str,
    },
    #[error(
        "instance {instance_id} holds {authority} authority, which the {placement} placement of connector {connector} does not accept"
    )]
    AuthorityUnsupported {
        instance_id: String,
        connector: ConnectorRef,
        placement: &'static str,
        authority: &'static str,
    },
    #[error("host-injected grant authority is incomplete: {0}")]
    InjectedIncomplete(&'static str),
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

    /// Resolve the instance a call on `connector` runs under.
    pub(crate) fn resolve(
        &self,
        connector: &LoadedConnector,
        env: &BTreeMap<String, String>,
    ) -> Result<ResolvedInstance, InstanceError> {
        if let Some(injected) = injected(connector, env)? {
            return Ok(injected);
        }
        let Some(instance_id) = self.selected(connector.connector_ref.as_str())? else {
            return public(connector).ok_or_else(|| InstanceError::NotSelected {
                connector: connector.connector_ref.clone(),
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
        Ok(ResolvedInstance {
            instance: record.instance,
            source: InstanceSource::Selected,
        })
    }

    fn selection_append(
        &self,
        connector: &ConnectorRef,
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

/// Where a resolved instance came from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum InstanceSource {
    /// A host-injected grant, with the scopes the host attests.
    Injected {
        granted_scopes: Vec<String>,
    },
    Selected,
    Public,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedInstance {
    pub instance: ConnectorInstance,
    pub source: InstanceSource,
}

/// Whether the connector, as currently loaded, still supports the instance's
/// placement and authority.
pub(crate) fn check_supported(
    connector: &LoadedConnector,
    instance: &ConnectorInstance,
) -> Result<(), InstanceError> {
    let manifest = &connector.manifest;
    if instance.connector != connector.connector_ref {
        return Err(InstanceError::WrongConnector {
            instance_id: instance.instance_id.as_str().to_owned(),
            expected: connector.connector_ref.clone(),
            actual: instance.connector.clone(),
        });
    }
    let unsupported_placement = || InstanceError::PlacementUnsupported {
        instance_id: instance.instance_id.as_str().to_owned(),
        connector: connector.connector_ref.clone(),
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
                        InstanceAuthority::Credential { .. },
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
            connector: connector.connector_ref.clone(),
            placement: instance.placement.as_str(),
            authority: instance.authority.kind(),
        })
    }
}

fn injected(
    connector: &LoadedConnector,
    env: &BTreeMap<String, String>,
) -> Result<Option<ResolvedInstance>, InstanceError> {
    // A hosted grant can only stand for a connector that has a hosted
    // placement; a local-only connector resolves as if none were injected.
    if connector.manifest.placements.hosted.is_none() {
        return Ok(None);
    }
    let value = |name: &str| {
        env.get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let (grant_id, scopes, principal_ref, binding) = match (
        value(INJECTED_GRANT_ID_ENV),
        value(INJECTED_GRANTED_SCOPES_ENV),
        value(INJECTED_PRINCIPAL_REF_ENV),
        value(INJECTED_CONNECTOR_BINDING_ENV),
    ) {
        (None, None, None, None) => return Ok(None),
        (Some(grant_id), Some(scopes), Some(principal_ref), Some(binding)) => {
            (grant_id, scopes, principal_ref, binding)
        }
        _ => {
            return Err(InstanceError::InjectedIncomplete(
                "set the grant id, granted scopes, principal and exact connector binding together",
            ));
        }
    };
    let binding =
        decode_injected_connector_binding(binding).ok_or(InstanceError::InjectedIncomplete(
            "the connector binding must name a qualified connector and exact digests",
        ))?;
    if binding != connector.binding() {
        return Err(InstanceError::InjectedIncomplete(
            "the injected grant does not bind this connector revision",
        ));
    }
    let granted_scopes =
        decode_injected_scopes(scopes).ok_or(InstanceError::InjectedIncomplete(
            "the granted scopes must be a JSON array of non-blank strings",
        ))?;
    let instance_id =
        instance_id(&["injected", connector.connector_ref.as_str()]).map_err(|_| {
            InstanceError::InjectedIncomplete("the connector reference is not an instance id")
        })?;
    let instance = pinned(
        connector,
        instance_id,
        Placement::Hosted,
        principal_ref.to_owned(),
        None,
        InstanceAuthority::HostedGrant {
            grant_id: grant_id.to_owned().into(),
        },
    );
    check_supported(connector, &instance)?;
    Ok(Some(ResolvedInstance {
        instance,
        source: InstanceSource::Injected { granted_scopes },
    }))
}

/// The instance a connector runs under when none is selected: its public
/// one, when its local placement admits acting with no authority. The store's
/// is the workspace store.
fn public(connector: &LoadedConnector) -> Option<ResolvedInstance> {
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
    let instance_id = instance_id(&["public", connector.connector_ref.as_str()]).ok()?;
    let mut instance = pinned(
        connector,
        instance_id,
        Placement::Local,
        "runx:public".to_owned(),
        None,
        InstanceAuthority::Public,
    );
    instance.confirmed_at = IsoDateTime::from("1970-01-01T00:00:00Z".to_owned());
    Some(ResolvedInstance {
        instance,
        source: InstanceSource::Public,
    })
}

/// An instance of `connector` as it is loaded now: its operations, and the
/// declaration of the credential it holds, are pinned. Confirming it again
/// after the connector changes re-pins them.
/// The credential an instance delivers to implementations: its stored
/// profile's, or the one its command session's token stands in for.
pub(crate) fn delivered_credential<'a>(
    manifest: &'a runx_contracts::connector::ConnectorManifest,
    authority: &'a InstanceAuthority,
) -> Option<&'a runx_contracts::connector::CredentialName> {
    match authority {
        InstanceAuthority::Credential { credential, .. } => Some(credential),
        InstanceAuthority::Session {
            session: runx_contracts::connector::SessionKind::Command,
        } => match manifest.auth.session.as_ref()? {
            runx_contracts::connector::SessionAuth::Command { credential, .. } => Some(credential),
            _ => None,
        },
        _ => None,
    }
}

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
        connector: connector.connector_ref.clone(),
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

/// The environment carries strings; scopes are opaque strings that may hold
/// any delimiter, so they travel as a JSON array.
#[must_use]
pub fn encode_injected_scopes(scopes: &[String]) -> Option<String> {
    valid_scopes(scopes)
        .then(|| serde_json::to_string(scopes).ok())
        .flatten()
}

#[must_use]
pub fn decode_injected_scopes(value: &str) -> Option<Vec<String>> {
    serde_json::from_str::<Vec<String>>(value)
        .ok()
        .filter(|scopes| valid_scopes(scopes))
}

#[must_use]
pub fn decode_injected_connector_binding(
    value: &str,
) -> Option<runx_contracts::connector::ConnectorBinding> {
    serde_json::from_str(value).ok()
}

fn valid_scopes(scopes: &[String]) -> bool {
    !scopes.is_empty() && scopes.iter().all(|scope| !scope.trim().is_empty())
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
