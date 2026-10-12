//! Authority algebra: terms, capabilities, verbs, attenuation, and effect bounds.
use serde::{Deserialize, Serialize};

use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{JsonNumber, JsonObject, ProofKind, Reference};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityResourceFamily {
    Workspace,
    Filesystem,
    Network,
    Deployment,
    Credential,
    Effect,
    Artifact,
    Harness,
    Publication,
    /// Operations on one connector instance.
    Connector,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityVerb {
    Read,
    Write,
    Comment,
    Review,
    Approve,
    Merge,
    Create,
    Update,
    Delete,
    Execute,
    Verify,
    Estimate,
    Prepare,
    Commit,
    Reverse,
    Publish,
    SpawnChild,
    Revoke,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityCapability {
    FilesystemRead,
    FilesystemWrite,
    NetworkEgress,
    SecretRead,
    ProcessSpawn,
    ProviderMutation,
    PublicPublication,
    ChildHarnessSpawn,
    EffectSingleUseCapability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityEffectCredentialForm {
    SingleUseCapability,
    ExternalSigner,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityEffectLimit {
    pub family: NonEmptyString,
    pub unit: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_per_call_units: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_per_run_units: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_per_period_units: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<NonEmptyString>,
    pub channels: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realm: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preflight_ttl_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_threshold_units: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization_form: Option<AuthorityEffectCredentialForm>,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub preflight_required: bool,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub commitment_required: bool,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub idempotency_required: bool,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub recovery_required: bool,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub receipt_before_success: bool,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub single_use_capability: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityEffectGuardKind {
    ReceiptBeforeSuccess,
    NonReplay,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityEffectGuard {
    pub family: NonEmptyString,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guard_kinds: Vec<AuthorityEffectGuardKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_kinds: Vec<ProofKind>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBounds {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repo_path_globs: Vec<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branch_patterns: Vec<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystem_roots: Vec<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_destinations: Vec<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deployment_environments: Vec<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub token_audiences: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost_units: Option<JsonNumber>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effect_limits: Vec<AuthorityEffectLimit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<AuthorityEffectGuard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_runtime_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fanout: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_child_depth: Option<u32>,
    /// Connector operations, as `<connector>/<operation>` or `<connector>/*`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<NonEmptyString>,
    /// Provider grant scopes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<NonEmptyString>,
    /// Provider targets: exact values, `<prefix>*`, or `*`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uses: Option<AuthorityUseLimit>,
}

/// How many times a term may be used. An execution term is single use.
/// Usage is reserved atomically with each effect attempt, never counted from
/// receipts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityUseLimit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_day: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityConditionPredicate {
    SignalVerified,
    DecisionSelected,
    HostPostureValid,
    ApprovalPresent,
    WithinTimeWindow,
    WithinBudget,
    EffectProofPresent,
    EffectRecoveryAvailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityCondition {
    pub condition_id: NonEmptyString,
    pub predicate: AuthorityConditionPredicate,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<JsonObject>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityApproval {
    pub approval_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<IsoDateTime>,
}

/// How an act's legitimacy names the execution term it acts under.
const TERM_LEGITIMACY_PREFIX: &str = "runx:authority_term:";
const TERM_LEGITIMACY_PATTERN: &str = r"^runx:authority_term:\S+$";
const TERM_LEGITIMACY_MAX_LENGTH: usize = 512;

/// An act's legitimacy: a reference to the execution term it was admitted
/// under, `runx:authority_term:<term id>`, never prose. Every act carries one,
/// and a verifier checks the receipt's authority holds the term it names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TermLegitimacy(String);

impl TermLegitimacy {
    /// The legitimacy of an act admitted under `term_id`; `None` when the id
    /// is empty or holds whitespace.
    #[must_use]
    pub fn for_term(term_id: &str) -> Option<Self> {
        Self::new(format!("{TERM_LEGITIMACY_PREFIX}{term_id}"))
    }

    /// `None` when `value` is not a reference to an execution term.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let valid = value.len() <= TERM_LEGITIMACY_MAX_LENGTH
            && value.strip_prefix(TERM_LEGITIMACY_PREFIX).is_some_and(|id| {
                !id.is_empty() && !id.chars().any(|c| c.is_whitespace() || c.is_control())
            });
        valid.then_some(Self(value))
    }

    /// The execution term this legitimacy names.
    #[must_use]
    pub fn term_id(&self) -> &str {
        &self.0[TERM_LEGITIMACY_PREFIX.len()..]
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TermLegitimacy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TermLegitimacy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value.clone()).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{value:?} is not a reference to an execution term"
            ))
        })
    }
}

impl RunxSchema for TermLegitimacy {
    fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "string",
            "pattern": TERM_LEGITIMACY_PATTERN,
            "maxLength": TERM_LEGITIMACY_MAX_LENGTH,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.authority_term.v1",
    url = "https://schemas.runx.ai/runx/authority/term/v1.json"
)]
pub struct AuthorityTerm {
    pub term_id: NonEmptyString,
    pub principal_ref: Reference,
    pub resource_ref: Reference,
    pub resource_family: AuthorityResourceFamily,
    pub verbs: Vec<AuthorityVerb>,
    pub bounds: AuthorityBounds,
    #[serde(default)]
    pub conditions: Vec<AuthorityCondition>,
    #[serde(default)]
    pub approvals: Vec<AuthorityApproval>,
    #[serde(default)]
    pub capabilities: Vec<AuthorityCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<IsoDateTime>,
    pub issued_by_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthoritySubsetRelation {
    Equal,
    Subset,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthoritySubsetComparison {
    pub child_term_id: NonEmptyString,
    pub parent_term_id: NonEmptyString,
    pub relation: AuthoritySubsetRelation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum AuthoritySubsetResult {
    #[serde(rename = "subset")]
    Subset,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.authority_subset_proof.v1",
    url = "https://schemas.runx.ai/runx/authority/subset-proof/v1.json"
)]
pub struct AuthoritySubsetProof {
    pub parent_authority_ref: Reference,
    pub comparison_algorithm: NonEmptyString,
    pub result: AuthoritySubsetResult,
    pub compared_terms: Vec<AuthoritySubsetComparison>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_ref: Option<Reference>,
    pub checked_at: IsoDateTime,
}

/// The three kinds of evidence admission proves. Each has one job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityEvidenceKind {
    /// What the principal can do at the provider: a grant, credential or
    /// session. Authorizes only the principal acting directly.
    Capability,
    /// What an actor may do for the principal: a mandate, or child authority
    /// attenuated from a parent run. Narrows; never creates.
    Delegation,
    /// A person's confirmation of this exact intent, or of a bounded kind of
    /// intent. Satisfies an approval floor; never authorizes alone.
    Approval,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityEvidence {
    pub kind: AuthorityEvidenceKind,
    pub reference: Reference,
    /// The capability or delegation term, sealed so the subset proofs can be
    /// re-checked offline. Approvals carry no term.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term: Option<AuthorityTerm>,
}

/// How an execution term was derived: the evidence admission proved, ordered
/// capability first, then each delegation link from principal to actor, then
/// any approval; and one subset proof per parent term.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.authority_derivation.v1")]
pub struct AuthorityDerivation {
    pub term_id: NonEmptyString,
    pub intent_digest: NonEmptyString,
    pub evidence: Vec<AuthorityEvidence>,
    pub proofs: Vec<AuthoritySubsetProof>,
    pub derived_at: IsoDateTime,
}
