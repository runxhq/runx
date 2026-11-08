//! Connector contracts: `runx.connector.v1`.
//!
//! A connector is the one definition of how runx reaches a provider: its auth
//! methods, the placements that can execute it, and its operations. Skills name
//! operations, and the kernel admits each call against the operation defined
//! here. A manifest carries no authority. It says what an operation means;
//! capability, delegation and approval evidence decide whether a call may run.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::schema::{NonEmptyString, NonEmptyVec, RunxSchema};
use crate::{AuthorityVerb, JsonObject};

/// Deserialize an externally tagged enum of newtype variants from a
/// single-key map, streaming: YAML and JSON read it the same way, with no tag
/// syntax and no buffered intermediate.
macro_rules! single_key_enum {
    ($enum:ident, $expecting:literal, { $($key:literal => $variant:ident),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $enum {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct Single;

                impl<'de> serde::de::Visitor<'de> for Single {
                    type Value = $enum;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str(concat!($expecting, " as a single-key map"))
                    }

                    fn visit_map<A>(self, mut map: A) -> Result<$enum, A::Error>
                    where
                        A: serde::de::MapAccess<'de>,
                    {
                        const KEYS: &[&str] = &[$($key),+];
                        let key: std::borrow::Cow<'de, str> = map
                            .next_key()?
                            .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                        let value = match key.as_ref() {
                            $($key => $enum::$variant(map.next_value()?),)+
                            other => return Err(serde::de::Error::unknown_variant(other, KEYS)),
                        };
                        if map.next_key::<serde::de::IgnoredAny>()?.is_some() {
                            return Err(serde::de::Error::custom(concat!(
                                $expecting,
                                " has exactly one key"
                            )));
                        }
                        Ok(value)
                    }
                }

                deserializer.deserialize_map(Single)
            }
        }
    };
}

mod amount;
mod cost;
mod digest;
pub(crate) mod ids;
mod template;

pub use amount::{AmountBinding, AmountRule};
pub use cost::{CallCost, CostBasis, CostPayer, CostSource, OperationCost};
pub use digest::{
    ConnectorDigestError, connector_digest, credential_declaration_digest, implementation_digest,
    input_schema_digest, manifest_digest, mcp_tool_digest, operation_digest, stable_digest,
    tool_package_digest,
};
pub use ids::{
    CONNECTOR_NAME_PATTERN, CREDENTIAL_NAME_PATTERN, ConnectorName, CredentialName, DIGEST_PATTERN,
    Digest, ENV_NAME_PATTERN, EnvName, INSTANCE_ID_PATTERN, InstanceId, OPERATION_NAME_MAX_LENGTH,
    OPERATION_NAME_PATTERN, OperationName, RESERVED_CONNECTOR_SEGMENT, SECRET_NAME_PATTERN,
    SECRET_REFERENCE_EXPECTED, SECRET_REFERENCE_PATTERN, SECRET_SCHEMES, SecretName,
    SecretReference, is_connector_name, is_operation_name, is_publisher_namespace,
    is_secret_reference,
};

/// An exact connector revision. Acquisition, grant delivery and hosted calls
/// carry this same binding; a name alone cannot substitute bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.connector_binding.v1")]
pub struct ConnectorBinding {
    pub connector: ConnectorName,
    pub manifest_digest: Digest,
    pub connector_digest: Digest,
}
pub use template::{Template, TemplateError, TemplateInputs, decimal_units};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ConnectorSchema {
    #[serde(rename = "runx.connector.v1")]
    V1,
}

/// One provider's operations and how runx reaches them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.connector.v1")]
pub struct ConnectorManifest {
    pub schema: ConnectorSchema,
    /// The connector's whole name: `stripe`, `google/calendar`.
    pub name: ConnectorName,
    /// Its display name, such as "GitHub".
    pub title: NonEmptyString,
    pub description: NonEmptyString,
    /// The connector category it declares from the registry taxonomy
    /// (`registry/taxonomy.json`); a connector that declares none is
    /// uncategorized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<NonEmptyString>,
    pub auth: ConnectorAuth,
    /// The JSON Schema of what an instance pins besides its authority, such
    /// as a store's location. Templates bind them as `$parameter`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<JsonObject>,
    pub placements: ConnectorPlacements,
    pub operations: BTreeMap<OperationName, ConnectorOperation>,
    /// What the connector's locators name, so references to them are
    /// classified without the runtime knowing the provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ReferenceDeclaration>,
    /// What a payment connector pays with, so a purchase offers it as a
    /// method only for a quote it can pay (S8, N10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<ConnectorPayment>,
}

