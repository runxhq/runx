//! What the native data tools return.

use serde::{Deserialize, Serialize};

use crate::digest::DigestAlgorithm;
use crate::schema::RunxSchema;
use crate::{Digest, JsonObject, JsonValue};

/// What `data.digest` returns: the SHA-256 digest of canonical JSON or of
/// exact UTF-8 text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.data.digest.v1")]
pub struct DataDigest {
    pub algorithm: DigestAlgorithm,
    pub digest: Digest,
}

/// What `data.compare` returns: whether two resolved values are equal, and
/// the digest of each, without copying either into the output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.data.comparison.v1")]
pub struct DataComparison {
    pub equal: bool,
    pub actual_digest: Digest,
    pub expected_digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.data.validation.v1")]
pub struct DataValidation {
    pub valid: bool,
    pub schema_digest: String,
    pub values_digest: String,
    pub results: Vec<DataValueValidation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct DataValueValidation {
    pub index: usize,
    pub valid: bool,
    pub errors: Vec<DataValidationIssue>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct DataValidationIssue {
    pub instance_path: String,
    pub schema_path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.data.operation_result.v1",
    url = "https://schemas.runx.ai/runx/data/operation-result/v1.json"
)]
pub struct DataOperationResult {
    pub schema: String,
    pub data_source_uri: String,
    pub provider: String,
    pub operation: String,
    pub resource: String,
    pub aggregate_id: String,
    pub status: DataOperationStatus,
    pub before_version: u64,
    pub after_version: u64,
    pub idempotency_key: Option<String>,
    pub event_id: Option<String>,
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
