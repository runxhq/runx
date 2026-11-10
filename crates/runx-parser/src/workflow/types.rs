use std::collections::BTreeMap;

use runx_contracts::connector::OperationName;
use runx_contracts::{JsonObject, JsonValue, TurnAllowance};
use serde::{Deserialize, Serialize};

use runx_contracts::EnvironmentRequirements;

use super::binding::{Binding, Operand, Source};
use crate::skill::{ArtifactPageSource, InputMode, OutputArtifacts};

/// A condition on a step: `value` equals, or does not equal, a literal. As a
/// step's `when` it selects the step: when the condition does not hold (or
/// the field is unresolved), the step is skipped and the workflow continues,
/// and sibling steps with complementary conditions form a branch. As one of a
/// step's `guard` conditions it must hold before the step may run, or the run
/// fails.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowWhen {
    pub value: Operand,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equals: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<JsonValue>,
}

/// What one step does: exactly one of the five step kinds a method declares,
/// or a load, which only an agent's call makes.
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
    /// A package a turn's catalog rule admits, bound for the turn's later
    /// calls.
    Load(LoadStep),
    /// Deterministic code in the JavaScript worker that drives the calls its
    /// step lists, each admitted, run and recorded by the kernel.
    Script(ScriptStep),
}

/// A script step: `script: { module, export }`, called as `(ctx, inputs)`.
/// It reaches only what its `calls` list, and replays from its run's record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptStep {
    /// Relative ECMAScript module path inside the package.
    pub module: String,
    /// Named export; the module's default export when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<String>,
    /// What the script may call: the step's `calls`, its tools, operations
    /// and skills. Writing them grants nothing.
    #[serde(default, skip_serializing_if = "TurnAllowance::is_empty")]
    pub allowance: TurnAllowance,
    /// The skill's requirements for the connectors the allowance's operations
    /// name, bound with it: what admits each operation call.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connectors: BTreeMap<String, crate::ConnectorRequirement>,
    /// The hard stop: a call past it fails the step.
    pub max_calls: u32,
    /// How many of its calls run at once, drawn from the run's
    /// `max_concurrency`.
    pub concurrency: u32,
}

/// A load an agent's call makes: what it names, the rules that admit it,
/// and what the turn already binds, which the load must agree with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadStep {
    pub target: LoadTarget,
    /// The catalog rules of the turn's allowance.
    pub rules: Vec<runx_contracts::CatalogRule>,
    /// Each connector the turn already reaches, at the exact manifest and
    /// implementation it binds: one run binds one revision of a name, and
    /// what reaches beyond these is what a load adds.
    pub held: Vec<runx_contracts::connector::ConnectorBinding>,
    /// Each `project:<name>` the run already binds, at its directory: one
    /// directory is each in a run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<runx_contracts::ProjectBinding>,
}

/// What a load names: a skill, with the one method it loads or all of them,
/// or a connector by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum LoadTarget {
    Skill {
        skill: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        method: Option<String>,
    },
    Connector {
        name: String,
    },
}

/// An agent turn: the task it performs, a `## Agent task: <name>` section of
/// the package's `SKILL.md`, which carries its instructions. The step's
/// `outputs` declare what the answer must contain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTask {
    pub task: String,
    /// What the turn may call: the step's `calls`. Writing them grants
    /// nothing.
    #[serde(default, skip_serializing_if = "TurnAllowance::is_empty")]
    pub allowance: TurnAllowance,
    /// The skill's requirements for the connectors the allowance's operations
    /// name, bound with it: what admits each operation call.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connectors: BTreeMap<String, crate::ConnectorRequirement>,
    /// What the turn needs from its agent harness: the step's `requires`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<runx_contracts::TurnRequirement>,
    /// The kinds of context the turn asks its host to supply: the step's
    /// `context.kinds` (N22, D31).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_kinds: Vec<String>,
    /// The earlier steps' outputs of context entries the turn is given: the
    /// step's `context.entries`, each a step's field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_entries: Vec<Source>,
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
    /// Helpers the module requires; unsupported names fail workflow admission.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<runx_contracts::javascript_worker::WorkerApi>,
    /// Relative ECMAScript module path inside the package.
    pub module: String,
    /// Named export; the module's default export when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export: Option<String>,
    /// The CPU time one invocation may spend, in seconds: 1 to 30, 2 when
    /// omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<u64>,
    /// Runtime-owned paging over one contained immutable file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paged_input: Option<ArtifactPageSource>,
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
    pub inputs_via: Option<InputMode>,
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
    pub reason: AskReason,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub gate_type: Option<String>,
}

/// What a person reads at an ask: its text, or a field of an earlier step's
/// output, read when the ask runs, so a gate shows the exact change it
/// decides. Text is written as a string, and a bound reason as the step and
/// path it reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum AskReason {
    Text(String),
    Step { step: String, path: Vec<String> },
}