impl ConnectorManifest {
    /// Whether a placement implements an operation. The local placement runs
    /// any implementation, and an operation with none is unavailable there.
    /// The hosted placement runs an `http` implementation through the
    /// provider gateway, and one the connector leaves to the host through a
    /// private driver, never a process, an MCP tool or the runtime's own
    /// capabilities; under delegated auth only an operation naming the
    /// provider's scopes it needs can run, since the grant asks for them.
    #[must_use]
    pub fn implements(&self, operation: &ConnectorOperation, placement: Placement) -> bool {
        match placement {
            Placement::Local => operation.implementation.is_some(),
            Placement::Hosted => self.placements.hosted.as_ref().is_some_and(|hosted| {
                matches!(
                    operation.implementation,
                    None | Some(Implementation::Http(_))
                ) && (hosted.auth != HostedAuth::Delegated || !operation.provider_scopes.is_empty())
            }),
        }
    }

    /// The declared requests the hosted placement sends through the provider
    /// gateway, when it runs the operation and the implementation is `http`.
    #[must_use]
    pub fn hosted_http<'a>(
        &self,
        operation: &'a ConnectorOperation,
    ) -> Option<&'a HttpImplementation> {
        match &operation.implementation {
            Some(Implementation::Http(http)) if self.implements(operation, Placement::Hosted) => {
                Some(http)
            }
            _ => None,
        }
    }
}

/// The settlement families a connector can pay. Family names are extension
/// identifiers, not a fixed list of first-party processors or protocols.
pub type ConnectorPayment = BTreeMap<crate::SettlementFamily, PaymentCompatibility>;

/// Constraints on a candidate payment method. These declarations filter
/// discovery; the selected operation still admits the exact payment and account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PaymentCompatibility {
    /// Protocol payment method identifiers, such as `stripe` or `tempo` for
    /// MPP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_methods: Option<NonEmptyVec<NonEmptyString>>,
    /// Protocol network identifiers, such as the CAIP-2 `eip155:8453`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub networks: Option<NonEmptyVec<NonEmptyString>>,
    /// Quoted currencies; an omitted restriction leaves validation to the
    /// provider's exact operation, rather than inventing an FX conversion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currencies: Option<NonEmptyVec<crate::CurrencyCode>>,
}

impl PaymentCompatibility {
    /// Every declared restriction must match. An unknown term cannot satisfy
    /// a restriction. An omitted restriction is not an execution grant.
    #[must_use]
    pub fn matches(
        &self,
        method: Option<&str>,
        network: Option<&str>,
        currency: Option<&str>,
    ) -> bool {
        self.payment_methods.as_ref().is_none_or(|allowed| {
            method.is_some_and(|value| allowed.iter().any(|entry| entry.as_str() == value))
        }) && self.networks.as_ref().is_none_or(|allowed| {
            network.is_some_and(|value| allowed.iter().any(|entry| entry.as_str() == value))
        }) && self.currencies.as_ref().is_none_or(|allowed| {
            currency.is_some_and(|value| allowed.iter().any(|entry| entry.as_str() == value))
        })
    }
}

/// One kind of thing a connector's locators name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceDeclaration {
    pub kind: crate::ExternalReferenceKind,
    /// A regular expression the whole locator matches.
    pub locator: NonEmptyString,
}

/// How a principal's authority over the provider is held. Every key is an
/// alternative; placements say which ones they accept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorAuth {
    /// A hosted grant brokered by runx Connect. Valid only on the hosted
    /// placement; the OSS contract never describes the brokerage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated: Option<DelegatedAuth>,
    /// Named keys the principal supplies.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub credentials: BTreeMap<CredentialName, CredentialDeclaration>,
    /// Authority held by a local session the runtime can identify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionAuth>,
    /// A public API with no credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub none: Option<NoAuth>,
}

impl ConnectorAuth {
    /// Whether the connector takes no authority at all. Its writes then
    /// stand on the operator's exact approval alone, so every one requires
    /// it.
    pub fn unauthenticated(&self) -> bool {
        self.delegated.is_none() && self.credentials.is_empty() && self.session.is_none()
    }
}

/// A grant runx Connect brokers. Its principal request names whose
/// authorization it holds, so one provider account holds one binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegatedAuth {
    pub principal: PrincipalRequest,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct NoAuth {}

/// The kind of a credential a lease fills. runx Connect holds it per resource
/// for the resource's owner, taken from the seller's provider answer
/// (`OperationLease`), so nobody connects it and nothing local delivers it.
pub const LEASE_CREDENTIAL_KIND: &str = "lease";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialDeclaration {
    /// The credential kind the hosted store checks (`api_key`, `oauth1_user`,
    /// `lease`, ...).
    pub kind: NonEmptyString,
    /// The connect form, or for a lease the fields its take fills.
    pub fields: NonEmptyVec<CredentialField>,
    /// How the local placement delivers the secret to an implementation. A
    /// lease has none: it never leaves runx Connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<CredentialDelivery>,
    /// Hosts the runtime may present the credential to. A local store-only
    /// credential has none. For process-only use this is optional: delivery is
    /// to the bound executor, which owns further presentation. This field does
    /// not claim to confine a trusted process's network access.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audience: Vec<NonEmptyString>,
}

