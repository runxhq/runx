//! A skill package's inspection (`runx.skill.inspect.v1`, N12): what `runx
//! skill inspect` and a turn's `registry.inspect` report of a package, by
//! runner, without running it. The execution closure and the catalog's
//! semantic report keep the shapes their owners give them.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{ExecutionRequirements, JsonObject, JsonValue, Readiness};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum SkillInspectionSchema {
    #[serde(rename = "runx.skill.inspect.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillInspectionStatus {
    Ok,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.inspect.v1")]
pub struct SkillInspection {
    pub schema: SkillInspectionSchema,
    pub status: SkillInspectionStatus,
    pub name: String,
    pub skill_path: String,
    pub manual_digest: String,
    /// The execution package digest, as a run records it; a listing's
    /// package digest is the registry's digest of the published files.
    pub package_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<SkillCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<SkillCatalog>,
    /// The catalog's semantic analysis of the package, as the parser makes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_report: Option<JsonValue>,
    /// Every runner the package declares, by name.
    pub runners: Vec<String>,
    pub operator_journeys: Vec<OperatorJourney>,
    /// Each runner with the execution closure it binds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runner_inspections: Vec<RunnerInspectionEntry>,
    /// The runner inspected: the one named, else the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<RunnerInspection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_closure: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<InspectionReadiness>,
    /// Each operation the runner may call, as ready as the project leaves it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connector_readiness: Vec<Readiness>,
    /// The environment the runner reads, by name and availability, never by
    /// value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentReadiness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<InspectionResume>,
    /// Where the inspected package came from, when a resolver found it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_provenance: Option<JsonObject>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillCapabilities {
    pub execution: String,
    pub completion: String,
    pub approval: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillCatalog {
    pub kind: String,
    pub audience: String,
    pub visibility: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_skill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_of: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorJourney {
    pub case: String,
    pub mode: String,
    pub request: String,
    pub expected_outcome: String,
    pub confusors: Vec<String>,
    pub prior_evidence: Vec<String>,
    pub must_not_repeat: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exercises_runner: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunnerInspectionEntry {
    pub runner: RunnerInspection,
    pub execution_closure: JsonObject,
}

/// One runner as a caller reads it: what it takes, what it returns, and
/// what it needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunnerInspection {
    pub name: String,
    /// Its inputs as one JSON schema, with the runner's examples.
    pub input_schema: JsonObject,
    pub inputs: Vec<InspectedInput>,
    pub outputs: Vec<InspectedOutput>,
    /// The packet each result output declares, by output name.
    pub declared_result_packets: BTreeMap<String, String>,
    pub requirements: ExecutionRequirements,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// The runners this one's work continues in, which its run may admit.
    #[serde(default)]
    pub continuations: Vec<String>,
    /// The connector requirements of the skill, by key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connectors: Option<JsonObject>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectedInput {
    pub name: String,
    #[serde(rename = "type")]
    pub input_type: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schema: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectedOutput {
    pub name: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub output_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum InspectionReadinessStatus {
    Ready,
    NeedsConnectorSetup,
    NeedsEnvironment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectionReadiness {
    pub status: InspectionReadinessStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentReadiness {
    pub status: EnvironmentReadinessStatus,
    pub variables: Vec<crate::EnvironmentRequirementStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentReadinessStatus {
    Ready,
    NeedsEnvironment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectionResume {
    pub may_pause: bool,
    pub command: String,
}
