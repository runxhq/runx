//! What a native approval gate records.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.approval.decision.v1",
    url = "https://schemas.runx.ai/runx/approval/decision/v1.json"
)]
pub struct ApprovalDecisionPacket {
    pub approved: Option<bool>,
    pub gate_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_type: Option<String>,
    pub idempotency_key: String,
    pub status: ApprovalDecisionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<ApprovalDecisionActor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionStatus {
    Approved,
    Denied,
    Pending,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionActor {
    Human,
    Agent,
}