impl CredentialDeclaration {
    /// Whether a lease fills this credential rather than a person.
    #[must_use]
    pub fn is_lease(&self) -> bool {
        self.kind.as_str() == LEASE_CREDENTIAL_KIND
    }

    /// The variable the local placement delivers the credential in; none
    /// for a lease, which runx Connect alone presents.
    #[must_use]
    pub fn env(&self) -> Option<&EnvName> {
        self.delivery.as_ref().map(|delivery| &delivery.env)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialField {
    pub name: CredentialName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<NonEmptyString>,
    #[serde(
        default = "default_true",
        skip_serializing_if = "crate::schema::is_true"
    )]
    pub secret: bool,
    #[serde(
        default = "default_true",
        skip_serializing_if = "crate::schema::is_true"
    )]
    pub required: bool,
    /// Prefixes every value of this secret starts with, such as a provider's
    /// key prefix. Text that contains one carries the secret, and runx
    /// refuses to write it into a skill package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefixes: Vec<NonEmptyString>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialDelivery {
    pub env: EnvName,
}

/// A local session: authority the person already holds on this machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionAuth {
    /// A token printed by a command-line tool the person is signed into, such
    /// as `gh auth token`. The runtime runs the command when an instance is
    /// confirmed and before each call, never stores the token, and delivers
    /// it as the named credential.
    Command {
        /// The program, found on `PATH` and run without a shell, and its
        /// arguments.
        command: NonEmptyVec<NonEmptyString>,
        /// The one-field credential the token stands in for.
        credential: CredentialName,
        /// The request that names whose token it is.
        principal: PrincipalRequest,
    },
    /// A CLI that keeps its own credential and refresh lifecycle. The command
    /// returns only public account JSON; process implementations use that same
    /// CLI and must enforce the pinned principal at credential use.
    ManagedCommand {
        command: NonEmptyVec<NonEmptyString>,
        id: NonEmptyString,
        login: NonEmptyString,
    },
    /// A signed-in browser profile. Its installed tool receives
    /// `operation: session` and the instance parameters and returns the
    /// rendered account as `{id, login}`. No session secret is exported.
    HostBrowser { process: ProcessImplementation },
}

impl SessionAuth {
    #[must_use]
    pub fn kind(&self) -> SessionKind {
        match self {
            Self::Command { .. } => SessionKind::Command,
            Self::ManagedCommand { .. } => SessionKind::ManagedCommand,
            Self::HostBrowser { .. } => SessionKind::HostBrowser,
        }
    }
}

/// The request that names whose authority a session or a delegated grant
/// holds, sent with that authority as a bearer credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRequest {
    /// An `https` URL the authority may be sent to: for a session, a host in
    /// its credential's audience; for a delegated grant, a host its hosted
    /// operations reach.
    pub url: NonEmptyString,
    /// Where the answer carries the account's stable id.
    pub id: NonEmptyString,
    /// Where the answer carries the account's name.
    pub login: NonEmptyString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Command,
    ManagedCommand,
    HostBrowser,
}

impl SessionKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::ManagedCommand => "managed_command",
            Self::HostBrowser => "host_browser",
        }
    }
}

/// The placements a connector supports. A connector instance pins exactly one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorPlacements {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted: Option<HostedPlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalPlacement>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, RunxSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Hosted,
    Local,
}

impl Placement {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Local => "local",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct HostedPlacement {
    pub auth: HostedAuth,
    /// The stored credential used under credential custody.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialName>,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub callers: HostedCallers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostedAuth {
    Delegated,
    Credential,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostedCallers {
    #[default]
    Public,
    /// Only the hosted runtime may call; the public route refuses.
    Runtime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalPlacement {
    pub auth: NonEmptyVec<LocalAuth>,
    /// Required when any operation uses an `mcp` implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_server: Option<McpServer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum LocalAuth {
    Credentials,
    Session,
    None,
}

/// The MCP server a connector's local `mcp` implementations call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpServer {
    StreamableHttp {
        url: NonEmptyString,
        /// How each credential authenticates the session, by credential
        /// name: a bearer token. With none, the session carries no
        /// credential.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        auth: BTreeMap<CredentialName, HttpAuth>,
        /// Headers every request of a session carries, rendered from the
        /// call (a tenant or brand the server selects by header). A header
        /// whose argument the call does not carry is not sent. Calls that
        /// render different headers run on different sessions.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, Template>,
    },
    /// A server the runtime starts as a declared trusted host process and
    /// speaks to over stdio. `command` is found on the child's `PATH`. The
    /// instance's credential reaches it in the variable its declaration
    /// names, as a process implementation's does.
    Stdio {
        command: NonEmptyString,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<NonEmptyString>,
    },
}

/// The authority verbs an operation may declare. `read` makes the operation a
/// read; every other verb makes it a mutation. What kind of write it is, a
/// comment or a merge, is its `scopes`' to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationVerb {
    Read,
    Write,
    Execute,
    Revoke,
}

impl OperationVerb {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Execute => "execute",
            Self::Revoke => "revoke",
        }
    }

    #[must_use]
    pub fn access(self) -> OperationAccess {
        if self == Self::Read {
            OperationAccess::Read
        } else {
            OperationAccess::Mutate
        }
    }

    #[must_use]
    pub fn authority_verb(self) -> AuthorityVerb {
        match self {
            Self::Read => AuthorityVerb::Read,
            Self::Write => AuthorityVerb::Write,
            Self::Execute => AuthorityVerb::Execute,
            Self::Revoke => AuthorityVerb::Revoke,
        }
    }
}

