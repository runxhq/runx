//! What the native policy tools return.

use serde::{Deserialize, Serialize};

use crate::operational_policy::{OperationalPolicyReadback, OperationalPolicyValidationFinding};
use crate::schema::RunxSchema;

/// Whether the policy parsed and linted clean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyLintStatus {
    Pass,
    Fail,
}

/// The engine that linted the policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum PolicyLintEngine {
    #[serde(rename = "runx policy")]
    RunxPolicy,
}

/// What `policy.lint` returns: the policy's findings, and its readback when
/// it parsed as a `runx.operational_policy.v1`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.policy.lint.v1")]
pub struct PolicyLint {
    pub status: PolicyLintStatus,
    pub engine: PolicyLintEngine,
    pub findings: Vec<OperationalPolicyValidationFinding>,
    pub readback: Option<OperationalPolicyReadback>,
}
