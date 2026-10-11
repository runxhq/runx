use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue};

use crate::RuntimeError;

pub(super) fn json_value_env(value: &JsonValue) -> Result<String, RuntimeError> {
    match value {
        JsonValue::Null => Ok(String::new()),
        JsonValue::Bool(value) => Ok(value.to_string()),
        JsonValue::Number(value) => serde_json::to_string(value)
            .map_err(|source| RuntimeError::json("serializing input number", source)),
        JsonValue::String(value) => Ok(value.clone()),
        JsonValue::Array(_) | JsonValue::Object(_) => serde_json::to_string(value)
            .map_err(|source| RuntimeError::json("serializing structured input", source)),
    }
}

/// An argument with its placeholders filled in one pass: `{{name}}` is the
/// input `name` and `{{env.NAME}}` the declared environment value `NAME`. A
/// filled value is never read again, so an input cannot reach the
/// environment, and a placeholder that names neither is refused.
pub(super) fn resolve_template(
    template: &str,
    inputs: &JsonObject,
    declared_environment: &BTreeMap<String, String>,
) -> Result<String, RuntimeError> {
    let mut resolved = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        resolved.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| unknown_placeholder(template, &rest[start..]))?;
        let name = &after[..end];
        let value = match name.strip_prefix("env.") {
            Some(key) => declared_environment.get(key).cloned(),
            None => inputs.get(name).map(json_value_env).transpose()?,
        };
        resolved.push_str(
            &value.ok_or_else(|| unknown_placeholder(template, &rest[start..start + end + 4]))?,
        );
        rest = &after[end + 2..];
    }
    resolved.push_str(rest);
    Ok(resolved)
}

fn unknown_placeholder(template: &str, placeholder: &str) -> RuntimeError {
    RuntimeError::InvalidProcessInvocation {
        message: format!(
            "process argument {template:?} names {placeholder:?}, which is neither a provided input nor a declared environment value"
        ),
    }
}
