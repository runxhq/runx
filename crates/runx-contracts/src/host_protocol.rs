//! Host protocol contracts: execution events, resolution requests, host-run lifecycle.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::paid_invocation::{
    CurrencyCode, OfferRevisionRef, PortableAmountMinor, PrincipalReference, Sha256Digest,
};
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{AgentContextEnvelope, JsonObject, JsonValue};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionEvent {
    SkillLoaded {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    InputsResolved {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    AuthResolved {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    ResolutionRequested {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    ResolutionResolved {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    Admitted {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    Executing {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    StepStarted {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    StepWaitingResolution {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    StepCompleted {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    Warning {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
    Completed {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<JsonValue>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[runx_schema(id = "runx.resolution_request.v1")]
pub enum ResolutionRequest {
    Input {
        id: NonEmptyString,
        questions: Vec<Question>,
    },
    Approval {
        id: NonEmptyString,
        gate: Box<ApprovalGate>,
    },
    AgentAct {
        id: NonEmptyString,
        invocation: Box<AgentActInvocation>,
    },
    /// A step or call waits to run again. A bounded retry
    /// (`retry_after_seconds` above zero) is runx's own: the driver that
    /// reaches it after the delay runs it. A claimed call that never settled
    /// (`retry_after_seconds` zero) runs again only on a person's
    /// acknowledgement, `{"retry": true}`, and only when it can be resent.
    Retry {
        id: NonEmptyString,
        reason: NonEmptyString,
        retry_after_seconds: u64,
        /// Whether a claimed call can be sent again with the same input: its
        /// provider deduplicates it, or it only reads. One that cannot is
        /// never resent; its run can only be closed with the call
        /// unconfirmed.
        #[serde(default, skip_serializing_if = "crate::schema::is_default")]
        resend: RetryResend,
    },
    /// A call reached a paid runner and waits on the person's exact decision
    /// (N10): they answer `approved` and the method they pay with
    /// (`choice`).
    Purchase {
        id: NonEmptyString,
        purchase: Box<PurchaseRequest>,
    },
}

/// What a person decides when a call reaches a paid runner (N10): one
/// quote's exact offer, seller and price, and each payment method that can
/// pay it. The request's digest binds all of it, so an answer pays only this
/// quote, before it expires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.purchase_request.v1")]
pub struct PurchaseRequest {
    pub skill: NonEmptyString,
    pub runner: NonEmptyString,
    pub offer: OfferRevisionRef,
    /// The execution package digest the purchase runs (`runx.skill-package.v1`
    /// over the files a run loads), not the registry's digest of the published
    /// package files that a listing binds.
    pub package_digest: Sha256Digest,
    pub seller: PrincipalReference,
    pub amount_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub input_digest: Sha256Digest,
    pub quote: PurchaseQuote,
    /// Never empty: a call no method can pay fails instead of asking.
    pub methods: Vec<PurchaseMethod>,
}

/// The quote decided on: the paid invocation Cloud admitted for these
/// inputs, what its x402 requirement pays, and when it expires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchaseQuote {
    pub invocation_id: NonEmptyString,
    pub expires_at: IsoDateTime,
    pub network: NonEmptyString,
    pub asset: NonEmptyString,
    pub pay_to: NonEmptyString,
}

/// What an answer's `choice` names to pay from the account's runx balance. No
/// connector instance id can take this value.
pub const PURCHASE_BALANCE_CHOICE: &str = "runx:balance";

/// One way the payer can pay. An answer's `choice` names it: the balance by
/// [`PURCHASE_BALANCE_CHOICE`], a payment grant by its instance id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PurchaseMethod {
    /// The account's runx balance, which covers the price: the price moves
    /// inside runx's ledger, with no settlement per call.
    Balance {
        available_micro: u64,
        total_minor: PortableAmountMinor,
    },
    /// A hosted payment grant whose connector pays on the quote's network
    /// and asset.
    Grant {
        instance_id: NonEmptyString,
        connector: NonEmptyString,
        total_minor: PortableAmountMinor,
    },
}

impl PurchaseMethod {
    /// What an answer's `choice` names to pay with this method.
    #[must_use]
    pub fn choice(&self) -> &str {
        match self {
            Self::Balance { .. } => PURCHASE_BALANCE_CHOICE,
            Self::Grant { instance_id, .. } => instance_id.as_str(),
        }
    }
}

/// Whether sending a claimed call again could repeat its effect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetryResend {
    #[default]
    Safe,
    Unsafe,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.question.v1")]
pub struct Question {
    pub id: NonEmptyString,
    pub prompt: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub required: bool,
    /// `choice` when it offers options, `text` otherwise.
    #[serde(rename = "type")]
    pub question_type: NonEmptyString,
    /// What the person may choose, the recommended first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<QuestionOption>,
    /// Whether the person may answer in their own words.
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub free_text: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionOption {
    pub label: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Check a person's answer to `questions`: `{<question id>: {option?:
/// <label>, text?: <string>}}`, each naming one of its question's options or,
/// where the question allows it, text of their own, and every required
/// question answered.
pub fn check_answer(questions: &[Question], answer: &JsonValue) -> Result<(), String> {
    let answers = answer
        .as_object()
        .ok_or("an answer to questions is an object by question id")?;
    for (id, given) in answers {
        let question = questions
            .iter()
            .find(|question| question.id.as_str() == id)
            .ok_or_else(|| format!("{id} is not one of the questions"))?;
        let given = given
            .as_object()
            .ok_or_else(|| format!("the answer to {id} is {{option?, text?}}"))?;
        if let Some(field) = given.keys().find(|key| *key != "option" && *key != "text") {
            return Err(format!(
                "the answer to {id} takes option and text, not {field}"
            ));
        }
        let option = given.get("option").map(|option| {
            option
                .as_str()
                .ok_or_else(|| format!("the option answering {id} is a label"))
        });
        let text = given.get("text").map(|text| {
            text.as_str()
                .ok_or_else(|| format!("the text answering {id} is a string"))
        });
        match (option.transpose()?, text.transpose()?) {
            (None, None) => return Err(format!("the answer to {id} names an option or text")),
            (Some(label), _)
                if !question
                    .options
                    .iter()
                    .any(|option| option.label.as_str() == label) =>
            {
                return Err(format!("{label} is not an option of {id}"));
            }
            (_, Some(_)) if !question.free_text => {
                return Err(format!("{id} takes one of its options, not text"));
            }
            _ => {}
        }
    }
    if let Some(missing) = questions
        .iter()
        .find(|question| question.required && !answers.contains_key(question.id.as_str()))
    {
        return Err(format!("{} is required", missing.id));
    }
    Ok(())
}

/// Host protocol approval request gate carried by `ResolutionRequest::Approval`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.approval_gate.v1")]
pub struct ApprovalGate {
    pub id: NonEmptyString,
    pub reason: NonEmptyString,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub gate_type: Option<String>,
    /// Shallow summary payload. `payload_digest` in it is the stable digest
    /// (`runx.stable-json.v1`) of what executes once the gate is approved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<JsonObject>,
    /// What executes once the gate is approved, when it is at most 64 KiB. A
    /// host recomputes `summary.payload_digest` over it before it offers
    /// Approve; the request's digest binds it with the rest of the gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonValue>,
    /// What executes, when it is larger: a project artifact whose bytes are
    /// its canonical JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_ref: Option<crate::ProjectArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.agent_act_invocation.v1")]
pub struct AgentActInvocation {
    pub id: NonEmptyString,
    /// The kind of agent the step asks for.
    pub role: NonEmptyString,
    /// The act it performs.
    pub task: NonEmptyString,
    pub envelope: AgentContextEnvelope,
    /// What the agent may call while its turn is open.
    pub allowance: TurnAllowance,
}

/// The calls an open turn admits: the skill's declared limits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnAllowance {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<AllowedOperation>,
    /// Catalog tool refs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<NonEmptyString>,
    /// Child skill refs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<NonEmptyString>,
    /// Whether the turn may ask the human.
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub asks: bool,
}

impl TurnAllowance {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty() && self.tools.is_empty() && self.skills.is_empty() && !self.asks
    }
}

/// One connector operation a turn may call, named by the requirement that
/// binds its instance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AllowedOperation {
    pub requirement: NonEmptyString,
    pub operation: NonEmptyString,
}

/// What a turn call names: one entry of the turn's allowance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnTarget {
    Tool {
        tool: NonEmptyString,
    },
    Operation {
        requirement: NonEmptyString,
        operation: NonEmptyString,
    },
    Skill {
        skill: NonEmptyString,
    },
    /// The call's input is `{reason}`; the human approves or declines it.
    Ask,
}

/// One call an agent made in its turn, and how it ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.turn_call.v1")]
pub struct TurnCallResult {
    /// The call's place in the turn, from 0.
    pub index: u32,
    pub target: TurnTarget,
    /// `sha256:` over the canonical JSON of the call's input
    /// (`runx.stable-json.v1`), so a host that recorded a call before making
    /// it can prove which call sits at this index.
    pub input_digest: NonEmptyString,
    /// The id its host pinned on the call, unique within its turn: a call
    /// made again with it returns this call rather than running twice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<NonEmptyString>,
    pub outcome: TurnCallOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnCallOutcome {
    /// The call ran and succeeded.
    Completed {
        output: JsonObject,
        receipt_id: NonEmptyString,
        /// Each packet output as a project artifact of the run, by output
        /// name: a reference a later call passes in place of the packet.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        artifacts: BTreeMap<String, crate::ProjectArtifactRef>,
    },
    /// The call ran and failed, with its receipt; or it failed before it
    /// could run.
    Failed {
        reason: NonEmptyString,
        #[serde(skip_serializing_if = "Option::is_none")]
        receipt_id: Option<NonEmptyString>,
    },
    /// Admission refused the call; nothing ran.
    Refused { reason: NonEmptyString },
    /// The call has not settled. `waiting` is what it waits on, each request
    /// by its address: its own ask or approval, its retry while its effect
    /// is unconfirmed, or what the child run it started waits on, including
    /// that run's open turns. The call settles when those are answered.
    Pending { waiting: Vec<ResolutionRequest> },
}

/// One call an open turn offers its agent: an allowance entry, named for
/// MCP, with the input it takes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnTool {
    /// Distinct within the turn, in the MCP tool-name grammar.
    pub name: NonEmptyString,
    pub description: NonEmptyString,
    pub target: TurnTarget,
    pub input_schema: JsonObject,
}

/// An open turn as its agent reads it: the act it answers (its id is the
/// turn's address), the calls it may make, and how each call it made so far
/// ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.turn_status.v1")]
pub struct TurnStatus {
    pub invocation: AgentActInvocation,
    pub tools: Vec<TurnTool>,
    pub calls: Vec<TurnCallResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.resolution_response.v1")]
pub struct ResolutionResponse {
    pub actor: ResolutionResponseActor,
    /// Shallow response payload.
    pub payload: JsonValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionResponseActor {
    Human,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecision {
    pub gate: ApprovalGate,
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HostRunResult {
    NeedsAgent {
        skill_name: String,
        run_id: String,
        requests: Vec<ResolutionRequest>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_ids: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_labels: Option<Vec<String>>,
        events: Vec<ExecutionEvent>,
    },
    Completed {
        skill_name: String,
        receipt_id: String,
        output: String,
        events: Vec<ExecutionEvent>,
    },
    Failed {
        skill_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        receipt_id: Option<String>,
        error: String,
        events: Vec<ExecutionEvent>,
    },
    Escalated {
        skill_name: String,
        receipt_id: String,
        error: String,
        events: Vec<ExecutionEvent>,
    },
    Denied {
        skill_name: String,
        reasons: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        receipt_id: Option<String>,
        events: Vec<ExecutionEvent>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostRunState {
    NeedsAgent(HostNeedsAgentState),
    Completed(HostTerminalState),
    Failed(HostTerminalState),
    Escalated(HostTerminalState),
    Denied(HostTerminalState),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostNeedsAgentState {
    pub skill_name: String,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_runner: Option<String>,
    pub requests: Vec<ResolutionRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineage: Option<HostRunLineage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostTerminalState {
    pub kind: HostRunKind,
    pub skill_name: String,
    pub run_id: String,
    pub receipt_id: String,
    pub verification: HostRunVerification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actors: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<HostRunApproval>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineage: Option<HostRunLineage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostRunKind {
    Harness,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRunVerification {
    pub status: HostRunVerificationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostRunVerificationStatus {
    Verified,
    Unverified,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostRunLineage {
    pub kind: HostRunLineageKind,
    pub source_run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_receipt_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostRunLineageKind {
    Rerun,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostRunApproval {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<HostRunApprovalDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostRunApprovalDecision {
    Approved,
    Denied,
}
