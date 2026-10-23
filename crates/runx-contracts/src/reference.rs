//! Reference contracts: typed references to receipts, acts, and external surfaces.
use serde::{Deserialize, Serialize};

use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};

/// A hosted Runx principal identifier accepted by the canonical owner-reference
/// constructor.
///
/// This is deliberately narrower than a generic principal reference URI. It
/// protects the hosted authentication boundary without constraining external
/// identity schemes carried by [`Reference`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunxPrincipalId(String);

impl RunxPrincipalId {
    pub const MAX_LENGTH: usize = 256;

    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = (1..=Self::MAX_LENGTH).contains(&bytes.len())
            && bytes[0].is_ascii_alphanumeric()
            && bytes[1..].iter().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            });
        valid.then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

/// The kinds of thing outside runx a connector's locators name: what its
/// declared references classify and what its operations produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExternalReferenceKind {
    Thread,
    Comment,
    Event,
    TrackingItem,
    ChangeRequest,
    Repository,
    ExternalUrl,
}

impl From<ExternalReferenceKind> for ReferenceType {
    fn from(kind: ExternalReferenceKind) -> Self {
        match kind {
            ExternalReferenceKind::Thread => Self::Thread,
            ExternalReferenceKind::Comment => Self::Comment,
            ExternalReferenceKind::Event => Self::Event,
            ExternalReferenceKind::TrackingItem => Self::TrackingItem,
            ExternalReferenceKind::ChangeRequest => Self::ChangeRequest,
            ExternalReferenceKind::Repository => Self::Repository,
            ExternalReferenceKind::ExternalUrl => Self::ExternalUrl,
        }
    }
}

/// What a reference points at. External kinds never name a provider: the
/// reference's `provider` and `locator` say where, and connectors declare
/// which of their locators are which kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceType {
    Thread,
    Comment,
    Event,
    TrackingItem,
    ChangeRequest,
    Repository,
    Signal,
    Run,
    Act,
    Receipt,
    Artifact,
    Verification,
    Harness,
    Host,
    Surface,
    Target,
    SkillBinding,
    Principal,
    ScopeAdmission,
    Grant,
    Mandate,
    Credential,
    WebhookDelivery,
    RedactionPolicy,
    ExternalUrl,
    ConnectorInstance,
    ConnectorOperation,
}

impl ReferenceType {
    /// Stable snake_case wire name for this reference type. Matches the serde
    /// representation and the `runx:<name>:<id>` URI segment.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReferenceType::Thread => "thread",
            ReferenceType::Comment => "comment",
            ReferenceType::Event => "event",
            ReferenceType::TrackingItem => "tracking_item",
            ReferenceType::ChangeRequest => "change_request",
            ReferenceType::Repository => "repository",
            ReferenceType::Signal => "signal",
            ReferenceType::Run => "run",
            ReferenceType::Act => "act",
            ReferenceType::Receipt => "receipt",
            ReferenceType::Artifact => "artifact",
            ReferenceType::Verification => "verification",
            ReferenceType::Harness => "harness",
            ReferenceType::Host => "host",
            ReferenceType::Surface => "surface",
            ReferenceType::Target => "target",
            ReferenceType::SkillBinding => "skill_binding",
            ReferenceType::Principal => "principal",
            ReferenceType::ScopeAdmission => "scope_admission",
            ReferenceType::Grant => "grant",
            ReferenceType::Mandate => "mandate",
            ReferenceType::Credential => "credential",
            ReferenceType::WebhookDelivery => "webhook_delivery",
            ReferenceType::RedactionPolicy => "redaction_policy",
            ReferenceType::ExternalUrl => "external_url",
            ReferenceType::ConnectorInstance => "connector_instance",
            ReferenceType::ConnectorOperation => "connector_operation",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProofKind {
    EffectEvidence,
    EffectFinality,
    CredentialResolution,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.reference.v1")]
pub struct Reference {
    #[serde(rename = "type")]
    pub reference_type: ReferenceType,
    pub uri: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locator: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<IsoDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_kind: Option<ProofKind>,
}

impl Reference {
    /// A reference to an explicit URI, with no provider/locator/label/proof.
    pub fn with_uri(reference_type: ReferenceType, uri: impl Into<NonEmptyString>) -> Self {
        Self {
            reference_type,
            uri: uri.into(),
            provider: None,
            locator: None,
            label: None,
            observed_at: None,
            proof_kind: None,
        }
    }

    /// A reference whose URI is the canonical `runx:<type>:<id>` scheme.
    pub fn runx(reference_type: ReferenceType, id: &str) -> Self {
        let uri = format!("runx:{}:{id}", reference_type.as_str());
        Self::with_uri(reference_type, uri)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.reference_link.v1")]
pub struct ReferenceLink {
    pub role: NonEmptyString,
    #[serde(rename = "ref")]
    pub reference: Reference,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ActAddress {
    pub receipt_ref: Reference,
    pub act_id: NonEmptyString,
}
