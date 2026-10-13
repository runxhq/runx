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
#[serde(deny_unknown_fields)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    /// How the run's seal closed it: its disposition and reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closure: Option<JsonObject>,
    /// A failed run's exit code and message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonObject>,
    /// Why a guard or an authority refused the run, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<JsonObject>,
    /// The run's operator context and step trace, written only with
    /// `--diagnostics`; otherwise they live behind `diagnostics_ref`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<JsonValue>,
    /// The command that continues a paused run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// Where the skill the run started came from, as the CLI resolved it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_provenance: Option<JsonObject>,
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
    fn the_envelope_carries_its_declared_fields_and_refuses_others() -> Result<(), serde_json::Error>
    {
        for value in [
            serde_json::json!({"status": "failure", "error": {"code": "skill_error"}}),
            serde_json::json!({"schema": "runx.skill_run.v1", "status": "sealed", "outcome": "failed", "skill_name": "echo", "runner": "default", "closure": {"disposition": "failed"}}),
            serde_json::json!({"schema": "runx.skill_run.v1", "status": "needs_agent", "run_id": "rx_1", "requests": [], "next": "runx resume rx_1 - --json", "entry_provenance": {"kind": "local"}}),
            serde_json::json!({"schema": "runx.skill_run.v1", "status": "payment_required", "outcome": "deferred", "result": {"payment_required": {}}}),
        ] {
            let output: SkillOutput = serde_json::from_value(value.clone())?;
            assert_eq!(serde_json::to_value(output)?, value);
        }
        assert!(
            serde_json::from_value::<SkillOutput>(
                serde_json::json!({"status": "sealed", "payment": {"challenge": "native"}})
            )
            .is_err()
        );
        Ok(())
    }
}
