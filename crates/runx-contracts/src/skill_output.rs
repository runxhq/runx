//! JSON envelope emitted by the native skill CLI, including compact artifacts.
//! Execution semantics remain owned by the runtime; this is a transport view.
use serde::{Deserialize, Serialize};

use crate::{JsonObject, JsonValue, schema::RunxSchema};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum SkillOutputSchema {
    #[serde(rename = "runx.skill_run.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillOutputStatus {
    Sealed,
    NeedsAgent,
    NeedsApproval,
    PaymentRequired,
    PolicyDenied,
    Failure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillOutputOutcome {
    Completed,
    Deferred,
    Blocked,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[runx_schema(id = "runx.skill_run.v1")]
pub struct SkillOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SkillOutputSchema>,
    pub status: SkillOutputStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<SkillOutputOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<Vec<JsonValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_set_ref: Option<ProjectArtifactRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics_ref: Option<ProjectArtifactRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_refs: Option<Vec<ProjectArtifactRef>>,
    // Native execution fields are intentionally preserved, not reinterpreted.
    #[serde(flatten)]
    pub fields: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ProjectArtifactRefSchema {
    #[serde(rename = "runx.project_artifact_ref.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.project_artifact_ref.v1")]
pub struct ProjectArtifactRef {
    pub schema: ProjectArtifactRefSchema,
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub digest: String,
    pub bytes: u64,
    pub media_type: String,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::{ProjectArtifactRef, SkillOutput};

    #[test]
    fn compact_reference_uses_the_native_wire_field() -> Result<(), serde_json::Error> {
        let value = serde_json::json!({
            "schema": "runx.project_artifact_ref.v1",
            "ref": "runx:project-artifact:sha256:abc",
            "digest": "sha256:abc",
            "bytes": 12,
            "media_type": "application/json",
            "path": "/project/.runx/artifacts/request.json"
        });
        let reference: ProjectArtifactRef = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(reference)?, value);
        Ok(())
    }

    #[test]
    fn transport_preserves_native_execution_fields_and_raw_failures()
    -> Result<(), serde_json::Error> {
        for value in [
            serde_json::json!({"status": "failure", "error": {"code": "skill_error"}}),
            serde_json::json!({"schema": "runx.skill_run.v1", "status": "sealed", "outcome": "failed", "closure": {"disposition": "failed"}}),
            serde_json::json!({"schema": "runx.skill_run.v1", "status": "payment_required", "outcome": "deferred", "payment": {"challenge": "native"}}),
        ] {
            let output: SkillOutput = serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(output)?, value);
        }
        Ok(())
    }
}
