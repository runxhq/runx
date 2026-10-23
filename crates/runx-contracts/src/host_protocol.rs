//! Host protocol contracts: execution events, resolution requests, host-run lifecycle.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::purchase::{
    CurrencyCode, OfferVersionBinding, PortableAmountMinor, PrincipalReference,
    PurchasePaymentMethod, PurchasePaymentOptionBinding,
};
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{AgentContextEnvelope, Digest, JsonObject, JsonValue};

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
    /// A call reached a paid runner and waits on the person's exact decision:
    /// they answer `approved` and the method they pay with (`choice`).
    Purchase {
        id: NonEmptyString,
        purchase: Box<PurchaseRequest>,
    },
}

/// What a person decides when a call reaches a paid runner: one
/// quote's exact offer version, seller and price, and each payment method
/// that can pay it. The request's digest binds all of it, so an answer pays
/// only this quote, before it expires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.purchase_request.v1")]
pub struct PurchaseRequest {
    pub skill: NonEmptyString,
    pub runner: NonEmptyString,
    pub offer_version: OfferVersionBinding,
    /// The execution package digest the purchase runs (`runx.skill-package.v1`
    /// over the files a run loads), not the registry's digest of the published
    /// package files that a listing binds.
    pub package_digest: Digest,
    pub seller_ref: PrincipalReference,
    pub amount_minor: PortableAmountMinor,
    pub currency: CurrencyCode,
    pub input_digest: Digest,
    pub quote: PurchaseQuote,
    /// Never empty: a call no method can pay fails instead of asking.
    pub methods: Vec<PurchaseMethod>,
}

/// The quote decided on, including its protocol-owned display terms and the
/// exact challenge binding. An absent network is not an EVM network default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchaseQuote {
    pub purchase_id: NonEmptyString,
    pub expires_at: IsoDateTime,
    pub payment_options: Vec<PurchasePaymentOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchasePaymentOption {
    pub option: PurchasePaymentOptionBinding,
    pub expires_at: IsoDateTime,
    pub quote_ref: crate::Reference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_method: Option<PurchasePaymentMethod>,
}

/// What an answer's `choice` names to pay from the account's runx balance. No
/// connector instance id can take this value.
pub const PURCHASE_BALANCE_CHOICE: &str = "runx:balance";

/// One way the buyer can pay. An answer's `choice` names it: the balance by
/// `runx:balance`, a payment grant by the choice binding its instance and
/// quoted option.
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
        choice: NonEmptyString,
        instance_id: NonEmptyString,
        connector: NonEmptyString,
        total_minor: PortableAmountMinor,
        option: PurchasePaymentOptionBinding,
    },
}