/// Read versus mutate, derived from the verb. The wire values are what the
/// hosted route's `access` field and the provider operation packet carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationAccess {
    Read,
    Mutate,
}

impl OperationAccess {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mutate => "mutate",
        }
    }
}

/// One named action on a connector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorOperation {
    pub description: NonEmptyString,
    pub verb: OperationVerb,
    /// Grant scopes, all required.
    pub scopes: NonEmptyVec<NonEmptyString>,
    /// Named credentials that may authorize this operation (any of).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credentials: Vec<CredentialName>,
    pub target: OperationTarget,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub approval: ApprovalFloor,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub idempotency_key_format: IdempotencyKeyFormat,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub guarantees: OperationGuarantees,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub amount: AmountRule,
    /// What the provider charges per call; absent when no charge is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<OperationCost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub result_fields: Vec<NonEmptyString>,
    /// What a successful write produced, as the reference its act's receipt
    /// carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<OperationEffect>,
    /// What the call does to a lease: takes it from the provider's answer,
    /// renews it, or releases it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<OperationLease>,
    /// The provider's own permissions, needed under delegated auth.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_scopes: Vec<NonEmptyString>,
    /// How the local placement runs it. The hosted placement runs an `http`
    /// implementation through the provider gateway, and an operation with no
    /// implementation through a private driver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<Implementation>,
}

impl ConnectorOperation {
    /// The connector declares metered usage without a monetary quote.
    #[must_use]
    pub fn is_unpriced_metered(&self) -> bool {
        self.cost
            .as_ref()
            .is_some_and(|cost| cost.usd_micro.is_none())
    }

    /// The connector declares a provider charge, with or without a USD quote.
    /// A charge can land when its answer never arrives, so neither read access
    /// nor state idempotency makes a metered call safe to send again. A zero
    /// quote declares no charge. An operation with no declaration has no known
    /// charge: it recovers as its verb and guarantees say, and only a money
    /// limit refuses it, for want of a quote.
    #[must_use]
    pub fn is_metered(&self) -> bool {
        self.cost
            .as_ref()
            .is_some_and(|cost| cost.usd_micro != Some(0))
    }

    #[must_use]
    pub fn access(&self) -> OperationAccess {
        self.verb.access()
    }

    /// How a sealed call stands: a write the provider cannot read back is
    /// acknowledged; every other call is confirmed by its answer or readback.
    #[must_use]
    pub fn finality(&self) -> &'static str {
        if self.access() == OperationAccess::Mutate
            && self.guarantees.readback == ReadbackGuarantee::Unavailable
        {
            "acknowledged"
        } else {
            "confirmed"
        }
    }

    /// Whether the operation is a write sent once per item of an input array.
    /// Its acknowledgement is then the list of each item's outcome, and only
    /// an `applied` outcome carries the provider's answer.
    #[must_use]
    pub fn repeated_write(&self) -> bool {
        self.access() == OperationAccess::Mutate
            && matches!(
                &self.implementation,
                Some(Implementation::Http(http))
                    if matches!(http.requests.as_slice(), [request] if request.each.is_some())
            )
    }
}

/// What a write produced: one reference, or one per element of a result
/// array. The locator is rendered from the call's arguments, the instance's
/// parameters and the call's result (`$result`); under `each` it reads the
/// element as `$item`. A repeated write's result array is its items'
/// outcomes: an effect is rendered for each applied item, whose `$item` is
/// the provider's answer, and an item not applied produced nothing. Where the
/// connector declares `references`, the kind is one of them and every
/// locator matches its pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationEffect {
    pub kind: crate::ExternalReferenceKind,
    pub locator: Template,
    /// A `$result` pointer to the array the effect repeats over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<Template>,
}

