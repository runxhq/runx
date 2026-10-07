//! Business work proposals over native runs. Proposals convey no authority;
//! the operator admits only configured routes and validates their exact inputs.

use serde::{Deserialize, Serialize};

use crate::{JsonObject, schema::RunxSchema};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkProposal {
    pub route_id: String,
    pub target_ref: String,
    pub inputs: JsonObject,
    pub due_at_unix_seconds: u64,
    /// Relative scheduling for a successor emitted after an external effect.
    /// The operator resolves it against the time it admits that successor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_after_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkDisposition {
    Satisfied,
    FollowUp,
    Held,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.work_outcome.v1")]
pub struct WorkOutcome {
    pub disposition: WorkDisposition,
    pub summary: String,
    pub completion_evidence: Vec<String>,
    pub followups: Vec<WorkProposal>,
}
