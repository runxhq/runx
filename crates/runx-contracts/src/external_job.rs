//! Portable durable-external-job continuation contract.
//!
//! The contract records provider-neutral execution progress only. Hosted
//! persistence, leases, credentials, SDK values, and provider response bodies
//! stay behind Cloud adapters.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::schema::{BoundedString, IsoDateTime, NamedSchemas, RunxSchema, record_named};
use crate::{Digest, PaidSkillExecutorBinding, PrincipalReference, Reference};

pub const EXTERNAL_JOB_CONTINUATION_SCHEMA: &str = "runx.external_job_continuation.v1";
pub const EXTERNAL_JOB_SCHEDULE_SCHEMA: &str = "runx.external_job_schedule.v1";
pub const EXTERNAL_JOB_SCHEDULE_INTENT_SCHEMA: &str = "runx.external_job_schedule_intent.v1";
pub const EXTERNAL_JOB_STAGE_REQUEST_SCHEMA: &str = "runx.external_job_stage.request.v1";
pub const EXTERNAL_JOB_STAGE_RESULT_SCHEMA: &str = "runx.external_job_stage.result.v1";
const EXTERNAL_JOB_CHECKPOINT_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExternalJobStage {
    Start,
    Inspect,
    Materialize,
    Finalize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExternalJobState {
    Runnable,
    WaitingExternal,
    Succeeded,
    Failed,
    Superseded,
    DeadLetter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ExternalJobScheduleIntentSchema {
    #[serde(rename = "runx.external_job_schedule_intent.v1")]
    V1,
}

/// Bounded for durable scheduling while allowing multi-hour provider jobs to
/// use a five-minute steady-state inspection cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExternalJobAttemptLimit(u8);

impl ExternalJobAttemptLimit {
    pub fn new(value: u8) -> Option<Self> {
        (1..=128).contains(&value).then_some(Self(value))
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ExternalJobAttemptLimit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("max_attempts must be between 1 and 128"))
    }
}

impl RunxSchema for ExternalJobAttemptLimit {
    fn json_schema() -> Value {
        json!({ "type": "integer", "minimum": 1, "maximum": 128 })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExternalJobDelayMillis(u32);

impl<'de> Deserialize<'de> for ExternalJobDelayMillis {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        (value <= 300_000).then_some(Self(value)).ok_or_else(|| {
            de::Error::custom("external job delay must not exceed 300000 milliseconds")
        })
    }
}

impl RunxSchema for ExternalJobDelayMillis {
    fn json_schema() -> Value {
        json!({ "type": "integer", "minimum": 0, "maximum": 300_000 })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExternalJobDeadlineMillis(u32);

impl<'de> Deserialize<'de> for ExternalJobDeadlineMillis {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        (1_000..=86_400_000)
            .contains(&value)
            .then_some(Self(value))
            .ok_or_else(|| {
                de::Error::custom(
                    "external job deadline must be between 1000 and 86400000 milliseconds",
                )
            })
    }
}

impl RunxSchema for ExternalJobDeadlineMillis {
    fn json_schema() -> Value {
        json!({ "type": "integer", "minimum": 1_000, "maximum": 86_400_000 })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalJobFailure {
    pub code: BoundedString<96>,
    pub message: BoundedString<500>,
    pub retryable: bool,
}

/// Private package state only. Credentials, SDK values, presigned URLs, and
/// raw provider responses never belong here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExternalJobCheckpoint(BTreeMap<String, Value>);

impl<'de> Deserialize<'de> for ExternalJobCheckpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = BTreeMap::<String, Value>::deserialize(deserializer)?;
        let encoded = serde_json::to_vec(&value).map_err(de::Error::custom)?;
        if encoded.len() > EXTERNAL_JOB_CHECKPOINT_MAX_BYTES {
            return Err(de::Error::custom(
                "external job checkpoint exceeds 65536 bytes",
            ));
        }
        Ok(Self(value))
    }
}

impl RunxSchema for ExternalJobCheckpoint {
    fn json_schema() -> Value {
        // Package checkpoints are intentionally open, but the openness must be
        // explicit so packet-schema drift checks cannot mistake it for an
        // unspecified envelope shape.
        json!({ "type": "object", "additionalProperties": true, "title": "ExternalJobCheckpoint" })
    }

    fn named_schemas(named: &mut NamedSchemas) {
        record_named(named, "ExternalJobCheckpoint", Self::json_schema);
    }
}

/// Where one purchased external job stands: its stage and state, its
/// attempts against their limit and deadline, and how it ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.external_job_continuation.v1")]
pub struct ExternalJobContinuation {
    pub continuation_id: BoundedString<256>,
    pub principal_ref: PrincipalReference,
    pub seller_ref: PrincipalReference,
    pub purchase_ref: Reference,
    pub source_run_ref: Reference,
    pub executor_binding: PaidSkillExecutorBinding,
    pub operation_identity: Digest,
    pub stage: ExternalJobStage,
    pub state: ExternalJobState,
    pub attempts: u32,
    pub max_attempts: ExternalJobAttemptLimit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<IsoDateTime>,
    pub deadline_at: IsoDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_job_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_artifact_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_receipt_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_evidence_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_evidence_digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<ExternalJobFailure>,
    /// The most recent retryable failure while the job is still active, so a
    /// stalled continuation is diagnosable before it dead-letters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<ExternalJobFailure>,
    pub created_at: IsoDateTime,
    pub updated_at: IsoDateTime,
}

/// Durable outbox payload used to create one continuation exactly once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.external_job_schedule.v1")]
pub struct ExternalJobSchedule {
    pub continuation_id: BoundedString<256>,
    pub principal_ref: PrincipalReference,
    pub seller_ref: PrincipalReference,
    pub purchase_ref: Reference,
    pub source_run_ref: Reference,
    pub executor_binding: PaidSkillExecutorBinding,
    pub operation_identity: Digest,
    pub checkpoint: ExternalJobCheckpoint,
    pub max_attempts: ExternalJobAttemptLimit,
    pub next_attempt_at: IsoDateTime,
    pub deadline_at: IsoDateTime,
    pub created_at: IsoDateTime,
}

/// Product-owned opt-in emitted by the initial execution package. Trusted
/// purchase identity and absolute scheduling timestamps are added by Runx.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.external_job_schedule_intent.v1")]
pub struct ExternalJobScheduleIntent {
    pub schema: ExternalJobScheduleIntentSchema,
    /// Runner in the same immutable package that owns continuation stages.
    /// Runx resolves and pins its full execution closure before scheduling.
    pub stage_runner: BoundedString<128>,
    pub checkpoint: ExternalJobCheckpoint,
    pub max_attempts: ExternalJobAttemptLimit,
    pub initial_delay_ms: ExternalJobDelayMillis,
    pub deadline_ms: ExternalJobDeadlineMillis,
}

/// Exact package re-entry input for one persisted stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.external_job_stage.request.v1")]
pub struct ExternalJobStageRequest {
    pub continuation: ExternalJobContinuation,
    pub checkpoint: ExternalJobCheckpoint,
    pub operation_key: Digest,
}

/// Provider-neutral package result. Runx owns scheduling and finalization;
/// the package owns only the meaning of each stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[runx_schema(id = "runx.external_job_stage.result.v1")]
pub enum ExternalJobStageResult {
    Waiting {
        provider_job_ref: Reference,
        checkpoint: ExternalJobCheckpoint,
        retry_after_ms: ExternalJobDelayMillis,
    },
    Materialize {
        provider_job_ref: Reference,
        checkpoint: ExternalJobCheckpoint,
        retry_after_ms: ExternalJobDelayMillis,
    },
    Ready {
        provider_job_ref: Reference,
        result_artifact_ref: Reference,
        evidence_ref: Reference,
        evidence_digest: Digest,
    },
    ProviderFailed {
        provider_job_ref: Reference,
        evidence_ref: Reference,
        evidence_digest: Digest,
        failure: ExternalJobFailure,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_limit_is_bounded() {
        assert!(ExternalJobAttemptLimit::new(1).is_some());
        assert!(ExternalJobAttemptLimit::new(128).is_some());
        assert!(ExternalJobAttemptLimit::new(0).is_none());
        assert!(ExternalJobAttemptLimit::new(129).is_none());
    }
}
