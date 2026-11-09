use std::collections::BTreeMap;

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ContextEntry, ContextEntrySchema, ContextLabels, ContextOrigin, ContextProducer,
    ContextPurpose, ContextSharing, ContextTrust, JsonObject, JsonValue, sha256_prefixed,
};

use crate::RuntimeError;

const CONTEXT_ENTRY_TYPE: &str = "runx.skill.context";
const CONTEXT_PRODUCER_SKILL: &str = "runx-runtime";
const CONTEXT_PRODUCER_METHOD: &str = "skill-context";
const PENDING_RUN_ID: &str = "rx_pending";

pub(super) struct SkillContextEntryInput<'a> {
    pub(super) step_id: &'a str,
    pub(super) reference: &'a str,
    pub(super) env: &'a BTreeMap<String, String>,
    pub(super) created_at: &'a str,
    pub(super) digest: &'a str,
    pub(super) size_bytes: u64,
    /// What the entry is, in a line, for an index.
    pub(super) summary: String,
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
        schema: ContextEntrySchema::V1,
        r#type: non_empty(CONTEXT_ENTRY_TYPE)?,
        summary: Some(non_empty(input.summary)?),
        data: input.data,
        labels: ContextLabels {
            purpose: ContextPurpose::Procedure,
            trust: ContextTrust::SkillInstructions,
            sharing: ContextSharing::ConfiguredModel,
        },
        // A skill's instructions come from the skill its reference names.
        sources: vec![non_empty(input.reference)?],
        origin: ContextOrigin {
            artifact_id: non_empty(artifact_id)?,
            run_id: non_empty(
                input
                    .env
                    .get(crate::execution::runner::RUNX_RUN_ID_ENV)
                    .map(String::as_str)
                    .unwrap_or(PENDING_RUN_ID),
            )?,
            step_id: Some(non_empty(input.step_id)?),
            producer: ContextProducer {
                skill: non_empty(CONTEXT_PRODUCER_SKILL)?,
                method: non_empty(CONTEXT_PRODUCER_METHOD)?,
            },
            created_at: non_empty(input.created_at)?,
            digest: non_empty(input.digest)?,
            size_bytes: input.size_bytes,
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
