//! Tool step targets: a catalog reference, a deterministic worker module, or a
//! trusted host process.

use runx_contracts::javascript_worker::{MAX_INPUT_BYTES, MAX_WALL_MILLISECONDS};
use runx_contracts::{JsonObject, JsonValue};
use runx_core::policy::admit_agent_tool_ref;

use super::helpers::{
    number_to_non_negative_integer, optional_number, optional_object, optional_string,
    optional_string_array, required_string, validation_error,
};
use super::types::{ProcessTool, ToolTarget, WorkerTool};
use crate::ValidationError;
use crate::skill::{
    ArtifactPageFraming, ArtifactPageSource, InputMode, validate_environment_requirements,
};

const WORKER_FIELDS: &[&str] = &[
    "module",
    "export",
    "timeout_seconds",
    "pages",
    "environment",
];
const PROCESS_FIELDS: &[&str] = &[
    "command",
    "args",
    "cwd",
    "timeout_seconds",
    "input_mode",
    "environment",
];

/// `tool: <reference>`, `tool: {module, ...}` or `tool: {command, ...}`.
pub(super) fn validate_tool_target(
    value: &JsonValue,
    field: &str,
) -> Result<ToolTarget, ValidationError> {
    match value {
        JsonValue::String(reference) => {
            let admission = admit_agent_tool_ref(reference);
            if !admission.allowed {
                return Err(validation_error(format!(
                    "{field} {reference:?} is not an admissible catalog tool ref: {}.",
                    admission.reason
                )));
            }
            Ok(ToolTarget::Catalog(reference.clone()))
        }
        JsonValue::Object(tool) => {
            match (tool.contains_key("module"), tool.contains_key("command")) {
                (true, false) => validate_worker(tool, field).map(ToolTarget::Worker),
                (false, true) => validate_process(tool, field).map(ToolTarget::Process),
                _ => Err(validation_error(format!(
                    "{field} must declare exactly one of module (a worker) or command (a host process)."
                ))),
            }
        }
        _ => Err(validation_error(format!(
            "{field} must be a catalog tool reference or a worker or process declaration."
        ))),
    }
}

/// A tool manifest's executable: always a trusted host process.
pub fn validate_process_tool(
    value: &JsonObject,
    field: &str,
) -> Result<ProcessTool, ValidationError> {
    validate_process(value, field)
}

fn validate_worker(tool: &JsonObject, field: &str) -> Result<WorkerTool, ValidationError> {
    reject_unknown(tool, field, WORKER_FIELDS)?;
    let timeout_seconds = optional_u64(
        tool.get("timeout_seconds"),
        &format!("{field}.timeout_seconds"),
    )?;
    if let Some(timeout_seconds) = timeout_seconds {
        let maximum = MAX_WALL_MILLISECONDS / 1_000;
        if timeout_seconds == 0 || timeout_seconds > maximum {
            return Err(validation_error(format!(
                "{field}.timeout_seconds must be between 1 and {maximum}."
            )));
        }
    }
    Ok(WorkerTool {
        module: validate_module(tool.get("module"), &format!("{field}.module"))?,
        export: validate_export(tool.get("export"), &format!("{field}.export"))?,
        timeout_seconds,
        pages: validate_pages(tool.get("pages"), &format!("{field}.pages"))?,
        environment: validate_environment_requirements(tool.get("environment"))?,
    })
}

fn validate_process(tool: &JsonObject, field: &str) -> Result<ProcessTool, ValidationError> {
    reject_unknown(tool, field, PROCESS_FIELDS)?;
    let command = required_string(tool.get("command"), &format!("{field}.command"))?;
    if command.trim().is_empty() {
        return Err(validation_error(format!(
            "{field}.command must not be empty."
        )));
    }
    Ok(ProcessTool {
        command,
        args: optional_string_array(tool.get("args"), &format!("{field}.args"))?
            .unwrap_or_default(),
        cwd: optional_string(tool.get("cwd"), &format!("{field}.cwd"))?,
        timeout_seconds: optional_u64(
            tool.get("timeout_seconds"),
            &format!("{field}.timeout_seconds"),
        )?,
        input_mode: validate_input_mode(tool.get("input_mode"), &format!("{field}.input_mode"))?,
        environment: validate_environment_requirements(tool.get("environment"))?,
    })
}

