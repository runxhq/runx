//! Exact, non-secret requirements declared by one executable skill act.
//!
//! These values are transport contracts. Runx validates their shape and
//! preserves their values; provider adapters and workers decide
//! how an admitted requirement is fulfilled.

use serde::{Deserialize, Serialize};

use crate::JsonValue;
use crate::schema::RunxSchema;

/// Environment names consumed by an executable act. Values are resolved only
/// at execution time and never enter this declaration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRequirements {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional: Vec<String>,
}

impl EnvironmentRequirements {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.required.is_empty() && self.optional.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.required
            .iter()
            .chain(&self.optional)
            .map(String::as_str)
    }
}

/// Parser-owned projection of all execution requirements for one act.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRequirements {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "EnvironmentRequirements::is_empty")]
    pub environment: EnvironmentRequirements,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<JsonValue>,
}

/// Safe environment readiness visible to an operating agent. The value is
/// deliberately absent: models receive the requirement and its availability,
/// never ambient or credential material.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRequirementStatus {
    pub name: String,
    pub required: bool,
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentExecutionRequirements {
    pub declaration: ExecutionRequirements,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<EnvironmentRequirementStatus>,
    pub execution_boundary: crate::ExecutionBoundaryObservation,
}