/// What a hosted call does to a lease (S10, N11): a credential the provider
/// issues for one resource a call creates, held by runx Connect for the
/// resource's owner. Connect applies it at the provider's answer, so a leased
/// value never reaches the kernel, which sees only the lease's evidence (its
/// target, expiry and the value's digest). A lease is keyed by connector,
/// credential and target, so a seller leases resources only in its own
/// connector. Each action carries only what it needs: a take names the new
/// resource, and a renew or a release acts on the operation's own target.
#[derive(Clone, Debug, PartialEq, Serialize, RunxSchema)]
#[serde(untagged)]
pub enum OperationLease {
    /// This call creates the resource and takes its credential.
    Take(LeaseTake),
    /// This call moves the lease's expiry.
    Renew(LeaseRenew),
    /// This call ends the lease: the resource is gone.
    Release(LeaseRelease),
}

/// A take: the `kind: lease` credential, the new resource's target rendered
/// from the call's public result, when it ends, and each of the credential's
/// fields filled from the provider's raw answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseTake {
    pub take: CredentialName,
    pub target: Template,
    /// When the resource ends, one `$result` binding.
    pub expires_at: Template,
    /// Each field of the credential and the one `$response` binding that
    /// fills it.
    pub fields: BTreeMap<CredentialName, Template>,
}

/// A renew: the leased credential and its new expiry, one `$result` binding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseRenew {
    pub renew: CredentialName,
    pub expires_at: Template,
}

/// A release: the leased credential, whose resource is gone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseRelease {
    pub release: CredentialName,
}

impl<'de> Deserialize<'de> for OperationLease {
    /// A lease names exactly one action, so the error names what's wrong with
    /// that action's keys rather than that no shape matched.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let value = crate::JsonValue::deserialize(deserializer)?;
        let crate::JsonValue::Object(map) = &value else {
            return Err(D::Error::custom(
                "a lease is a map naming one of take, renew or release",
            ));
        };
        let actions = [LeaseAction::Take, LeaseAction::Renew, LeaseAction::Release]
            .into_iter()
            .filter(|action| map.contains_key(action.as_str()))
            .collect::<Vec<_>>();
        let lease = match actions.as_slice() {
            [LeaseAction::Take] => value.deserialize_into().map(Self::Take),
            [LeaseAction::Renew] => value.deserialize_into().map(Self::Renew),
            [LeaseAction::Release] => value.deserialize_into().map(Self::Release),
            _ => {
                return Err(D::Error::custom(
                    "a lease names exactly one of take, renew or release",
                ));
            }
        };
        lease.map_err(|error| D::Error::custom(format!("lease: {error}")))
    }
}

impl OperationLease {
    #[must_use]
    pub fn action(&self) -> LeaseAction {
        match self {
            Self::Take(_) => LeaseAction::Take,
            Self::Renew(_) => LeaseAction::Renew,
            Self::Release(_) => LeaseAction::Release,
        }
    }

    /// The `kind: lease` credential the action applies to.
    #[must_use]
    pub fn credential(&self) -> &CredentialName {
        match self {
            Self::Take(take) => &take.take,
            Self::Renew(renew) => &renew.renew,
            Self::Release(release) => &release.release,
        }
    }

    /// The leased resource: a take's own, rendered from the call's result,
    /// and otherwise the operation's target, rendered from its arguments.
    #[must_use]
    pub fn target<'a>(&'a self, operation: &'a OperationTarget) -> &'a Template {
        match self {
            Self::Take(take) => &take.target,
            Self::Renew(_) | Self::Release(_) => &operation.template,
        }
    }

    /// When the resource ends: a take and a renew say, a release doesn't.
    #[must_use]
    pub fn expires_at(&self) -> Option<&Template> {
        match self {
            Self::Take(take) => Some(&take.expires_at),
            Self::Renew(renew) => Some(&renew.expires_at),
            Self::Release(_) => None,
        }
    }
}

/// The key a lease's evidence stands under in a provider answer.
pub const LEASE_EVIDENCE_KEY: &str = "$lease";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaseAction {
    /// Take a new resource's credential for the buyer of the purchase whose
    /// run made the call.
    Take,
    /// Move a leased resource's expiry.
    Renew,
    /// End a lease: the resource is gone.
    Release,
}

impl LeaseAction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Take => "take",
            Self::Renew => "renew",
            Self::Release => "release",
        }
    }
}

/// The target an operation acts on, rendered from the call's arguments and
/// the instance's parameters. The rendered value must equal the admitted
/// target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationTarget {
    pub template: Template,
    /// A regular expression the rendered target must match whole, for a
    /// connector whose targets have a shape authority compares, such as a
    /// conversation locator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<NonEmptyString>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalFloor {
    #[default]
    None,
    /// Every call needs one-shot approval of its exact intent.
    Required,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyKeyFormat {
    /// `runx:<plan digest>`.
    #[default]
    Runx,
    /// A UUID derived from the plan digest.
    Uuid,
}

