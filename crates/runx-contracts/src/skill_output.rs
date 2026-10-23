//! JSON envelope emitted by the native skill CLI, including compact artifacts.
//! Execution semantics remain owned by the runtime; this is a transport view.
use serde::{Deserialize, Serialize};

use crate::{JsonObject, JsonValue, RunOutcome, SealReason, schema::RunxSchema};

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill_run.v1")]
pub struct SkillOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SkillOutputSchema>,
    pub status: SkillOutputStatus,
    /// How the run ended: its seal's outcome, or `deferred` while it waits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// Why the seal ended the run so, where the outcome alone does not say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seal_reason: Option<SealReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<Vec<JsonValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_set_artifact: Option<ProjectArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics_artifact: Option<ProjectArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<ProjectArtifact>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    /// A failed run's exit code and message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonObject>,
    /// Why a guard or an authority refused the run, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<JsonObject>,
    /// The run's operator context and step trace, written only with
    /// `--diagnostics`; otherwise they live behind `diagnostics_artifact`.
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
pub enum ProjectArtifactSchema {
    #[serde(rename = "runx.project_artifact.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.project_artifact.v1")]
pub struct ProjectArtifact {
    pub schema: ProjectArtifactSchema,
    /// `runx:project-artifact:` and the artifact's digest.
    pub uri: String,
    pub digest: String,
    pub bytes: u64,
    pub media_type: String,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::{ProjectArtifact, SkillOutput};

    #[test]
    fn a_project_artifact_uses_the_native_wire_fields() -> Result<(), serde_json::Error> {
        let value = serde_json::json!({
            "schema": "runx.project_artifact.v1",
            "uri": "runx:project-artifact:sha256:abc",
            "digest": "sha256:abc",
            "bytes": 12,
            "media_type": "application/json",
            "path": "/project/.runx/artifacts/request.json"
        });
        let artifact: ProjectArtifact = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(artifact)?, value);
        Ok(())
    }

    #[test]
    fn the_envelope_carries_its_declared_fields_and_refuses_others() -> Result<(), serde_json::Error>
    {
        for value in [
            serde_json::json!({"status": "failure", "error": {"code": "skill_error"}}),
            serde_json::json!({"schema": "runx.skill_run.v1", "status": "sealed", "outcome": "blocked", "seal_reason": "graph_blocked", "skill_name": "echo", "runner": "default"}),
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