impl PurchaseMethod {
    /// What an answer's `choice` names to pay with this method.
    #[must_use]
    pub fn choice(&self) -> &str {
        match self {
            Self::Balance { .. } => PURCHASE_BALANCE_CHOICE,
            Self::Grant { choice, .. } => choice.as_str(),
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

/// The gate an approval request asks a person to pass, and why.
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
    pub payload_artifact: Option<crate::ProjectArtifact>,
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
    /// Catalog tool names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<NonEmptyString>,
    /// Child skills by ref, and the catalog rules a load is admitted under.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillAllowance>,
    /// Whether the turn may ask the human.
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub asks: bool,
}

impl TurnAllowance {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty() && self.tools.is_empty() && self.skills.is_empty() && !self.asks
    }

    /// The child skills the allowance names.
    pub fn named_skills(&self) -> impl Iterator<Item = &NonEmptyString> {
        self.skills.iter().filter_map(|entry| match entry {
            SkillAllowance::Skill(reference) => Some(reference),
            SkillAllowance::Rule(_) => None,
        })
    }

    /// The catalog rules a load is admitted under.
    pub fn rules(&self) -> impl Iterator<Item = &CatalogRule> {
        self.skills.iter().filter_map(|entry| match entry {
            SkillAllowance::Rule(rule) => Some(rule),
            SkillAllowance::Skill(_) => None,
        })
    }
}

/// One entry of an allowance's `skills`: a child skill by name, or a catalog
/// rule, which admits the skills and connectors a `load` call finds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(untagged)]
pub enum SkillAllowance {
    Skill(NonEmptyString),
    Rule(CatalogRule),
}

/// What a load admits: exactly one of `catalog`, the registry's packages at
/// a trust tier or above, and `project: true`, the packages the project
/// authored. A registry package the project installed keeps its registry
/// identity, so only `catalog` admits it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogTrust>,
    #[serde(default, skip_serializing_if = "crate::schema::is_default")]
    pub project: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogTrust {
    /// The lowest tier a load admits on its own; a package below it waits
    /// on the person's trust.
    pub trust: crate::TrustTier,
}

impl CatalogRule {
    /// One kind of rule.
    pub fn check(&self) -> Result<(), String> {
        if self.catalog.is_some() == self.project {
            return Err("a catalog rule is exactly one of catalog and project: true".to_owned());
        }
        Ok(())
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
    /// A child skill: one the allowance names, run by its default runner, or
    /// one runner of a skill a load bound, run as the load bound it.
    Skill {
        skill: NonEmptyString,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runner: Option<NonEmptyString>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bound: Option<BoundClosure>,
    },
    /// The call's input is `{reason}`; the human approves or declines it.
    Ask,
    /// The call's input is `{skill, runner?}` or `{connector}`: a package a
    /// catalog rule admits, whose runners or operations the turn offers from
    /// then on.
    Load,
}

/// The exact package and execution closure a load bound for one runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundClosure {
    pub package_digest: NonEmptyString,
    pub closure_digest: NonEmptyString,
}

/// A completed load's output: what it bound, a skill's manual and runners,
/// and the calls it adds to the turn. A skill's load binds the runner it
/// names, or the skill's only runner; one that binds none is a reading of the
/// skill, which binds no closure and so asks no trust.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.turn_load.v1")]
pub struct TurnLoad {
    pub package: LoadedPackage,
    /// A loaded skill's manual; none for a connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual: Option<SkillManual>,
    /// Every runner of a loaded skill, as a model chooses one, and whether
    /// this load bound it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runners: Vec<LoadedRunner>,
    /// One per bound runner, or per operation of a loaded connector, as the
    /// turn offers it from then on.
    pub tools: Vec<TurnTool>,
    /// Each connector what it bound reaches, by name, at the manifest digest
    /// it binds: a later load that binds another revision is refused.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connectors: BTreeMap<String, NonEmptyString>,
}

/// A skill's manual: its `SKILL.md` exactly as its package holds it, the
/// skill's operating guidance. A host gives it to its model as guidance,
/// below the host's own instructions and runx's governance, which it never
/// widens, and never as a tool result's data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillManual {
    /// `sha256:` over `content`'s bytes.
    pub digest: NonEmptyString,
    pub content: String,
}

/// One runner of a loaded skill, as a model chooses it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadedRunner {
    pub name: NonEmptyString,
    /// What it does and when to use it, as its package states it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<NonEmptyString>,
    pub inputs: Vec<LoadedRunnerInput>,
    /// Whether this load bound it, so the turn offers it as a tool.
    pub bound: bool,
}

/// One input of a loaded skill's runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadedRunnerInput {
    pub name: NonEmptyString,
    #[serde(rename = "type")]
    pub input_type: NonEmptyString,
    pub required: bool,
}

/// The package a load bound. A project's own package has no tier and no
/// publisher.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LoadedPackage {
    Skill {
        /// The ref each of its tools names: the canonical
        /// `<publisher>/<name>@<version>` of a registry skill, or the project's
        /// path.
        skill: NonEmptyString,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trust_tier: Option<crate::TrustTier>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        publisher: Option<NonEmptyString>,
        package_digest: NonEmptyString,
    },
    Connector {
        connector: NonEmptyString,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trust_tier: Option<crate::TrustTier>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        publisher: Option<NonEmptyString>,
        /// The manifest each of its operations runs; one that changed since
        /// the load is refused.
        manifest_digest: NonEmptyString,
    },
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
        artifacts: BTreeMap<String, crate::ProjectArtifact>,
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
/// turn's address), with the act's context as an index, the calls it may
/// make, and how each call it made so far ended. Nothing in it is the data of
/// a context entry, which `turn_read` reads by its locator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.turn_status.v1")]
pub struct TurnStatus {
    pub invocation: TurnInvocation,
    pub tools: Vec<TurnTool>,
    pub calls: Vec<TurnCallResult>,
}