/// What the provider guarantees for this operation. Shared mechanics never
/// assume more, and a use never tolerates more.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationGuarantees {
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub provider_idempotency: ProviderIdempotency,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub readback: ReadbackGuarantee,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderIdempotency {
    /// Only runx's own records prevent repeats.
    #[default]
    None,
    /// The provider deduplicates the whole call, including billed usage,
    /// by the idempotency key that its implementation sends.
    Enforced,
    /// The operation sets a state, so sending it again changes nothing.
    Inherent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadbackGuarantee {
    /// The operation's declared readbacks confirm each write.
    #[default]
    Confirmed,
    /// The provider offers no read of the effect, as with a webhook: its
    /// acknowledgement is the only evidence, and the call seals as
    /// acknowledged, never confirmed.
    Unavailable,
}

/// What one operation does, written as a single-key map: `{http: ...}`,
/// `{mcp: ...}`, `{process: ...}`, `{web: {}}`, `{store: {}}` or
/// `{artifact: {}}`. The local placement runs every kind; the hosted
/// placement runs `http`.
#[derive(Clone, Debug, PartialEq, Serialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum Implementation {
    Http(HttpImplementation),
    Mcp(McpImplementation),
    Process(ProcessImplementation),
    Web(WebImplementation),
    Store(StoreImplementation),
    Artifact(ArtifactImplementation),
}

single_key_enum!(Implementation, "an implementation", {
    "http" => Http,
    "mcp" => Mcp,
    "process" => Process,
    "web" => Web,
    "store" => Store,
    "artifact" => Artifact,
});

impl Implementation {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Http(_) => "http",
            Self::Mcp(_) => "mcp",
            Self::Process(_) => "process",
            Self::Web(_) => "web",
            Self::Store(_) => "store",
            Self::Artifact(_) => "artifact",
        }
    }

    /// Whether the operation writes only storage runx holds for the caller:
    /// the store the operator configured, or the artifact space of the runx
    /// account the runtime is signed in as. Such a write needs no provider
    /// credential.
    #[must_use]
    pub fn writes_runx_storage(&self) -> bool {
        matches!(self, Self::Store(_) | Self::Artifact(_))
    }
}

/// The runtime's guarded public web read: one URL fetched with GET, redirects
/// admitted against the call's host allowlist, private networks refused, the
/// body bounded and extracted as text, metadata or links.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct WebImplementation {}

/// The runtime's event store: append-only streams with an expected version
/// and an idempotency key per append, a rolling projection per stream, and
/// stream heads by latest event. The instance's `url` parameter chooses
/// SQLite, Postgres or Redis; without one it is the workspace store.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreImplementation {}

/// The runtime's artifact space: one bounded value stored under the runx
/// account the runtime is signed in as, keyed by the call's idempotency key
/// and confirmed by reading its reference back.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactImplementation {}

/// Whether an allowlist entry admits `host`: an entry is a host, or
/// `*.domain` for exactly one label under that domain.
#[must_use]
pub fn host_allowed(allowed: &str, host: &str) -> bool {
    match allowed.strip_prefix("*.") {
        Some(domain) => host
            .strip_suffix(domain)
            .and_then(|label| label.strip_suffix('.'))
            .is_some_and(|label| !label.is_empty() && !label.contains('.')),
        None => allowed == host,
    }
}

/// An HTTP exchange the native runtime performs. Bodies are JSON; the
/// transport carries text, so no byte or multipart body is expressible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpImplementation {
    pub allowed_hosts: NonEmptyVec<NonEmptyString>,
    /// How each credential the operation accepts authenticates a request,
    /// by credential name; the instance's credential picks its entry. With
    /// none, requests carry no credential.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub auth: BTreeMap<CredentialName, HttpAuth>,
    pub requests: NonEmptyVec<HttpRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<HttpReadback>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readbacks: Vec<HttpReadback>,
    /// How an answer is decoded; JSON unless declared.
    #[serde(default = "default_decode")]
    pub decode: Vec<DecodeStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id_path: Option<NonEmptyString>,
    /// Where each result field comes from. An operation with one result
    /// field takes the whole decoded answer unless this says otherwise;
    /// validation writes that default in.
    #[serde(default)]
    pub result_paths: BTreeMap<String, NonEmptyString>,
    /// How the provider says when a rate limit lifts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
}

/// How one credential authenticates a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HttpAuth {
    /// `Authorization: Bearer <credential field>`.
    Bearer {
        #[serde(default = "default_token_field")]
        field: CredentialName,
    },
    /// A declared credential field as the complete value of a fixed header.
    /// Unlike a request template, this is rendered only at secret delivery.
    Header {
        name: NonEmptyString,
        field: CredentialName,
    },
    /// Exchange app credentials at a declared OAuth 2 token endpoint using
    /// client_secret_basic, then present its short-lived bearer token.
    /// Neither the credentials nor the token enter request templates/results.
    Oauth2ClientCredentials {
        token_url: Template,
        client_id: CredentialName,
        client_secret: CredentialName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<NonEmptyString>,
    },
    /// OAuth 1.0a user context, signed with the credential's
    /// `consumer_key`, `consumer_secret`, `access_token` and `access_secret`.
    Oauth1,
    /// A declared credential field as the request's complete URL, for a
    /// provider whose secret is the endpoint itself, such as a webhook. The
    /// request's declared URL is the prefix the credential's URL must start
    /// with.
    Url { field: CredentialName },
    /// A declared credential field placed in the request's JSON body at
    /// `pointer` (RFC 6901), for a provider that reads its key from the body,
    /// such as IndexNow. The body's literal object holds the pointer's parent
    /// and never sets it, and no input names it.
    Body {
        field: CredentialName,
        pointer: NonEmptyString,
    },
}

