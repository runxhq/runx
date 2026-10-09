use std::collections::BTreeMap;

use runx_contracts::connector::OperationName;
use runx_contracts::{JsonObject, JsonValue, TurnAllowance};
use serde::{Deserialize, Serialize};

use runx_contracts::EnvironmentRequirements;

use crate::skill::{ArtifactPageSource, InputMode, SkillArtifactContract};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawGraphIr {
    pub document: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphContextEdge {
    pub input: String,
    pub from_step: String,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphGuard {
    pub step: String,
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equals: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<JsonValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphPolicy {
    pub guards: Vec<GraphGuard>,
}

/// Per-step conditional selection. When present and the condition does not hold
/// (or the field is unresolved), the step is skipped and the graph continues;
/// sibling steps with complementary conditions form a branch. Unlike a guard,
/// a `when` never blocks the run, it selects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphWhen {
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equals: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<JsonValue>,
}

/// What one step does: exactly one of the five step kinds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// An agent act, resolved by the caller or the managed agent loop.
    Agent(AgentTask),
    /// A catalog tool, a deterministic worker, or a trusted host process.
    Tool(ToolTarget),
    /// A connector operation, admitted and executed by the kernel.
    Operation(OperationStepTarget),
    /// A resolution the run waits on before it continues.
    Ask(AskGate),
    /// An attenuated child run of another skill.
    Skill(SkillTarget),
}

impl StepKind {
    /// The step kind's name, as authors write it.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Agent(_) => "agent",
            Self::Tool(_) => "tool",
            Self::Operation(_) => "operation",
            Self::Ask(_) => "ask",
            Self::Skill(_) => "skill",
        }
    }
}

/// An agent act: the role that answers and the task it performs. The step's
/// `outputs` declare what the answer must contain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTask {
    pub role: String,
    pub task: String,
    /// What the turn may call: the enclosing skill's limits, bound when the
    /// skill is parsed. A graph outside a skill allows nothing.
    #[serde(default, skip_serializing_if = "TurnAllowance::is_empty")]
    pub allowance: TurnAllowance,
    /// The skill's declarations of the requirements the allowance's
    /// operations name, bound with it: what admits each operation call.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connectors: BTreeMap<String, crate::ConnectorRequirement>,
}

/// A tool step's target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolTarget {
    /// A catalog or manifest tool, by reference.
    Catalog(String),
    /// A deterministic JavaScript module run in the worker, with no I/O.
    Worker(WorkerTool),
    /// A trusted host process: runx controls its argv, cwd, environment and
    /// lifetime, but does not confine it.
    Process(ProcessTool),
}

/// A deterministic JavaScript module the worker runs over the step's inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerTool {
    /// Relative ECMAScript module path inside the package.
    pub module: String,
    /// Named export; the module's default export when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Runtime-owned paging over one contained immutable file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<ArtifactPageSource>,
    /// Non-secret configuration names the module reads. Values are resolved
    /// by the runtime and never stored here.
    #[serde(default, skip_serializing_if = "EnvironmentRequirements::is_empty")]
    pub environment: EnvironmentRequirements,
}

/// A trusted host process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessTool {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<InputMode>,
    /// Non-secret configuration names the process reads. Values are resolved
    /// by the runtime and never stored here.
    #[serde(default, skip_serializing_if = "EnvironmentRequirements::is_empty")]
    pub environment: EnvironmentRequirements,
}

/// An approval the run waits on: the gate, the reason a person is shown, and
/// optionally the kind of approval. The step's `inputs` and `context` form the
/// summary shown beside the reason. Its result is a
/// `runx.approval.decision.v1` packet named `approval_decision`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskGate {
    pub gate: String,
    pub reason: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub gate_type: Option<String>,
}

/// A child skill run, by reference and optional runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillTarget {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphStep {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What the step does.
    pub kind: StepKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<SkillArtifactContract>,
    /// Complete output declarations owned by this step. Preserve these in
    /// typed IR so inspection, packet generation, and registry packaging do
    /// not have to reparse the raw manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<JsonObject>,
    /// The values the step is invoked with. For an operation step these are
    /// the call's fields (`input`, `approval`, `idempotency_key`, ...); for an
    /// ask step the gate and reason.
    pub inputs: JsonObject,
    pub context: BTreeMap<String, String>,
    pub context_edges: Vec<GraphContextEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_skills: Vec<String>,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<GraphWhen>,
    /// A tool or skill step's request key. An operation step's key is one of
    /// its call fields in `inputs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// What this use accepts when an operation's outcome is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_uncertain: Option<UncertainOutcome>,
    /// Whether this use tolerates a visible duplicate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<DuplicatePolicy>,
}

impl GraphStep {
    #[must_use]
    pub fn operation(&self) -> Option<&OperationStepTarget> {
        match &self.kind {
            StepKind::Operation(target) => Some(target),
            _ => None,
        }
    }

    #[must_use]
    pub fn skill(&self) -> Option<&SkillTarget> {
        match &self.kind {
            StepKind::Skill(target) => Some(target),
            _ => None,
        }
    }

    /// The catalog tool reference, for a catalog tool step.
    #[must_use]
    pub fn catalog_tool(&self) -> Option<&str> {
        match &self.kind {
            StepKind::Tool(ToolTarget::Catalog(reference)) => Some(reference),
            _ => None,
        }
    }
}

/// An operation step's target: a key of the skill's `connectors:` block and,
/// unless the step binds it as its `operation` input, one operation on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationStepTarget {
    pub requirement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationName>,
    /// The requirement as the enclosing skill declares it, bound when the
    /// skill is parsed. A graph outside a skill has none, and its operation
    /// steps are refused at admission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<crate::ConnectorRequirement>,
}

/// What a use accepts when an operation's outcome is unknown. The kernel
/// refuses a tolerance the operation's guarantees cannot support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum UncertainOutcome {
    /// Hold for reconciliation; never re-send blind. The default.
    Hold,
    /// Settle the outcome through the operation's reconcile read.
    Reconcile,
    /// Give up after this long and let the caller supersede the work.
    ExpireAfter { seconds: u64 },
}

impl UncertainOutcome {
    /// `hold`, `reconcile`, or `expire_after <n><s|m|h|d>`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "hold" => Some(Self::Hold),
            "reconcile" => Some(Self::Reconcile),
            other => {
                let duration = other.strip_prefix("expire_after")?.trim();
                let (count, unit) = duration.split_at(duration.len().checked_sub(1)?);
                let count = count.parse::<u64>().ok().filter(|count| *count > 0)?;
                let unit = match unit {
                    "s" => 1,
                    "m" => 60,
                    "h" => 3_600,
                    "d" => 86_400,
                    _ => return None,
                };
                count
                    .checked_mul(unit)
                    .map(|seconds| Self::ExpireAfter { seconds })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicatePolicy {
    /// No duplicate may become visible. The default.
    Forbidden,
    /// A duplicate is acceptable to this use.
    Visible,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionGraph {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Steps whose complete declared output contracts form the graph's public
    /// result. Multiple entries support mutually exclusive terminal branches
    /// and workflows that intentionally return both an act result and its
    /// durable readback. Diagnostic/base fields are never exported.
    pub result_from: Vec<String>,
    pub steps: Vec<GraphStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<GraphPolicy>,
    pub raw: RawGraphIr,
}
