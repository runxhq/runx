//! Public packets emitted by reusable native runtime capabilities.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{JsonObject, JsonValue};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.local_artifact.v1",
    url = "https://schemas.runx.ai/runx/local-artifact/v1.json"
)]
pub struct LocalArtifact {
    pub artifact_ref: String,
    pub media_type: String,
    pub bytes: u64,
    pub whole_digest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.local_artifact.page.v1",
    url = "https://schemas.runx.ai/runx/local-artifact/page/v1.json"
)]
pub struct LocalArtifactPage {
    pub artifact_ref: String,
    pub media_type: String,
    pub offset: u64,
    pub length: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub range_digest: String,
    pub whole_digest: String,
    pub encoding: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.git.blob_digest.v1",
    url = "https://schemas.runx.ai/runx/git/blob-digest/v1.json"
)]
pub struct GitBlobDigest {
    pub algorithm: String,
    pub digest: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.data.operation_result.v1",
    url = "https://schemas.runx.ai/runx/data/operation-result/v1.json"
)]
pub struct DataOperationResult {
    pub schema: String,
    pub data_source_ref: String,
    pub provider: String,
    pub operation: String,
    pub resource: String,
    pub aggregate_id: String,
    pub status: DataOperationStatus,
    pub before_version: u64,
    pub after_version: u64,
    pub idempotency_key: Option<String>,
    pub event_ref: Option<String>,
    pub event_digest: Option<String>,
    pub result_digest: String,
    pub projection_digest: String,
    pub projection: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_after_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    pub events: Vec<JsonValue>,
    pub rows: Vec<JsonValue>,
    pub redactions: Vec<JsonValue>,
    pub stop_conditions: Vec<DataStopCondition>,
    pub provider_evidence: JsonObject,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataOperationStatus {
    Committed,
    IdempotentReplay,
    Read,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct DataStopCondition {
    pub code: String,
    pub message: String,
}

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
