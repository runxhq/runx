use std::collections::BTreeMap;

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ContextArtifactMeta, ContextArtifactProducer, ContextEntry, ContextEntryVersion, JsonObject,
    JsonValue, sha256_prefixed,
};

use crate::RuntimeError;

const CONTEXT_ENTRY_TYPE: &str = "runx.skill.context";
const CONTEXT_PRODUCER_SKILL: &str = "runx-runtime";
const CONTEXT_PRODUCER_RUNNER: &str = "skill-context";
const PENDING_RUN_ID: &str = "rx_pending";

pub(super) struct SkillContextEntryInput<'a> {
    pub(super) step_id: &'a str,
    pub(super) reference: &'a str,
    pub(super) env: &'a BTreeMap<String, String>,
    pub(super) created_at: &'a str,
    pub(super) digest: &'a str,
    pub(super) size_bytes: u64,
    pub(super) data: JsonObject,
}

pub(super) fn skill_context_entry(
    input: SkillContextEntryInput<'_>,
) -> Result<ContextEntry, RuntimeError> {
    let artifact_id = sha256_prefixed(
        format!(
            "{CONTEXT_ENTRY_TYPE}\0{}\0{}",
            input.reference, input.digest
        )
        .as_bytes(),
    );
    Ok(ContextEntry {
        entry_type: Some(non_empty(CONTEXT_ENTRY_TYPE)?),
        version: ContextEntryVersion::V1,
        scope: runx_contracts::ContextScope {
            purpose: runx_contracts::ContextPurpose::Procedure,
            trust: runx_contracts::ContextTrust::SkillInstructions,
            sharing: runx_contracts::ContextSharing::ConfiguredModel,
            goal_id: None,
            source_ref: None,
        },
        data: input.data,
        meta: ContextArtifactMeta {
            artifact_id: non_empty(artifact_id)?,
            run_id: non_empty(
                input
                    .env
                    .get(crate::execution::runner::RUNX_RUN_ID_ENV)
                    .map(String::as_str)
                    .unwrap_or(PENDING_RUN_ID),
            )?,
            step_id: Some(non_empty(input.step_id)?),
            producer: ContextArtifactProducer {
                skill: non_empty(CONTEXT_PRODUCER_SKILL)?,
                runner: non_empty(CONTEXT_PRODUCER_RUNNER)?,
            },
            created_at: non_empty(input.created_at)?,
            hash: non_empty(input.digest)?,
            size_bytes: input.size_bytes,
            parent_artifact_id: None,
            receipt_id: None,
            redacted: false,
        },
    })
}

pub(super) fn insert_string(object: &mut JsonObject, key: &str, value: &str) {
    object.insert(key.to_owned(), JsonValue::String(value.to_owned()));
}

fn non_empty(value: impl Into<String>) -> Result<NonEmptyString, RuntimeError> {
    NonEmptyString::new(value.into()).ok_or_else(|| RuntimeError::ReceiptInvalid {
        message: "skill context artifact included an empty required field".to_owned(),
    })
}
