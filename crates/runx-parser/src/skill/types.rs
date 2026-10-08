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

/// A skill's declared act: how a run describes the act it performs. The form and
/// purpose, and how the target, decision, effect, actor, authority, and previous
/// receipt are read, where `<field>_from` names a trusted input key and the bare
/// field is a static literal (the driver-pinned input wins). The runtime fills
/// the act's structure from these and the trusted inputs; the model authors only
/// the reason prose. Absent an `act:` block, a run seals a generic observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActDeclaration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legitimacy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legitimacy_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_from: Option<String>,
    /// The input key carrying the reason prose, authored by the calling operator
    /// (mutually exclusive with `reason_step`/`reason_from`): the operator agent
    /// holds the context and voice; runners must not embed an agent step just to
    /// write the sealed reason line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_from_input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_field_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_from_input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_prefix_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_from: Option<String>,
    /// Graph turns only: the step whose output supplies the reason prose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_step: Option<String>,
    /// Graph turns only: the step whose real result supplies the governed effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_step: Option<String>,
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

/// One runner: the steps it runs, its inputs, and the act it performs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRunnerDefinition {
    pub name: String,
    pub default: bool,
    pub inputs: BTreeMap<String, SkillInput>,
    /// Complete, parser-validated calls for this runner. These are contract
    /// examples, not fixtures: inspection and generated operator instructions
    /// expose them without reparsing package files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<JsonObject>,
    /// The act this runner performs. Absent, a run seals a generic
    /// observation act.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<ActDeclaration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runx: Option<JsonObject>,
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
