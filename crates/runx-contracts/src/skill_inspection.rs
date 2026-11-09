//! A skill package's inspection (`runx.skill.inspect.v1`, N12): what `runx
//! skill inspect` and a turn's `registry.inspect` report of a package, by
//! method, without running it. The execution closure and the catalog's
//! semantic report keep the shapes their owners give them.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{ExecutionRequirements, JsonObject, Readiness};

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
    pub catalog: Option<SkillCatalog>,
    /// Every method the package declares, by name.
    pub methods: Vec<String>,
    /// Each method with the execution closure it binds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub method_inspections: Vec<MethodInspectionEntry>,
    /// The method inspected: the one named, else the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<MethodInspection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_closure: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<InspectionReadiness>,
    /// Each operation the method may call, as ready as the project leaves it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connector_readiness: Vec<Readiness>,
    /// The environment the method reads, by name and availability, never by
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

/// How a catalog files the package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillCatalog {
    pub visibility: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct MethodInspectionEntry {
    pub method: MethodInspection,
    pub execution_closure: JsonObject,
}

/// A method's input and output JSON Schemas, derived from its fields, and the
/// canonical digest of each: what a paid method's listing, purchases and
/// receipts bind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct MethodSchemas {
    pub input_schema: JsonObject,
    pub input_schema_digest: crate::Digest,
    pub output_schema: JsonObject,
    pub output_schema_digest: crate::Digest,
}

/// One method as a caller reads it: what it takes, what it returns, and
/// what it needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct MethodInspection {
    pub name: String,
    /// Its inputs as one JSON schema, with the method's examples.
    pub input_schema: JsonObject,
    /// Its derived input and output schemas and their digests.
    pub schemas: MethodSchemas,
    pub inputs: Vec<InspectedInput>,
    pub outputs: Vec<InspectedOutput>,
    /// The packet each result output declares, by output name, the whole
    /// result's under the name of the output it is.
    pub packets: BTreeMap<String, crate::PacketId>,
    pub requirements: ExecutionRequirements,
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