/// An agent act as its turn's agent reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnInvocation {
    pub id: NonEmptyString,
    pub role: NonEmptyString,
    pub task: NonEmptyString,
    pub envelope: TurnEnvelope,
    pub allowance: TurnAllowance,
}

impl From<&AgentActInvocation> for TurnInvocation {
    fn from(invocation: &AgentActInvocation) -> Self {
        let AgentActInvocation {
            id,
            role,
            task,
            envelope,
            allowance,
        } = invocation;
        Self {
            id: id.clone(),
            role: role.clone(),
            task: task.clone(),
            envelope: TurnEnvelope::from(envelope),
            allowance: allowance.clone(),
        }
    }
}

/// An act's envelope as its turn's agent reads it: whole, except that each
/// context entry is a line of an index, its data read by `turn_read`. The
/// act's instructions and inputs stay whole, as the act itself; each input
/// also reads by its ref.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnEnvelope {
    pub run_id: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<NonEmptyString>,
    pub skill: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<crate::CallAct>,
    pub instructions_digest: NonEmptyString,
    pub instructions: NonEmptyString,
    pub inputs: JsonObject,
    pub requirements: crate::AgentExecutionRequirements,
    pub current_context: Vec<TurnContextLine>,
    pub historical_context: Vec<TurnContextLine>,
    pub provenance: Vec<crate::ProvenanceEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<crate::AgentContextProfiles>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_profile: Option<crate::ProfileFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_location: Option<crate::ExecutionLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<BTreeMap<String, crate::OutputField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonValue>,
    pub trust_boundary: NonEmptyString,
}

impl From<&AgentContextEnvelope> for TurnEnvelope {
    fn from(envelope: &AgentContextEnvelope) -> Self {
        let AgentContextEnvelope {
            run_id,
            step_id,
            skill,
            act,
            instructions_digest,
            instructions,
            inputs,
            requirements,
            current_context,
            historical_context,
            provenance,
            context,
            voice_profile,
            execution_location,
            output,
            output_schema,
            trust_boundary,
        } = envelope;
        let lines = |entries: &[crate::ContextEntry], at: fn(usize) -> TurnLocator| {
            entries
                .iter()
                .enumerate()
                .map(|(index, entry)| TurnContextLine::of(at(index), entry))
                .collect()
        };
        Self {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
            skill: skill.clone(),
            act: act.clone(),
            instructions_digest: instructions_digest.clone(),
            instructions: instructions.clone(),
            inputs: inputs.clone(),
            requirements: requirements.clone(),
            current_context: lines(current_context, TurnLocator::Current),
            historical_context: lines(historical_context, TurnLocator::Historical),
            provenance: provenance.clone(),
            context: context.clone(),
            voice_profile: voice_profile.clone(),
            execution_location: execution_location.clone(),
            output: output.clone(),
            output_schema: output_schema.clone(),
            trust_boundary: trust_boundary.clone(),
        }
    }
}

/// One context entry of an act as a model reads it in an index: what it is,
/// whom it may be shared with, how large it is, and the locator `turn_read`
/// reads its data by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnContextLine {
    pub locator: NonEmptyString,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub entry_type: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<NonEmptyString>,
    pub scope: crate::ContextScope,
    pub created_at: NonEmptyString,
    pub size_bytes: u64,
}

impl TurnContextLine {
    fn of(locator: TurnLocator, entry: &crate::ContextEntry) -> Self {
        Self {
            locator: locator.to_string().into(),
            entry_type: entry.entry_type.clone(),
            summary: entry.summary.clone(),
            scope: entry.scope.clone(),
            created_at: entry.meta.created_at.clone(),
            size_bytes: entry.meta.size_bytes,
        }
    }
}

/// What a locator names within one turn: an entry of the act's current or
/// historical context by its place, an input of the act by name, or a call
/// of the turn by its index. A locator means nothing outside its turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnLocator {
    Current(usize),
    Historical(usize),
    Input(String),
    Call(u32),
}

