// Module rationale: receipt contracts keep the signed envelope,
// lineage, and effect-finality schemas together for cross-language schema
// generation.
//! Governance receipt contracts: the flat `runx.receipt.v1` shape, seals,
//! fanout sync, lineage, and signatures.
//!
//! One flat artifact, each top-level key answering one question: integrity
//! (envelope), dedup (idempotency), what ran (subject), what allowed it
//! (authority), the inbound triggers (`signals[]`), the reasoning
//! (`decisions[]`), what was done (`acts[]`), the outcome (seal), and graph/resume
//! lineage. The post-run verdict is a `review`/`verification` act in `acts[]`
//! (or a follow-up receipt linked by `lineage`), never a side contract. The
//! reasoning and the full acts (intent, success criteria, criterion
//! bindings) are INLINE: that is simultaneously the proof, the training signal,
//! and the inspection narrative. Only the bulky per-act execution I/O (the
//! agent-context envelope: instructions/inputs/output) is referenced via
//! `acts[].context_ref` + `artifact_refs` and hydrated by projections.
//! Verification is computed at read time, never part of the signed body.
use serde::{Deserialize, Serialize};

use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{
    ActForm, AuthorityAttenuation, AuthorityDerivation, AuthorityTerm, Closure, ClosureDisposition,
    CriterionBinding, Decision, HashAlgorithm, Intent, JsonObject, OfferRevisionRef,
    ParentInvocationBinding, PrincipalReference, Reference, RevisionDetails, Sha256Digest,
    VerificationDetails,
};

/// Logical schema name for the governance receipt.
pub const RECEIPT_SCHEMA: &str = "runx.receipt.v1";

/// Logical schema name reserved for follow-on receipts that record deferred
/// effect finality. A finality receipt is emitted as a new artifact; sealed
/// receipts are never mutated after the fact.
pub const EFFECT_FINALITY_RECEIPT_SCHEMA: &str = "runx.effect_finality_receipt.v1";

/// The canonicalization byte contract this receipt's digest commits under.
pub const RECEIPT_CANONICALIZATION: &str = "runx.receipt.c14n.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ReceiptSchema {
    #[serde(rename = "runx.receipt.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum EffectFinalityReceiptSchema {
    #[serde(rename = "runx.effect_finality_receipt.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectFinalityPhase {
    Provisional,
    InFlight,
    Sealed,
    Failed,
    Reversed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.effect_finality_receipt.v1")]
pub struct EffectFinalityReceipt {
    pub schema: EffectFinalityReceiptSchema,
    pub id: NonEmptyString,
    pub created_at: IsoDateTime,
    pub family: NonEmptyString,
    pub phase: EffectFinalityPhase,
    pub original_receipt_ref: Reference,
    pub criterion_id: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_ref: Option<Reference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<Reference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub norm_refs: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_depth: Option<u64>,
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub payload: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum FanoutReceiptStrategy {
    All,
    Any,
    Quorum,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum FanoutReceiptDecision {
    Proceed,
    Halt,
    Pause,
    Escalate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct FanoutReceiptSyncPoint {
    pub group_id: NonEmptyString,
    pub strategy: FanoutReceiptStrategy,
    pub decision: FanoutReceiptDecision,
    pub rule_fired: NonEmptyString,
    pub reason: NonEmptyString,
    pub branch_count: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub required_successes: usize,
    #[serde(default)]
    pub branch_receipts: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<JsonObject>,
}

/// Scoped byte commitment; unifies the old hash_commitments + enforcement.std*_hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptCommitmentScope {
    Input,
    Output,
    Stdout,
    Stderr,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptCommitment {
    pub scope: ReceiptCommitmentScope,
    pub algorithm: HashAlgorithm,
    pub value: NonEmptyString,
    pub canonicalization: NonEmptyString,
}

/// Canonical receipt subject kinds. The wire form on `Subject.kind` is an
/// open `NonEmptyString` so receipts emitted by new subject categories (for
/// example, tool build or hosted provider publication) do not require a
/// contract edit.
pub mod receipt_subject_kind {
    /// A single skill invocation.
    pub const SKILL: &str = "skill";
    /// A graph execution composed of multiple acts.
    pub const GRAPH: &str = "graph";
}

/// The input signal for training and inspection: where the run's input came
/// from, a human-readable preview, and a content hash for integrity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptInputContext {
    pub source: NonEmptyString,
    pub preview: String,
    pub value_hash: NonEmptyString,
}

/// What relationship this receipt proves for its subject outcome.
///
/// This class is orthogonal to the observed runner boundary in
/// `ReceiptEnforcement`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptClass {
    Executed,
    Mediated,
}

/// Atomic paid-execution provenance signed into a receipt subject.
///
/// The grouped shape prevents partial vendor/package/input provenance. The
/// paid `input_digest` is authoritative for the purchased business input;
/// generic receipt commitments may describe other representations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptPaidInvocationBinding {
    pub invocation_id: NonEmptyString,
    pub vendor_ref: PrincipalReference,
    pub offer_revision: OfferRevisionRef,
    pub package_digest: Sha256Digest,
    pub input_digest: Sha256Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mediation: Option<crate::PaidInvocationMediation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_binding: Option<ParentInvocationBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    /// Open subject kind identifier (e.g. `receipt_subject_kind::SKILL`). Any
    /// non-empty string is accepted; new subject categories do not need a
    /// contract edit.
    pub kind: NonEmptyString,
    #[serde(rename = "ref")]
    pub reference: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_context: Option<ReceiptInputContext>,
    #[serde(default)]
    pub commitments: Vec<ReceiptCommitment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_invocation: Option<ReceiptPaidInvocationBinding>,
}

