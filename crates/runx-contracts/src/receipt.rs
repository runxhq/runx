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
    Intent, Reference,
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

/// What a receipt seals: one step's attempt, or the whole graph a run ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Step,
    Graph,
}

impl SubjectKind {
    /// Stable string label, identical to the serde `snake_case` representation.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Graph => "graph",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub kind: SubjectKind,
    /// What ran.
    #[serde(rename = "ref")]
    pub reference: Reference,
    /// The digest of the subject's outcome: how it closed (its seal's
    /// disposition), the outputs it declared, and its direct children's
    /// receipt ids. The receipt commits to that outcome; the produced content
    /// beyond its declared outputs is not part of it.
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
    /// The principal an act was admitted for, when the run was given one.
    /// A receipt with no admitted actor states none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_ref: Option<Reference>,
    /// The capability evidence that admitted the receipt's effects: a runx
    /// Connect grant, or the local connector instance the operator confirmed.
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
    /// The connector operation the act ran, exactly as admission bound it:
    /// its qualified connector, manifest and connector digests, and the
    /// operation, instance and target it was admitted for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<crate::OperationIntent>,
    /// The receipt the caller states this act follows. It is the caller's
    /// statement, recorded as given: runx does not check it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<Reference>,
}

/// Exactly one seal. `deferred` expresses a suspended (waiting/delegated) run.
/// It closes when the receipt is created, so the receipt's `created_at` is
/// its one time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Seal {
    pub disposition: ClosureDisposition,
    /// Why it closed so, where the disposition alone does not say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<SealReason>,
    pub criteria: Vec<CriterionBinding>,
}

/// Why a receipt closed as it did, beyond its disposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SealReason {
    /// The run was refused before any step ran.
    PreflightRefused,
    /// A harness case's inputs do not satisfy the runner's input contract.
    InputContractInvalid,
    /// The graph stopped because one of its steps failed.
    GraphStepFailed,
    /// The graph stopped at a step that could not proceed.
    GraphBlocked,
    /// A call's authority did not admit it.
    AuthorityDenied,
    /// The graph waits on a step's request its host has not yet answered.
    ResolutionPending,
    /// The managed agent's provider request failed.
    ManagedAgentProviderFailed,
    /// The managed agent could not resolve the turn it was given.
    ManagedAgentResolutionFailed,
    /// The managed agent's model answered without progress for every turn
    /// it was allowed.
    ManagedAgentEmptyTurnBudgetExhausted,
    /// A tool the managed agent called failed.
    ManagedAgentToolExecutionFailed,
    /// The managed agent used every round its turn allowed.
    ManagedAgentRoundBudgetExhausted,
}

impl SealReason {
    /// Stable string label, identical to the serde `snake_case` representation.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PreflightRefused => "preflight_refused",
            Self::InputContractInvalid => "input_contract_invalid",
            Self::GraphStepFailed => "graph_step_failed",
            Self::GraphBlocked => "graph_blocked",
            Self::AuthorityDenied => "authority_denied",
            Self::ResolutionPending => "resolution_pending",
            Self::ManagedAgentProviderFailed => "managed_agent_provider_failed",
            Self::ManagedAgentResolutionFailed => "managed_agent_resolution_failed",
            Self::ManagedAgentEmptyTurnBudgetExhausted => {
                "managed_agent_empty_turn_budget_exhausted"
            }
            Self::ManagedAgentToolExecutionFailed => "managed_agent_tool_execution_failed",
            Self::ManagedAgentRoundBudgetExhausted => "managed_agent_round_budget_exhausted",
        }
    }
}

/// What runx committed of a receipt's place among others.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    /// The receipts of the subject's direct children, each committed by its
    /// id and signed-body digest.
    pub children: Vec<Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptIssuerType {
    Local,
    Hosted,
    Ci,
}

impl ReceiptIssuerType {
    /// Stable string label, identical to the serde `snake_case` representation.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Hosted => "hosted",
            Self::Ci => "ci",
        }
    }

    /// The issuer type a label names.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        [Self::Local, Self::Hosted, Self::Ci]
            .into_iter()
            .find(|issuer_type| issuer_type.label() == label)
    }
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
/// the caller's reason, the checks bound to each act) are inline. Every field
/// is signed; display identity comes from `subject.kind`, `subject.ref`,
/// `issuer` and `authority.actor_ref`.
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
}
