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
    ArtifactDeclaration as SkillArtifactContract, IdempotencyPolicy as SkillIdempotencyPolicy,
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
    pub source: ArtifactPageInput,
    pub framing: ArtifactPageFraming,
    pub page_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactPageInput {
    /// A file, named by the input `path`, inside the directory the input
    /// `path_scope` names.
    Path {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        path_scope: Option<String>,
        media_type: String,
    },
    /// A stored artifact, referenced by the input `artifact`.
    Artifact { artifact: String },
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

/// One method: the steps it runs and its inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillMethodDefinition {
    pub name: String,
    /// What the method does and when to use it rather than the skill's other
    /// methods: the purpose a model chooses it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub inputs: BTreeMap<String, SkillInput>,
    /// Complete, parser-validated calls for this method. These are contract
    /// examples, not tests: inspection and generated operator instructions
    /// expose them without reparsing package files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<JsonObject>,
    /// What the method charges, when it's sold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<runx_contracts::PaidSkillOfferTerms>,
    /// The method's steps, as the workflow the engine runs. Its name is the
    /// skill's.
    pub graph: crate::Workflow,
    pub raw: JsonObject,
}

impl SkillMethodDefinition {
    /// The method's input schema: every input field, with which are required.
    #[must_use]
    pub fn input_schema(&self) -> JsonObject {
        runx_contracts::input_contract_schema(&self.inputs)
    }

    /// The method's output schema: its result step's outputs, or one of
    /// them, when the method returns the first of several that ran.
    #[must_use]
    pub fn output_schema(&self) -> JsonObject {
        let mut results = self
            .graph
            .steps
            .iter()
            .filter(|step| self.graph.result_from.contains(&step.id))
            .map(|step| {
                let fields = step
                    .outputs
                    .as_ref()
                    .and_then(|outputs| runx_contracts::parse_output_contract(outputs).ok());
                match runx_contracts::output_value_schema(fields.as_ref()) {
                    runx_contracts::JsonValue::Object(schema) => schema,
                    _ => JsonObject::new(),
                }
            })
            .collect::<Vec<_>>();
        match results.len() {
            1 => results.remove(0),
            _ => JsonObject::from([(
                "anyOf".to_owned(),
                runx_contracts::JsonValue::Array(
                    results
                        .into_iter()
                        .map(runx_contracts::JsonValue::Object)
                        .collect(),
                ),
            )]),
        }
    }

    /// Both schemas and their canonical digests.
    pub fn schemas(
        &self,
    ) -> Result<runx_contracts::MethodSchemas, runx_contracts::connector::ConnectorDigestError>
    {
        let input_schema = self.input_schema();
        let output_schema = self.output_schema();
        Ok(runx_contracts::MethodSchemas {
            input_schema_digest: runx_contracts::connector::stable_digest(&input_schema)?,
            output_schema_digest: runx_contracts::connector::stable_digest(&output_schema)?,
            input_schema,
            output_schema,
        })
    }

    /// The artifact contracts of the steps whose outputs form the method's
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

    /// The environment requirements of the method's worker and process steps.
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
