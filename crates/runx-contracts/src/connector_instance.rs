//! Connector instances: `runx.connector_instance.v1`.
//!
//! An instance is one connector bound to an account. It fixes the placement,
//! the principal and where the principal's authority is held, plus any
//! parameters a call should not supply (a Slack workspace, a base URL). A run
//! resolves each connector requirement to exactly one instance and never moves
//! admitted work to another. An instance is a binding, not authority:
//! capability evidence is read from the authority source at admission.
//!
//! Data stores are instances too: an instance of a store connector.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::JsonObject;
use crate::connector::{
    ConnectorDigestError, ConnectorName, CredentialName, Digest, EnvName, InstanceId, OperationName,
    Placement, SecretReference, SessionKind, stable_digest,
};
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ConnectorInstanceSchema {
    #[serde(rename = "runx.connector_instance.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.connector_instance.v1")]
pub struct ConnectorInstance {
    pub schema: ConnectorInstanceSchema,
    pub instance_id: InstanceId,
    pub connector: ConnectorName,
    pub manifest_digest: Digest,
    pub connector_digest: Digest,
    pub placement: Placement,
    /// The principal the instance acts for. Free of secret material.
    pub principal_ref: NonEmptyString,
    /// The provider account the instance acts as, pinned when it was
    /// confirmed (a GitHub user, a Slack team and user, a Google account).
    /// Work admitted against the instance never runs as another account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<NonEmptyString>,
    pub authority: InstanceAuthority,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: JsonObject,
    /// Where the instance's secret may be sent, pinned when it was confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_declaration_digest: Option<Digest>,
    /// What each operation meant when the instance was confirmed. A changed
    /// operation needs the instance confirmed again.
    pub operation_digests: BTreeMap<OperationName, Digest>,
    pub confirmed_at: IsoDateTime,
}

impl ConnectorInstance {
    /// Identity of the confirmed binding, excluding the time it was checked.
    /// Reconfirming unchanged pins must not invalidate work admitted to them.
    pub fn binding_digest(&self) -> Result<Digest, ConnectorDigestError> {
        stable_digest(&BindingIdentity {
            schema: self.schema,
            instance_id: &self.instance_id,
            connector: &self.connector,
            manifest_digest: &self.manifest_digest,
            connector_digest: &self.connector_digest,
            placement: self.placement,
            principal_ref: &self.principal_ref,
            account_ref: &self.account_ref,
            authority: &self.authority,
            parameters: &self.parameters,
            credential_declaration_digest: &self.credential_declaration_digest,
            operation_digests: &self.operation_digests,
        })
    }
}

#[derive(Serialize)]
struct BindingIdentity<'a> {
    schema: ConnectorInstanceSchema,
    instance_id: &'a InstanceId,
    connector: &'a ConnectorName,
    manifest_digest: &'a Digest,
    connector_digest: &'a Digest,
    placement: Placement,
    principal_ref: &'a NonEmptyString,
    account_ref: &'a Option<NonEmptyString>,
    authority: &'a InstanceAuthority,
    parameters: &'a JsonObject,
    credential_declaration_digest: &'a Option<Digest>,
    operation_digests: &'a BTreeMap<OperationName, Digest>,
}

/// Where the principal's authority over the provider is held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstanceAuthority {
    /// A runx Connect grant. Hosted placement only.
    HostedGrant { grant_id: NonEmptyString },
    /// A local credential profile holding the named connector credential.
    /// `scopes` is what the operator grants it: the connector's full scope
    /// set unless narrowed when it was bound. `material_ref` names the stored
    /// material it was bound to, so a profile replaced since, from this
    /// workspace or another, no longer serves it.
    Credential {
        profile: NonEmptyString,
        credential: CredentialName,
        scopes: Vec<NonEmptyString>,
        material_ref: NonEmptyString,
    },
    /// The named connector credential, read on every call from the variable
    /// its declaration delivers it in, in the process environment or the
    /// workspace `.env`. The instance holds the variable's name, never its
    /// value, so a key rotated there serves without binding again. `scopes`
    /// is what the operator grants it, as for a stored credential.
    Environment {
        credential: CredentialName,
        variable: EnvName,
        scopes: Vec<NonEmptyString>,
    },
    /// The named connector credential, read on every call from a secrets
    /// manager through that manager's own CLI and the operator's sign-in to
    /// it. The instance holds the reference, never the value, so a secret
    /// rotated there serves without binding again. `pinned` is what binding
    /// proved, the exact resource or server later reads go to (an AWS ARN, a
    /// Vault address), so a switched profile cannot serve another account's
    /// secret. `proof` is an HMAC keyed by the binding operator's runx home
    /// over the connector revision, the reference and its pin: a project
    /// store cannot supply a reference the operator did not bind. `scopes`
    /// is what the operator grants it.
    Reference {
        credential: CredentialName,
        reference: SecretReference,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pinned: Option<NonEmptyString>,
        proof: NonEmptyString,
        scopes: Vec<NonEmptyString>,
    },
    /// A local session the runtime can identify. A command session pins its
    /// account in the instance's `account_ref`. `scopes` is what the operator
    /// grants it: every scope its credential serves unless narrowed when it
    /// was confirmed. The provider enforces the account's own permissions.
    Session {
        session: SessionKind,
        scopes: Vec<NonEmptyString>,
    },
    /// A public API; reads only.
    Public,
}

impl InstanceAuthority {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::HostedGrant { .. } => "hosted_grant",
            Self::Credential { .. } => "credential",
            Self::Environment { .. } => "environment",
            Self::Reference { .. } => "reference",
            Self::Session { .. } => "session",
            Self::Public => "public",
        }
    }
}
