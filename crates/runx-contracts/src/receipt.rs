//! Governance receipt contracts: the flat `runx.receipt.v1` shape, seals,
//! lineage, and signatures.
//!
//! One flat artifact, each top-level key answering one question: integrity
//! (envelope), what ran and the digest of its outcome (subject), what allowed
//! it (authority), the choices made (`decisions[]`), what was done
//! (`acts[]`), the outcome (seal), and graph lineage. A receipt holds only
//! what its issuer observed or was given, each value once, as typed facts:
//! the runtime writes no prose. The acts (intent, the caller's reason, the
//! checks bound to them) are inline; the bulky per-act execution I/O (the
//! agent-context envelope) is referenced via `acts[].context_ref` and
//! `artifact_refs`. Verification is computed at read time, never part of the
//! signed body.
//!
//! The wire form is exact: every array is written even when empty, an absent
//! optional value is omitted (never `null`), and a reader refuses any other
//! form, so every verifier hashes the same bytes.
use serde::{Deserialize, Serialize};

use crate::connector::Digest;
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{
    ActForm, AuthorityDerivation, AuthorityTerm, ClosureDisposition, CriterionBinding, Decision,
    Intent, JsonObject, Reference,
};

/// Logical schema name for the governance receipt.
pub const RECEIPT_SCHEMA: &str = "runx.receipt.v1";

/// The canonicalization byte contract a receipt's id and digest commit
/// under. v1 has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ReceiptCanonicalization {
    #[serde(rename = "runx.receipt.c14n.v1")]
    C14nV1,
}

impl ReceiptCanonicalization {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::C14nV1 => "runx.receipt.c14n.v1",
        }
    }
}

/// The canonicalization's wire name, for contracts that name it.
pub const RECEIPT_CANONICALIZATION: &str = ReceiptCanonicalization::C14nV1.as_str();

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ReceiptSchema {
    #[serde(rename = "runx.receipt.v1")]
    V1,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    /// Open subject kind identifier (e.g. `receipt_subject_kind::SKILL`). Any
    /// non-empty string is accepted; new subject categories do not need a
    /// contract edit.
    pub kind: NonEmptyString,
    /// What ran.
    #[serde(rename = "ref")]
    pub reference: Reference,
    /// The digest of the subject's outcome: whether it succeeded, the
    /// outputs it declared, and its direct children's receipt ids. The
    /// receipt commits to that outcome; the produced content beyond its
    /// declared outputs is not part of it.
    pub outcome: Digest,
}

/// What the runtime enforced around the subject.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptEnforcement {
    /// Where the subject ran, when the runtime observed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_boundary: Option<crate::ExecutionBoundaryObservation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptAuthority {
    pub actor_ref: Reference,
    pub grant_refs: Vec<Reference>,
    pub scope_refs: Vec<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mandate_ref: Option<Reference>,
    pub terms: Vec<AuthorityTerm>,
    /// How each derived execution term in `terms` was proven.
    pub derivations: Vec<AuthorityDerivation>,
    pub enforcement: ReceiptEnforcement,
}

/// What was done, inline. The act's semantic core (intent, the caller's
/// reason, the checks bound to it) stays in the signed body; its outcome is
/// the receipt's seal. Only the bulky execution I/O (the agent-context
/// envelope: instructions/inputs/output and tool calls) is referenced via
/// `context_ref` and hydrated by projections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptAct {
    pub id: NonEmptyString,
    pub form: ActForm,
    pub intent: Intent,
    /// What the caller told a reader about this particular call, when it
    /// said anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<NonEmptyString>,
    pub criterion_bindings: Vec<CriterionBinding>,
    pub target_refs: Vec<Reference>,
    pub artifact_refs: Vec<Reference>,
    /// The agent-context envelope (instructions/inputs/output/tool-calls) is
    /// referenced here and hydrated by the inspection projections.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_ref: Option<Reference>,
}

/// Exactly one seal. `deferred` expresses a suspended (waiting/delegated) run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Seal {
    pub disposition: ClosureDisposition,
    pub reason_code: NonEmptyString,
    pub closed_at: IsoDateTime,
    pub criteria: Vec<CriterionBinding>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    /// The receipt of the act this one follows, when its caller named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<Reference>,
    /// The receipts of the subject's direct children.
    pub children: Vec<Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptIssuerType {
    Local,
    Hosted,
    Ci,
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
/// `decisions[]` (each choice and the act it selects) and `acts[]` (intent,
/// the caller's reason, the checks bound to each act) are inline. `metadata` is a
/// runtime-local read aid (the invocation's execution limits, a failure's
/// public projection) and is NOT part of the canonical signed body (the
/// canonicalizer strips it). It is non-authoritative and must never be the
/// source of a trust-bearing identity label. Display identity comes from
/// signed fields: `subject.kind`, `subject.ref`, `issuer` and
/// `authority.actor_ref`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.receipt.v1")]
pub struct Receipt {
    pub schema: ReceiptSchema,
    pub id: NonEmptyString,
    pub created_at: IsoDateTime,
    pub canonicalization: ReceiptCanonicalization,
    pub issuer: ReceiptIssuer,
    pub signature: ReceiptSignature,
    pub digest: NonEmptyString,
    pub subject: Subject,
    pub authority: ReceiptAuthority,
    pub decisions: Vec<Decision>,
    pub acts: Vec<ReceiptAct>,
    pub seal: Seal,
    pub lineage: Lineage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}
