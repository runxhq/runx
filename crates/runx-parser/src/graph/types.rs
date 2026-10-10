use std::collections::BTreeMap;

use runx_contracts::connector::OperationName;
use runx_contracts::{JsonObject, JsonValue, TurnAllowance};
use serde::{Deserialize, Serialize};

use runx_contracts::EnvironmentRequirements;

use super::binding::{Binding, Operand};
use crate::skill::{ArtifactPageSource, InputMode, SkillArtifactContract};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawGraphIr {
    pub document: JsonObject,
}

/// A condition the run must meet before `step` runs: a guard blocks the run
/// when `value` does not hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphGuard {
    pub step: String,
    pub value: Operand,
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
    pub value: Operand,
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
    /// The values the step is invoked with, as written: literals, with each
    /// reference left in place for `bindings` to fill. For an operation step
    /// these are the call's fields (`input`, `idempotency_key`, ...).
    pub inputs: JsonObject,
    /// Where `inputs` reads the run's input and earlier steps' results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
    /// The act an operation step's call performs, or the act a skill step's
    /// child run takes as its `$act`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<StepAct>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_skills: Vec<String>,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<GraphWhen>,
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

    /// The earlier steps whose results this step's values read, in order and
    /// once each. Its condition is not among them: a condition selects the
    /// step, it is not its input.
    #[must_use]
    pub fn reads(&self) -> Vec<&str> {
        let mut steps = Vec::new();
        for step in self
            .bindings
            .iter()
            .chain(self.act.iter().flat_map(|act| &act.bindings))
            .filter_map(|binding| binding.source.step())
        {
            if !steps.contains(&step) {
                steps.push(step);
            }
        }
        steps
    }
}

/// A step's `act:` as written: its literal fields, with each reference left
/// in place, and the bindings that fill them. `act: $act` is one binding of
/// the whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepAct {
    pub values: JsonObject,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
}

/// An operation step's target: a key of the skill's `connectors:` block and,
/// unless the step binds its `operation_name` at run time, one operation on it.
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
