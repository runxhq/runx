use std::collections::BTreeMap;

use crate::protocol::{InvocationLimits, WorkerFailureCode};

use super::EngineError;

pub(super) fn validate_input(
    inputs: &serde_json::Value,
    maximum: usize,
) -> Result<(), EngineError> {
    let bytes = serde_json::to_vec(inputs)
        .map_err(|error| EngineError::new(WorkerFailureCode::InvalidRequest, error.to_string()))?;
    if bytes.len() > maximum {
        return Err(EngineError::limit(format!(
            "JavaScript input is {} bytes; limit is {maximum} bytes",
            bytes.len()
        )));
    }
    if contains_unsafe_integer(inputs) {
        return Err(EngineError::new(
            WorkerFailureCode::InvalidRequest,
            "JavaScript input contains an integer outside its safe range; use strings for exact large integers",
        ));
    }
    Ok(())
}

fn contains_unsafe_integer(value: &serde_json::Value) -> bool {
    const MAX_SAFE_INTEGER: i64 = (1_i64 << 53) - 1;
    match value {
        serde_json::Value::Number(number) => {
            number
                .as_i64()
                .is_some_and(|value| !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value))
                || number
                    .as_u64()
                    .is_some_and(|value| value > MAX_SAFE_INTEGER as u64)
        }
        serde_json::Value::Array(values) => values.iter().any(contains_unsafe_integer),
        serde_json::Value::Object(values) => values.values().any(contains_unsafe_integer),
        _ => false,
    }
}

pub(super) fn validate_bundle(
    entry_module: &str,
    export_name: &str,
    modules: &BTreeMap<String, String>,
    limits: InvocationLimits,
) -> Result<(), EngineError> {
    validate_module_path(entry_module)?;
    validate_export_name(export_name)?;
    if !modules.contains_key(entry_module) {
        return Err(EngineError::new(
            WorkerFailureCode::ModuleRejected,
            "entry module is not present in the supplied bundle",
        ));
    }
    let input_bytes = serde_json::to_vec(modules)
        .map_err(|error| EngineError::new(WorkerFailureCode::InvalidRequest, error.to_string()))?;
    if input_bytes.len() > limits.source_bytes {
        return Err(EngineError::limit(format!(
            "JavaScript module bundle is {} bytes; limit is {} bytes",
            input_bytes.len(),
            limits.source_bytes
        )));
    }
    for (path, source) in modules {
        validate_module_path(path)?;
        let imports = runx_parser::javascript_module_imports(path, source).map_err(|error| {
            EngineError::new(WorkerFailureCode::ModuleRejected, error.to_string())
        })?;
        for specifier in imports {
            let resolved = runx_parser::resolve_javascript_module_import(path, &specifier)
                .map_err(|error| {
                    EngineError::new(WorkerFailureCode::ModuleRejected, error.to_string())
                })?;
            if !modules.contains_key(&resolved) {
                return Err(EngineError::new(
                    WorkerFailureCode::ModuleRejected,
                    format!(
                        "JavaScript import {specifier:?} from {path:?} resolves outside the supplied bundle"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_module_path(path: &str) -> Result<(), EngineError> {
    let valid_extension = path.ends_with(".js") || path.ends_with(".mjs");
    let valid_segments = !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".."));
    if valid_extension && valid_segments {
        return Ok(());
    }
    Err(EngineError::new(
        WorkerFailureCode::ModuleRejected,
        format!("JavaScript module path {path:?} is not a normalized relative .js/.mjs path"),
    ))
}

fn validate_export_name(name: &str) -> Result<(), EngineError> {
    let mut characters = name.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || matches!(character, '_' | '$'));
    if valid_start
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'))
    {
        return Ok(());
    }
    Err(EngineError::new(
        WorkerFailureCode::InvalidRequest,
        format!("JavaScript export name {name:?} is not an identifier"),
    ))
}
