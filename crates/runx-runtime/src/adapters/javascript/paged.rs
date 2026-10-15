use std::path::Path;

use runx_contracts::javascript_worker::MAX_INPUT_BYTES;
use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::JavaScriptAdapter;
use crate::RuntimeError;
use crate::adapter::{InvocationOutput, InvocationStatus, SkillInvocation};
use crate::services::{LocalArtifact, LocalArtifactService, resolve_scoped_root};

const PAGE_CONTROL: &str = "runx_page";
const MAX_PAGES: usize = 4_096;
const MAX_CONTINUATION_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn invoke(
    adapter: &JavaScriptAdapter,
    mut request: SkillInvocation,
    artifacts: &LocalArtifactService,
) -> Result<InvocationOutput, RuntimeError> {
    let page_source = super::worker(&request)?
        .pages
        .clone()
        .ok_or_else(|| page_error("missing page source"))?;
    let requested_path = take_string_input(&mut request, &page_source.path_from)?;
    let path_scope = page_source
        .path_scope_from
        .as_deref()
        .map(|field| take_string_input(&mut request, field))
        .transpose()?
        .unwrap_or_else(|| "workspace".to_owned());
    reject_reserved_input(&request.inputs)?;

    let root = resolve_scoped_root(
        ".",
        &path_scope,
        &request.env,
        &request.skill_directory,
        crate::services::WorkspaceBounds::Operator,
    )
    .map_err(|error| page_error(error.to_string()))?;
    let artifact = artifacts
        .admit(
            &root,
            Path::new(&requested_path),
            &page_source.media_type,
            crate::services::WorkspaceBounds::Operator,
        )
        .map_err(|error| page_error(error.to_string()))?;
    let prepared = adapter.prepare_invocation(&request)?;

    let mut state = JsonValue::Null;
    let mut offset = 0_u64;
    let mut total_duration_ms = 0_u64;
    for page_index in 0..MAX_PAGES {
        let record_budget =
            page_record_budget(&request.inputs, &artifact, page_index, offset, &state)?;
        let page = artifacts
            .read_json_array_page_with_record_budget(
                &artifact.reference,
                offset,
                usize::try_from(page_source.page_bytes)
                    .map_err(|_| page_error("page size is not representable"))?,
                record_budget,
            )
            .map_err(|error| page_error_at(page_index, offset, error))?;
        if !page.eof && page.next_offset <= offset {
            return Err(page_error_at(
                page_index,
                offset,
                "artifact page made no forward progress",
            ));
        }

        let inputs = page_inputs(&request.inputs, &page, page_index, state)?;
        let mut output = adapter.invoke_prepared(&prepared, &inputs)?;
        total_duration_ms = total_duration_ms.saturating_add(output.duration_ms());
        if output.status == InvocationStatus::Failure {
            let failure = format!(
                "artifact page {page_index} at byte {offset} failed: {}",
                output
                    .failure_message()
                    .unwrap_or_else(|| "worker returned no failure detail".to_owned())
            );
            output.reject(failure);
            output.set_duration_ms(total_duration_ms);
            return Ok(output);
        }

        let mut result = parse_page_output(
            std::mem::replace(&mut output.value, JsonValue::Null),
            page_index,
            offset,
        )?;
        let control = result
            .remove(PAGE_CONTROL)
            .and_then(|value| match value {
                JsonValue::Object(value) => Some(value),
                _ => None,
            })
            .ok_or_else(|| {
                page_error_at(
                    page_index,
                    offset,
                    "module output must contain a runx_page object",
                )
            })?;
        let next_state = control.get("state").cloned().ok_or_else(|| {
            page_error_at(
                page_index,
                offset,
                "module output runx_page.state is required",
            )
        })?;
        validate_state_size(&next_state, page_index, offset)?;
        let done = match control.get("done") {
            Some(JsonValue::Bool(done)) => *done,
            None => false,
            Some(_) => {
                return Err(page_error_at(
                    page_index,
                    offset,
                    "module output runx_page.done must be boolean",
                ));
            }
        };
        let finished = done || page.eof;
        if !finished {
            if !result.is_empty() {
                return Err(page_error_at(
                    page_index,
                    offset,
                    "intermediate page output may contain only runx_page continuation state",
                ));
            }
            state = next_state;
            offset = page.next_offset;
            continue;
        }
        if result.is_empty() {
            return Err(page_error_at(
                page_index,
                offset,
                "final page output must contain the declared domain result",
            ));
        }
        output.value = JsonValue::Object(result);
        output.set_duration_ms(total_duration_ms);
        return Ok(output);
    }

    Err(page_error_at(
        MAX_PAGES,
        offset,
        format!("artifact exceeded the fixed {MAX_PAGES}-page execution ceiling"),
    ))
}