impl HttpAuth {
    /// The fields of the credential this presents.
    #[must_use]
    pub fn fields(&self) -> Vec<&str> {
        match self {
            Self::Bearer { field }
            | Self::Header { field, .. }
            | Self::Url { field }
            | Self::Body { field, .. } => {
                vec![field.as_str()]
            }
            Self::Oauth1 => OAUTH1_FIELDS.to_vec(),
            Self::Oauth2ClientCredentials {
                client_id,
                client_secret,
                ..
            } => {
                vec![client_id.as_str(), client_secret.as_str()]
            }
        }
    }

    /// The header the transport owns for this presentation; a credential
    /// presented as the URL or in the body owns none.
    #[must_use]
    pub fn header_name(&self) -> Option<&str> {
        match self {
            Self::Header { name, .. } => Some(name.as_str()),
            Self::Bearer { .. } | Self::Oauth1 | Self::Oauth2ClientCredentials { .. } => {
                Some("authorization")
            }
            Self::Url { .. } | Self::Body { .. } => None,
        }
    }

    /// The body pointer's segments, unescaped, when the credential is
    /// presented in the body and its pointer is well formed.
    #[must_use]
    pub fn body_pointer(&self) -> Option<Vec<String>> {
        let Self::Body { pointer, .. } = self else {
            return None;
        };
        json_pointer_segments(pointer.as_str())
    }
}

/// An RFC 6901 pointer's segments, unescaped: `/a/b~1c` is `["a", "b/c"]`.
/// None for the empty pointer, an empty segment or a stray `~`.
#[must_use]
pub fn json_pointer_segments(pointer: &str) -> Option<Vec<String>> {
    let rest = pointer.strip_prefix('/')?;
    rest.split('/')
        .map(|segment| {
            if segment.is_empty() {
                return None;
            }
            let mut unescaped = String::with_capacity(segment.len());
            let mut characters = segment.chars();
            while let Some(character) = characters.next() {
                if character != '~' {
                    unescaped.push(character);
                    continue;
                }
                match characters.next() {
                    Some('0') => unescaped.push('~'),
                    Some('1') => unescaped.push('/'),
                    _ => return None,
                }
            }
            Some(unescaped)
        })
        .collect()
}

/// The credential fields an OAuth 1.0a signature uses.
pub const OAUTH1_FIELDS: [&str; 4] = [
    "consumer_key",
    "consumer_secret",
    "access_token",
    "access_secret",
];

/// How a provider says when a rate limit lifts. A rate limited request (an
/// HTTP 429) stops a repeated write, and the outcome carries the time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    /// The response header holding the Unix second the limit lifts at.
    pub reset_header: NonEmptyString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    /// Names the request, unique within the exchange, so a later request
    /// reads its answer as `{"$response": "<id>.<path>"}`. A lone request
    /// needs no name. Readbacks address what a write returned as
    /// `{"$response": "write.<path>"}`; `write` is reserved for that.
    #[serde(default = "default_request_id")]
    pub id: NonEmptyString,
    /// Send this request once per element of the array this renders to,
    /// binding the element as `{"$item": ...}`. A read decodes the answers
    /// as an array in the same order; a write is a repeated write, one
    /// outcome per element. Bounded by the input schema's `maxItems`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<Template>,
    pub http_method: HttpMethod,
    pub url: Template,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, Template>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, Template>,
    /// The body template; JSON unless `body_encoding` declares a form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Template>,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub body_encoding: HttpBodyEncoding,
    /// A read whose answer comes in pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<HttpPages>,
}

/// The wire representation of a request's body. Forms render a flat object
/// of string, number and boolean values; nested form names are literal keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum HttpBodyEncoding {
    #[default]
    Json,
    Form,
}

impl HttpRequest {
    /// Every template the request renders.
    pub fn templates(&self) -> impl Iterator<Item = &Template> {
        std::iter::once(&self.url)
            .chain(self.query.values())
            .chain(self.headers.values())
            .chain(self.body.iter())
            .chain(self.pages.iter().filter_map(|pages| pages.limit.as_ref()))
    }

    /// The ids of the earlier requests whose answers it renders from.
    #[must_use]
    pub fn responses(&self) -> BTreeSet<&str> {
        self.templates().flat_map(Template::responses).collect()
    }
}