/// Signed receipt-to-receipt evidence. Generic execution lineage remains in
/// `Lineage`; marketplace composition uses this typed evidence edge only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptEvidence {
    InnerReceipt {
        receipt_ref: Reference,
        expected: ReceiptPaidInvocationBinding,
    },
}

/// Enforcement profile is hashed; the granted authority stays readable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptEnforcement {
    pub profile_hash: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_boundary: Option<crate::ExecutionBoundaryObservation>,
    #[serde(default)]
    pub redaction_refs: Vec<Reference>,
    #[serde(default)]
    pub setup_refs: Vec<Reference>,
    #[serde(default)]
    pub teardown_refs: Vec<Reference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptAuthority {
    pub actor_ref: Reference,
    #[serde(default)]
    pub grant_refs: Vec<Reference>,
    #[serde(default)]
    pub scope_refs: Vec<Reference>,
    #[serde(default)]
    pub authority_proof_refs: Vec<Reference>,
    pub attenuation: AuthorityAttenuation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mandate_ref: Option<Reference>,
    #[serde(default)]
    pub terms: Vec<AuthorityTerm>,
    /// How each derived execution term in `terms` was proven.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derivations: Vec<AuthorityDerivation>,
    pub enforcement: ReceiptEnforcement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptIdempotency {
    pub intent_key: NonEmptyString,
    pub trigger_fingerprint: NonEmptyString,
    pub content_hash: NonEmptyString,
}

/// Runner provenance for agent acts (drives the trainable-export projection).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunnerProvenance {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
}

/// What was done, rich and inline. The act's semantic core (intent, success
/// criteria, criterion bindings, outcome) stays in the signed body; only the
/// bulky execution I/O (the agent-context envelope: instructions/inputs/output
/// and tool calls) is referenced via `context_ref` and hydrated by projections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptAct {
    pub id: NonEmptyString,
    pub form: ActForm,
    pub intent: Intent,
    pub summary: NonEmptyString,
    #[serde(default)]
    pub criterion_bindings: Vec<CriterionBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<RunnerProvenance>,
    #[serde(default)]
    pub source_refs: Vec<Reference>,
    #[serde(default)]
    pub target_refs: Vec<Reference>,
    #[serde(default)]
    pub artifact_refs: Vec<Reference>,
    /// The agent-context envelope (instructions/inputs/output/tool-calls) is
    /// referenced here and hydrated by the trainable/inspection projections.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_ref: Option<Reference>,
    pub closure: Closure,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<RevisionDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationDetails>,
}

/// Exactly one seal. `deferred` expresses a suspended (waiting/delegated) run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Seal {
    pub disposition: ClosureDisposition,
    pub reason_code: NonEmptyString,
    pub summary: NonEmptyString,
    pub closed_at: IsoDateTime,
    /// The last time the run was observed (advances for `deferred`/`monitor`
    /// runs awaiting a follow-up verdict); equals `closed_at` for terminal seals.
    pub last_observed_at: IsoDateTime,
    #[serde(default)]
    pub criteria: Vec<CriterionBinding>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<Reference>,
    #[serde(default)]
    pub children: Vec<Reference>,
    #[serde(default)]
    pub sync: Vec<FanoutReceiptSyncPoint>,
    // Open resolution request when seal.disposition == "deferred".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_ref: Option<Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptIssuerType {
    Local,
    Hosted,
    Ci,
    Verifier,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptIssuer {
    #[serde(rename = "type")]
    pub issuer_type: ReceiptIssuerType,
    pub kid: NonEmptyString,
    pub public_key_sha256: NonEmptyString,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "PascalCase")]
pub enum SignatureAlgorithm {
    Ed25519,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSignature {
    pub alg: SignatureAlgorithm,
    pub value: NonEmptyString,
}

/// The single signed governance receipt: `runx.receipt.v1`.
///
/// `decisions[]` (the reasoning, with `proposed_intent` + `justification`) and
/// `acts[]` (intent, success criteria, criterion bindings) are inline: the proof
/// and the training signal are the same artifact. `metadata` is a runtime-local
/// read aid (display skill name, source type, actor labels for local projection)
/// and is NOT part of the canonical signed body (the canonicalizer strips it).
/// It is non-authoritative and must never be the source of a trust-bearing
/// identity label. Display identity comes from signed fields:
/// `subject.kind`, `subject.ref`, `issuer`, `authority.actor_ref`, and
/// `acts[].by`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.receipt.v1")]
pub struct Receipt {
    pub schema: ReceiptSchema,
    pub id: NonEmptyString,
    pub created_at: IsoDateTime,
    pub canonicalization: NonEmptyString,
    pub issuer: ReceiptIssuer,
    pub signature: ReceiptSignature,
    pub digest: NonEmptyString,
    pub idempotency: ReceiptIdempotency,
    pub class: ReceiptClass,
    pub subject: Subject,
    pub authority: ReceiptAuthority,
    /// Inbound triggers for this run: `runx:signal:` references whose
    /// authenticity/trust/body live in the signal artifact.
    #[serde(default)]
    pub signals: Vec<Reference>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub acts: Vec<ReceiptAct>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ReceiptEvidence>,
    pub seal: Seal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineage: Option<Lineage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}