fn reject_unknown(tool: &JsonObject, field: &str, allowed: &[&str]) -> Result<(), ValidationError> {
    match tool.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(validation_error(format!(
            "{field}.{key} is not a field of this tool kind."
        ))),
        None => Ok(()),
    }
}

fn optional_u64(value: Option<&JsonValue>, field: &str) -> Result<Option<u64>, ValidationError> {
    optional_number(value, field)?
        .map(|value| number_to_non_negative_integer(value, field))
        .transpose()
}

fn validate_module(value: Option<&JsonValue>, field: &str) -> Result<String, ValidationError> {
    let module = required_string(value, field)?;
    let segments = module.split('/').collect::<Vec<_>>();
    if module.starts_with('/')
        || module.contains('\\')
        || segments
            .iter()
            .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."))
        || !matches!(segments.last(), Some(name) if name.ends_with(".mjs") || name.ends_with(".js"))
    {
        return Err(validation_error(format!(
            "{field} must be a portable relative .mjs or .js path without '.', '..', or backslash segments."
        )));
    }
    Ok(module)
}

fn validate_export(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<String>, ValidationError> {
    let Some(name) = optional_string(value, field)? else {
        return Ok(None);
    };
    let mut chars = name.chars();
    let valid_start = chars
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || matches!(character, '_' | '$'));
    if !valid_start
        || !chars
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'))
    {
        return Err(validation_error(format!(
            "{field} must be a JavaScript identifier."
        )));
    }
    Ok(Some(name))
}

fn validate_input_mode(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<InputMode>, ValidationError> {
    let Some(value) = optional_string(value, field)? else {
        return Ok(None);
    };
    match value.as_str() {
        "args" => Ok(Some(InputMode::Args)),
        "stdin" => Ok(Some(InputMode::Stdin)),
        "none" => Ok(Some(InputMode::None)),
        _ => Err(validation_error(format!(
            "{field} must be args, stdin, or none."
        ))),
    }
}

fn validate_pages(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<ArtifactPageSource>, ValidationError> {
    let Some(pages) = optional_object(value, field)? else {
        return Ok(None);
    };
    reject_unknown(
        &pages,
        field,
        &[
            "framing",
            "media_type",
            "page_bytes",
            "path_from",
            "path_scope_from",
        ],
    )?;
    let path_from = page_input_name(
        &required_string(pages.get("path_from"), &format!("{field}.path_from"))?,
        &format!("{field}.path_from"),
    )?;
    let path_scope_from = optional_string(
        pages.get("path_scope_from"),
        &format!("{field}.path_scope_from"),
    )?
    .map(|name| page_input_name(&name, &format!("{field}.path_scope_from")))
    .transpose()?;
    if path_from == "runx_page" || path_scope_from.as_deref() == Some("runx_page") {
        return Err(validation_error(format!(
            "{field} input names cannot use the runtime-reserved runx_page field."
        )));
    }
    if path_scope_from.as_deref() == Some(path_from.as_str()) {
        return Err(validation_error(format!(
            "{field}.path_from and path_scope_from must name different inputs."
        )));
    }
    let media_type = required_string(pages.get("media_type"), &format!("{field}.media_type"))?;
    let framing = match required_string(pages.get("framing"), &format!("{field}.framing"))?.as_str()
    {
        "json_array" => ArtifactPageFraming::JsonArray,
        other => {
            return Err(validation_error(format!(
                "{field}.framing {other:?} is unsupported; expected json_array."
            )));
        }
    };
    let page_bytes = optional_u64(pages.get("page_bytes"), &format!("{field}.page_bytes"))?
        .unwrap_or(1024 * 1024);
    let maximum_page_bytes = u64::try_from(MAX_INPUT_BYTES).unwrap_or(u64::MAX);
    if page_bytes == 0 || page_bytes > maximum_page_bytes {
        return Err(validation_error(format!(
            "{field}.page_bytes must be between 1 and {maximum_page_bytes}."
        )));
    }
    Ok(Some(ArtifactPageSource {
        path_from,
        path_scope_from,
        media_type,
        framing,
        page_bytes,
    }))
}

fn page_input_name(name: &str, field: &str) -> Result<String, ValidationError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(validation_error(format!(
            "{field} must name one declared input using letters, digits, '_' or '-'."
        )));
    }
    Ok(name.to_owned())
}