impl TurnLocator {
    /// The locator `text` spells, if it spells one.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, rest) = text.split_once(':')?;
        match kind {
            "current" => rest.parse().ok().map(Self::Current),
            "historical" => rest.parse().ok().map(Self::Historical),
            "input" if !rest.is_empty() => Some(Self::Input(rest.to_owned())),
            "call" => rest.parse().ok().map(Self::Call),
            _ => None,
        }
    }
}

impl std::fmt::Display for TurnLocator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Current(index) => write!(formatter, "current:{index}"),
            Self::Historical(index) => write!(formatter, "historical:{index}"),
            Self::Input(name) => write!(formatter, "input:{name}"),
            Self::Call(index) => write!(formatter, "call:{index}"),
        }
    }
}

/// The most characters one `turn_read` page holds. It bounds a page, never
/// what can be read: a read continues from the offset it returns.
pub const TURN_READ_PAGE_CHARS: u64 = 32_000;

/// One page of what a turn's locator names, as text: a string as it is, any
/// other value as pretty JSON. Offsets count characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.turn_read.v1")]
pub struct TurnReadPage {
    pub locator: NonEmptyString,
    pub offset: u64,
    pub text: String,
    /// Where the next page starts: the offset after the last character this
    /// page delivered. None when this page reaches the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
    /// How many characters the whole holds.
    pub total: u64,
}

impl TurnReadPage {
    /// The page of `whole` from `offset`, of at most `limit` characters
    /// (and never more than a page holds).
    #[must_use]
    pub fn of(locator: &TurnLocator, whole: &str, offset: u64, limit: Option<u64>) -> Self {
        let total = whole.chars().count() as u64;
        let limit = limit
            .unwrap_or(TURN_READ_PAGE_CHARS)
            .clamp(1, TURN_READ_PAGE_CHARS);
        let start = usize::try_from(offset.min(total)).unwrap_or(usize::MAX);
        let length = usize::try_from(limit).unwrap_or(usize::MAX);
        let text = whole.chars().skip(start).take(length).collect::<String>();
        let end = offset.min(total) + text.chars().count() as u64;
        Self {
            locator: locator.to_string().into(),
            offset: offset.min(total),
            text,
            next_offset: (end < total).then_some(end),
            total,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locator_reads_back_as_it_was_written_and_nothing_else_is_one() {
        for locator in [
            TurnLocator::Current(0),
            TurnLocator::Historical(12),
            TurnLocator::Input("repo".to_owned()),
            TurnLocator::Call(3),
        ] {
            assert_eq!(TurnLocator::parse(&locator.to_string()), Some(locator));
        }
        for text in ["", "current", "current:x", "input:", "call:-1", "turn:1"] {
            assert_eq!(TurnLocator::parse(text), None, "{text}");
        }
    }

    #[test]
    fn a_read_makes_progress_page_by_page_to_the_end() {
        let locator = TurnLocator::Call(0);
        let whole = "ab\u{e9}cd";
        let first = TurnReadPage::of(&locator, whole, 0, Some(2));
        assert_eq!((first.text.as_str(), first.next_offset), ("ab", Some(2)));
        let second = TurnReadPage::of(&locator, whole, 2, Some(2));
        assert_eq!(
            (second.text.as_str(), second.next_offset),
            ("\u{e9}c", Some(4))
        );
        let last = TurnReadPage::of(&locator, whole, 4, None);
        assert_eq!(
            (last.text.as_str(), last.next_offset, last.total),
            ("d", None, 5)
        );
        // A limit of nothing still delivers, and an offset past the end reads empty.
        assert_eq!(TurnReadPage::of(&locator, whole, 0, Some(0)).text, "a");
        let past = TurnReadPage::of(&locator, whole, 9, None);
        assert_eq!(
            (past.offset, past.text.as_str(), past.next_offset),
            (5, "", None)
        );
        // No page exceeds its ceiling, whatever is asked.
        let long = "x".repeat(40_000);
        let page = TurnReadPage::of(&locator, &long, 0, Some(100_000));
        assert_eq!(page.text.len() as u64, TURN_READ_PAGE_CHARS);
        assert_eq!(page.next_offset, Some(TURN_READ_PAGE_CHARS));
    }
}