fn take_string_input(request: &mut SkillInvocation, name: &str) -> Result<String, RuntimeError> {
    request.resolved_inputs.remove(name);
    match request.inputs.remove(name) {
        Some(JsonValue::String(value)) if !value.trim().is_empty() => Ok(value),
        _ => Err(page_error(format!(
            "paged JavaScript input {name:?} must be a non-empty string"
        ))),
    }
}

fn reject_reserved_input(inputs: &JsonObject) -> Result<(), RuntimeError> {
    if inputs.contains_key(PAGE_CONTROL) {
        return Err(page_error(format!(
            "{PAGE_CONTROL:?} is reserved for runtime-owned page context"
        )));
    }
    Ok(())
}

fn page_inputs(
    base: &JsonObject,
    page: &crate::services::ArtifactRecordPage,
    page_index: usize,
    state: JsonValue,
) -> Result<JsonObject, RuntimeError> {
    let mut inputs = base.clone();
    let records = page
        .records
        .iter()
        .cloned()
        .map(JsonValue::String)
        .collect::<Vec<_>>();
    let context = JsonObject::from([
        (
            "artifact_ref".to_owned(),
            JsonValue::String(page.artifact.reference.clone()),
        ),
        (
            "media_type".to_owned(),
            JsonValue::String(page.artifact.media_type.clone()),
        ),
        (
            "whole_digest".to_owned(),
            JsonValue::String(page.artifact.whole_digest.clone()),
        ),
        (
            "artifact_bytes".to_owned(),
            JsonValue::Number(JsonNumber::U64(page.artifact.bytes)),
        ),
        (
            "page_index".to_owned(),
            JsonValue::Number(JsonNumber::U64(page_index as u64)),
        ),
        (
            "offset".to_owned(),
            JsonValue::Number(JsonNumber::U64(page.offset)),
        ),
        (
            "length".to_owned(),
            JsonValue::Number(JsonNumber::U64(page.length)),
        ),
        (
            "next_offset".to_owned(),
            JsonValue::Number(JsonNumber::U64(page.next_offset)),
        ),
        ("eof".to_owned(), JsonValue::Bool(page.eof)),
        (
            "range_digest".to_owned(),
            JsonValue::String(page.range_digest.clone()),
        ),
        ("records".to_owned(), JsonValue::Array(records)),
        ("state".to_owned(), state),
    ]);
    inputs.insert(PAGE_CONTROL.to_owned(), JsonValue::Object(context));
    let bytes = serde_json::to_vec(&inputs)
        .map_err(|source| RuntimeError::json("measuring paged JavaScript inputs", source))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(page_error_at(
            page_index,
            page.offset,
            format!(
                "framed page produces {} input bytes; deterministic worker limit is {MAX_INPUT_BYTES}",
                bytes.len()
            ),
        ));
    }
    Ok(inputs)
}

fn page_record_budget(
    base: &JsonObject,
    artifact: &LocalArtifact,
    page_index: usize,
    offset: u64,
    state: &JsonValue,
) -> Result<usize, RuntimeError> {
    let empty_page = crate::services::ArtifactRecordPage {
        artifact: artifact.clone(),
        offset,
        length: 0,
        next_offset: offset,
        eof: false,
        range_digest: format!("sha256:{}", "0".repeat(64)),
        records: Vec::new(),
    };
    let inputs = page_inputs(base, &empty_page, page_index, state.clone())?;
    let fixed_bytes = serde_json::to_vec(&inputs)
        .map_err(|source| RuntimeError::json("measuring fixed page inputs", source))?
        .len();
    MAX_INPUT_BYTES
        .checked_sub(fixed_bytes)
        .and_then(|remaining| remaining.checked_add(2))
        .and_then(|remaining| remaining.checked_sub(128))
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| {
            page_error_at(
                page_index,
                offset,
                "base inputs and continuation state leave no room for page records",
            )
        })
}

fn parse_page_output(
    value: JsonValue,
    page_index: usize,
    offset: u64,
) -> Result<JsonObject, RuntimeError> {
    match value {
        JsonValue::Object(value) => Ok(value),
        _ => Err(page_error_at(
            page_index,
            offset,
            "module output must be an object",
        )),
    }
}

fn validate_state_size(
    state: &JsonValue,
    page_index: usize,
    offset: u64,
) -> Result<(), RuntimeError> {
    let bytes = serde_json::to_vec(state)
        .map_err(|source| RuntimeError::json("measuring JavaScript continuation state", source))?;
    if bytes.len() > MAX_CONTINUATION_BYTES {
        return Err(page_error_at(
            page_index,
            offset,
            format!(
                "continuation state is {} bytes; fixed limit is {MAX_CONTINUATION_BYTES}",
                bytes.len()
            ),
        ));
    }
    Ok(())
}

fn page_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::JavaScriptWorker {
        message: format!("paged execution rejected: {}", message.into()),
    }
}

fn page_error_at(page_index: usize, offset: u64, message: impl std::fmt::Display) -> RuntimeError {
    page_error(format!(
        "artifact page {page_index} at byte {offset}: {message}"
    ))
}
