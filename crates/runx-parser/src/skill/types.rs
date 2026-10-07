// Module rationale: skill schema vocabulary is intentionally
// centralized so parser fixtures, contract mirrors, and runtime front validation
// share one typed source of truth.
use std::collections::BTreeMap;

use runx_contracts::{EnvironmentRequirements, ExecutionSemantics, JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::graph::MintAuthorityDirective;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSkillIr {
    pub frontmatter: JsonObject,
    pub raw_frontmatter: String,
    pub body: String,
}

pub use runx_contracts::{
    ArtifactContract as SkillArtifactContract, IdempotencyPolicy as SkillIdempotencyPolicy,
    InputDefinition as SkillInput, RetryPolicy as SkillRetryPolicy,
};

/// Closed set of built-in skill source kinds. `source.type` is never an open
/// string: every runner resolves to one of these variants at parse time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    CliTool,
    JavaScript,
    Agent,
    #[serde(rename = "agent-task")]
    AgentStep,
    Graph,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::CliTool => "cli-tool",
            SourceKind::JavaScript => "javascript",
            SourceKind::Agent => "agent",
            SourceKind::AgentStep => "agent-task",
            SourceKind::Graph => "graph",
        }
    }
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSource {
    #[serde(rename = "type")]
    pub source_type: SourceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Relative ECMAScript module path for a runtime-owned `javascript`
    /// invocation. The selected export maps resolved inputs to a JSON-compatible
    /// result; Runx owns the process protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Named function exported by a `javascript` module. Omitted means the
    /// module's default export.
    #[serde(rename = "export", skip_serializing_if = "Option::is_none")]
    pub javascript_export: Option<String>,
    /// Runtime-owned paging for a contained immutable file. The path fields are
    /// consumed by the runtime and never enter the deterministic module.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<ArtifactPageSource>,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<InputMode>,
    /// Explicit configuration names consumed by this executable source.
    /// Values are resolved by the runtime and never stored in the manifest IR.
    #[serde(default, skip_serializing_if = "EnvironmentRequirements::is_empty")]
    pub environment: EnvironmentRequirements,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<crate::ExecutionGraph>,
    /// The declared act this source performs, validated at load. `None` when no
    /// `act:` block is declared (the run then seals a generic observation act).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<ActDeclaration>,
    pub raw: JsonObject,
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
    /// Charter attenuation: the input key carrying the member's own authority term
    /// (the child grant minted from the charter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_term_from: Option<String>,
    /// The input key carrying the parent (charter) authority reference the child
    /// term attenuates from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_parent_from: Option<String>,
    /// The input key carrying the subset proof that the child term is no broader
    /// than the parent charter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_subset_proof_from: Option<String>,
    /// Compute path (mutually exclusive with the explicit `authority_*_from` keys
    /// above): when present, the runtime mints the child authority term from the
    /// charter off the model path, from the requested scope this directive names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mint_authority: Option<MintAuthorityDirective>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedSkill {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runx_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_owner: Option<String>,
    pub body: String,
    pub source: SkillSource,
    pub inputs: BTreeMap<String, SkillInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<SkillRetryPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency: Option<SkillIdempotencyPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<SkillArtifactContract>,
    pub allowed_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionSemantics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runx: Option<JsonObject>,
    pub raw: RawSkillIr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRunnerDefinition {
    pub name: String,
    pub default: bool,
    pub source: SkillSource,
    pub inputs: BTreeMap<String, SkillInput>,
    /// Complete, parser-validated calls for this runner. These are contract
    /// examples, not fixtures: inspection and generated operator instructions
    /// expose them without reparsing package files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<JsonObject>,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<SkillRetryPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency: Option<SkillIdempotencyPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<SkillArtifactContract>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionSemantics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runx: Option<JsonObject>,
    pub raw: JsonObject,
}

impl SkillRunnerDefinition {
    /// Return the complete scope ceiling declared by this runner and its inline
    /// graph steps. Consumers must use this typed projection rather than scan
    /// the raw manifest tree independently.
    #[must_use]
    pub fn declared_scopes(&self) -> Vec<String> {
        let mut scopes = self.scopes.clone();
        if let Some(graph) = &self.source.graph {
            for step in &graph.steps {
                scopes.extend(step.scopes.clone());
            }
        }
        scopes
    }
}