/// A read answered in pages. The request is sent again with the cursor each
/// page names until a page names none, a page holds no items, or the items
/// gathered reach `limit`. The pages decode to `{items, next}`: every page's
/// items in order, and the cursor a further read would start from, or null
/// once the read reached the end. Repeated requests additionally return
/// `started` and `complete` for each item: an exhausted shared budget can leave
/// later items unstarted, which must never be interpreted as no matches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpPages {
    /// Where a decoded page holds its items.
    pub items: NonEmptyString,
    /// How many items the caller asked for. Absent, or rendered from an
    /// argument the call does not carry, every page is read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Template>,
    /// Where the next page comes from.
    pub next: HttpPageCursor,
}

/// Where a page names the next page's cursor, and how the next request
/// carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpPageCursor {
    /// Where a decoded page names the next cursor.
    pub cursor: NonEmptyString,
    /// Query parameter name, or an object-member JSON pointer when `in` is body.
    pub param: NonEmptyString,
    /// Cursor placement. Body placement retains the cursor's JSON type.
    #[serde(
        rename = "in",
        default,
        skip_serializing_if = "crate::schema::is_default"
    )]
    pub location: HttpPageCursorLocation,
}

/// The request location of a provider's page cursor. Body cursors retain the
/// provider's scalar JSON type; query cursors are serialized as URL parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum HttpPageCursorLocation {
    #[default]
    Query,
    Body,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpReadback {
    pub request: HttpRequest,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<ReadbackMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode: Option<Vec<DecodeStep>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DecodeStep {
    AutoJson,
    Text,
    Status,
    Field {
        path: NonEmptyString,
    },
    ParseJson,
    /// Keep only this bounded evidence before acknowledgement custody. Each
    /// result member reads one path from the decoded provider response.
    Project {
        fields: BTreeMap<String, NonEmptyString>,
    },
    RejectTrue {
        path: NonEmptyString,
    },
    RejectPresent {
        path: NonEmptyString,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadbackMatch {
    pub expected: ReadbackExpected,
    pub observed: NonEmptyString,
}

/// What a confirming read's field must equal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadbackExpected {
    /// A field of the write's acknowledgement, which must be present.
    Acknowledgement {
        path: NonEmptyString,
    },
    /// A field of the call's own arguments, so the write is checked against
    /// what was approved. A match whose argument the call does not carry is
    /// not checked: nothing was asked for that field.
    Argument {
        path: NonEmptyString,
    },
    /// A field of the item a repeated write was sent for, checked the same
    /// way as an argument.
    Item {
        path: NonEmptyString,
    },
    Literal {
        value: crate::JsonValue,
    },
    /// An exact expected value rendered from approved arguments and the
    /// projected acknowledgement (`$response: write.json.<path>`). Useful
    /// for provider resource locators and structured relationship evidence.
    Template {
        value: Template,
    },
}

/// A tool on the connector's MCP server. The tool is pinned by its served
/// definition; the manifest's verb, not the tool's annotations, is enforced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct McpImplementation {
    pub tool: NonEmptyString,
    /// Rendered from the call input into the tool arguments. A multiplexed
    /// tool's discriminating argument is a literal here, never an input.
    pub arguments: Template,
    /// sha256 over the served tool's name, description, input schema and
    /// annotations when the manifest was authored. A tool served with another
    /// definition is refused.
    pub tool_digest: Digest,
    /// How a write is confirmed: an independent call to a read tool, checked
    /// against what the write returned. Every write has one; a read has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readback: Option<McpReadback>,
}

/// The read tool call that confirms an MCP write. Its arguments may bind the
/// write's result as `{"$response": "write.<path>"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct McpReadback {
    pub tool: NonEmptyString,
    pub arguments: Template,
    pub tool_digest: Digest,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<ReadbackMatch>,
}

/// A tool package that runs as this operation's implementation, receiving the
/// operation's credential. It is a declared trusted host process.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessImplementation {
    pub tool: NonEmptyString,
    /// What the runtime grants the tool, such as `fs:write`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<NonEmptyString>,
    /// How a write is confirmed: the tool is run again with `phase:
    /// readback` and the acknowledgement, and what it observes must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readback: Option<ProcessReadback>,
    /// The bounded public answer to return after confirmation. A durable
    /// acknowledgement can remain just an opaque provider reference.
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub result_from: ProcessResultSource,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessResultSource {
    #[default]
    Acknowledgement,
    Readback,
}

/// What a process tool's readback must match.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessReadback {
    pub matches: NonEmptyVec<ReadbackMatch>,
}

fn default_true() -> bool {
    true
}

fn default_token_field() -> CredentialName {
    CredentialName::token()
}

fn default_request_id() -> NonEmptyString {
    NonEmptyString::from("request")
}

fn default_decode() -> Vec<DecodeStep> {
    vec![DecodeStep::AutoJson]
}

#[cfg(test)]
mod tests;
