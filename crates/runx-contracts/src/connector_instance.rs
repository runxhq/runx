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
    ConnectorDigestError, ConnectorName, CredentialName, Digest, EnvName, InstanceId,
    OperationName, Placement, SecretName, SessionKind, stable_digest,
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
    pub principal_uri: NonEmptyString,
    /// The provider account the instance acts as, pinned when it was
    /// confirmed (a GitHub user, a Slack team and user, a Google account).
    /// Work admitted against the instance never runs as another account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<NonEmptyString>,
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
            principal_uri: &self.principal_uri,
            account_id: &self.account_id,
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
    principal_uri: &'a NonEmptyString,
    account_id: &'a Option<NonEmptyString>,
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
    /// The named connector credential, held by the secret the operator keeps
    /// in runx under `secret` (`runx secret set`): stored in the runx home, or
    /// read through a secrets manager by reference. Every call reads it again,
    /// so setting it anew serves without binding again. `proof` is an HMAC
    /// keyed by the binding operator's runx home over the whole binding, the
    /// proof left out: a project store can neither name the operator's
    /// secret nor widen a binding. `scopes` is what the operator grants it.
    Secret {
        credential: CredentialName,
        secret: SecretName,
        scopes: Vec<NonEmptyString>,
        proof: NonEmptyString,
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
    /// A local session the runtime can identify. A command session pins its
    /// account in the instance's `account_id`. `scopes` is what the operator
    /// grants it: every scope its credential serves unless narrowed when it
    /// was confirmed. The provider enforces the account's own permissions.
    Session {
        session: SessionKind,
        scopes: Vec<NonEmptyString>,
    },
    /// What a purchase bought (S10, N11): one resource's credential, held by
    /// runx Connect for the principal and presented only to the requests of
    /// operations on that resource. Admission pins it for one call, by the
    /// target the call names; it is never confirmed or selected. Hosted
    /// placement only.
    Held {
        credential: CredentialName,
        target: NonEmptyString,
    },
    /// A public API; reads only.
    Public,
}

impl InstanceAuthority {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::HostedGrant { .. } => "hosted_grant",
            Self::Secret { .. } => "secret",
            Self::Environment { .. } => "environment",
            Self::Session { .. } => "session",
            Self::Held { .. } => "held",
            Self::Public => "public",
        }
    }
}
