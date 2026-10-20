// Module rationale: skill schema vocabulary is intentionally
// centralized so parser fixtures, contract mirrors, and runtime front validation
// share one typed source of truth.
use std::collections::BTreeMap;

use runx_contracts::{EnvironmentRequirements, JsonObject};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSkillIr {
    pub frontmatter: JsonObject,
    pub raw_frontmatter: String,
    pub body: String,
}

pub use runx_contracts::{
    ArtifactContract as SkillArtifactContract, IdempotencyPolicy as SkillIdempotencyPolicy,
    InputDefinition as SkillInput,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    Args,
    Stdin,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPageFraming {
    JsonArray,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPageSource {
    pub path_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_scope_from: Option<String>,
    pub media_type: String,
    pub framing: ArtifactPageFraming,
    pub page_bytes: u64,
}

impl InputMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            InputMode::Args => "args",
            InputMode::Stdin => "stdin",
            InputMode::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidateSkillMode {
    Strict,
    Lenient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidateSkillOptions {
    pub mode: ValidateSkillMode,
}

impl Default for ValidateSkillOptions {
    fn default() -> Self {
        Self {
            mode: ValidateSkillMode::Strict,
        }
    }
}

impl ValidateSkillOptions {
    #[must_use]
    pub const fn strict() -> Self {
        Self {
            mode: ValidateSkillMode::Strict,
        }
    }

    #[must_use]
    pub const fn lenient() -> Self {
        Self {
            mode: ValidateSkillMode::Lenient,
        }
    }
}

/// A validated `SKILL.md`: the operator manual.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedSkill {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub body: String,
    pub raw: RawSkillIr,
}

/// One runner: the steps it runs and its inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRunnerDefinition {
    pub name: String,
    /// What the runner does and when to use it rather than the skill's other
    /// runners: the purpose a model chooses it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub default: bool,
    /// Same-package runners that may fulfil this runner after its turn ends.
    /// They are part of its pinned execution closure, not implicit execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub continuations: Vec<String>,
    pub inputs: BTreeMap<String, SkillInput>,
    /// Complete, parser-validated calls for this runner. These are contract
    /// examples, not fixtures: inspection and generated operator instructions
    /// expose them without reparsing package files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<JsonObject>,
    /// The runner's steps, as the graph the engine runs. Its name is the
    /// runner's.
    pub graph: crate::ExecutionGraph,
    pub raw: JsonObject,
}

impl SkillRunnerDefinition {
    /// The complete scope ceiling declared by this runner's steps. Consumers
    /// must use this typed projection rather than scan the raw manifest tree.
    #[must_use]
    pub fn declared_scopes(&self) -> Vec<String> {
        self.graph
            .steps
            .iter()
            .flat_map(|step| step.scopes.iter().cloned())
            .collect()
    }

    /// The artifact contracts of the steps whose outputs form the runner's
    /// public result.
    #[must_use]
    pub fn result_artifacts(&self) -> Vec<&SkillArtifactContract> {
        self.graph
            .steps
            .iter()
            .filter(|step| self.graph.result_from.contains(&step.id))
            .filter_map(|step| step.artifacts.as_ref())
            .collect()
    }

    /// The environment requirements of the runner's worker and process steps.
    #[must_use]
    pub fn environment(&self) -> EnvironmentRequirements {
        let mut environment = EnvironmentRequirements::default();
        for step in &self.graph.steps {
            match &step.kind {
                crate::StepKind::Tool(crate::ToolTarget::Process(process)) => {
                    environment.extend(&process.environment);
                }
                crate::StepKind::Tool(crate::ToolTarget::Worker(worker)) => {
                    environment.extend(&worker.environment);
                }
                _ => {}
            }
        }
        environment
    }
}