/// A child skill run: the skill, as the step's reference names it, and the
/// method after its `#`, if it names one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillTarget {
    pub skill: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowStep {
    pub id: String,
    /// What the step does.
    pub kind: StepKind,
    /// The outputs kept as artifacts, from the step's `outputs` that name a
    /// packet and its `result_as`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<OutputArtifacts>,
    /// Complete output declarations owned by this step. Preserve these in
    /// typed IR so inspection, packet generation, and registry packaging do
    /// not have to reparse the raw manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<JsonObject>,
    /// The values the step is invoked with, as written: literals, with each
    /// reference left in place for `bindings` to fill. For an operation step
    /// these are the call's fields (`inputs`, `idempotency_key`, ...).
    pub inputs: JsonObject,
    /// Where `inputs` reads the run's input and earlier steps' results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
    /// The act an operation step's call performs, or the act a skill step's
    /// child run takes as its `$run.act`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<StepAct>,
    /// The skills whose instructions an agent turn reads before it starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_skills: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<WorkflowWhen>,
    /// What must hold before the step may run, all of it, or the run fails.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guard: Vec<WorkflowWhen>,
    /// Whether recovery may resend a write whose outcome is unknown,
    /// accepting that it may happen twice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retry_unknown_outcomes: bool,
    /// What the step may change: its `effects`, which only narrows its
    /// method's. A turn's calls run under their agent step's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<runx_contracts::Effects>,
    /// Where the step's tools may go: its `reach`, which only narrows its
    /// method's. A turn's calls run under their agent step's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<runx_contracts::Reach>,
}

impl WorkflowStep {
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

    /// The catalog tool name, for a catalog tool step.
    #[must_use]
    pub fn catalog_tool(&self) -> Option<&str> {
        match &self.kind {
            StepKind::Tool(ToolTarget::Catalog(reference)) => Some(reference),
            _ => None,
        }
    }

    /// The earlier steps whose results this step's values read, in order and
    /// once each, an ask's bound reason among them. Its condition is not among
    /// them: a condition selects the step, it is not its input.
    #[must_use]
    pub fn reads(&self) -> Vec<&str> {
        let mut steps = self.needs();
        let entries = match &self.kind {
            StepKind::Agent(task) => task.context_entries.as_slice(),
            _ => &[],
        };
        for step in entries.iter().filter_map(Source::step) {
            if !steps.contains(&step) {
                steps.push(step);
            }
        }
        steps
    }

    /// The earlier steps this step can't run without: what it reads, less
    /// the steps its turn's context entries come from, since a skipped step
    /// gives a turn no entries and the turn runs with none from it.
    #[must_use]
    pub fn needs(&self) -> Vec<&str> {
        let mut steps = Vec::new();
        let reason = match &self.kind {
            StepKind::Ask(AskGate {
                reason: AskReason::Step { step, .. },
                ..
            }) => Some(step.as_str()),
            _ => None,
        };
        for step in self
            .bindings
            .iter()
            .chain(self.act.iter().flat_map(|act| &act.bindings))
            .map(|binding| &binding.source)
            .filter_map(Source::step)
            .chain(reason)
        {
            if !steps.contains(&step) {
                steps.push(step);
            }
        }
        steps
    }
}

/// A step's `act:` as written: its literal fields, with each reference left
/// in place, and the bindings that fill them. `act: $run.act` is one binding
/// of the whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepAct {
    pub values: JsonObject,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
}

/// An operation step's target: the connector requirement it names and, unless
/// the step chooses its operation per run, one operation on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationStepTarget {
    pub requirement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationName>,
    /// The operations a per-run choice may name, when the step chooses one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<OperationName>,
    /// The requirement as the enclosing skill declares it, bound when the
    /// skill is parsed, or as a load of the turn bound it. A workflow outside a
    /// skill has none, and its operation steps are refused at admission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<crate::ConnectorRequirement>,
    /// The exact binding a load of the turn bound the requirement at, when
    /// one did: its connector is then the project's, where the load found it,
    /// never one the enclosing skill reaches, and the call runs only against
    /// that manifest and implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded: Option<runx_contracts::connector::ConnectorBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    /// Steps whose complete declared output contracts form the method's
    /// public result, the first listed that ran. Multiple entries support
    /// mutually exclusive terminal branches and methods that intentionally
    /// return both an act result and its durable readback.
    pub result_from: Vec<String>,
    /// What the method returns, when it says: its result producers' outputs
    /// narrowed to these fields, each checked against its declaration. With
    /// none, it returns its result producers' outputs whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<JsonObject>,
    /// What the method's run may change: its `effects`, which only narrows
    /// its host's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<runx_contracts::Effects>,
    /// Where the method's tools may go: its `reach`, which only narrows its
    /// host's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<runx_contracts::Reach>,
    pub steps: Vec<WorkflowStep>,
}
