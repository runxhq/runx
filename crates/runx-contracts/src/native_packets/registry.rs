//! What the native registry tools return.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;

/// What a skill or connector is, as the catalog lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogPackageKind {
    Skill,
    Connector,
}

/// Where a package is: the registry publishes it, or the project authored
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSource {
    Registry,
    Project,
}

/// One package `registry.search` found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogHit {
    /// What a load and `registry.inspect` take: the registry package
    /// (`<publisher>/<name>@<version>`), or the project package's path in
    /// the workspace.
    pub package: String,
    pub kind: CatalogPackageKind,
    pub source: CatalogSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_tier: Option<crate::TrustTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// A skill's runners, each with its purpose when its source states one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runners: Vec<CatalogRunner>,
}

/// One runner of a skill, as a model chooses it from an index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogRunner {
    pub name: String,
    /// What it does and when to use it, as its package states it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

/// What `registry.search` found for a query, in the project and the
/// registry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.registry.search.v1",
    url = "https://schemas.runx.ai/runx/registry/search/v1.json"
)]
pub struct RegistrySearchPacket {
    pub query: String,
    pub hits: Vec<CatalogHit>,
}

/// One operation of a connector, as a caller decides on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectedOperation {
    pub operation: String,
    pub description: String,
    /// `read` or `mutate`, when the project holds the connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    /// Whether every call waits on the person's approval of its exact input,
    /// when the project holds the connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_required: Option<bool>,
}

/// A connector as `registry.inspect` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorInspection {
    pub connector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub description: String,
    pub source: CatalogSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_tier: Option<crate::TrustTier>,
    pub operations: Vec<InspectedOperation>,
    /// Each operation as ready as the project leaves it; none for a
    /// connector the project does not hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness: Vec<crate::Readiness>,
}

/// What `registry.inspect` reports: a skill's inspection
/// (`runx.skill.inspect.v1`), or a connector's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(untagged)]
#[runx_schema(
    id = "runx.registry.inspection.v1",
    url = "https://schemas.runx.ai/runx/registry/inspection/v1.json"
)]
pub enum RegistryInspection {
    Skill(Box<crate::SkillInspection>),
    Connector(ConnectorInspection),
}
