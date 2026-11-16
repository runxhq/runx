use runx_contracts::{JsonObject, JsonValue};

use super::{object, text};
use crate::RuntimeError;

const TOOL: &str = "evidence.verify_artifact";

pub(super) fn value_at_path<'a>(
    value: Option<&'a JsonValue>,
    dotted_path: &str,
) -> Option<&'a JsonValue> {
    if !valid_path(dotted_path) {
        return None;
    }
    dotted_path
        .split('.')
        .try_fold(value?, |current, segment| current.as_object()?.get(segment))
}

pub(super) fn apply_bindings(
    mut target: JsonObject,
    source: Option<&JsonValue>,
    bindings: &[JsonValue],
    field: &str,
) -> Result<JsonObject, RuntimeError> {
    for (index, raw) in bindings.iter().enumerate() {
        let binding = object(Some(raw));
        let target_path = text(binding.get("target_path"));
        let source_path = text(binding.get("source_path"));
        let Some(replacement) = value_at_path(source, &source_path).cloned() else {
            return Err(invalid(format!(
                "{field}[{index}] must bind existing target_path and source_path values"
            )));
        };
        if target_path.is_empty() {
            return Err(invalid(format!(
                "{field}[{index}] must bind existing target_path and source_path values"
            )));
        }
        set_at_path(&mut target, &target_path, replacement)?;
    }
    Ok(target)
}

pub(super) fn present(value: Option<&JsonValue>) -> bool {
    match value {
        Some(JsonValue::String(value)) => !value.trim().is_empty(),
        Some(JsonValue::Array(value)) => !value.is_empty(),
        Some(JsonValue::Object(value)) => !value.is_empty(),
        Some(JsonValue::Null) | None => false,
        Some(_) => true,
    }
}

pub(super) fn canonical_equal(left: Option<&JsonValue>, right: Option<&JsonValue>) -> bool {
    left.unwrap_or(&JsonValue::Null) == right.unwrap_or(&JsonValue::Null)
}

fn set_at_path(
    target: &mut JsonObject,
    dotted_path: &str,
    replacement: JsonValue,
) -> Result<(), RuntimeError> {
    if !valid_path(dotted_path) {
        return Err(invalid(format!(
            "invalid binding target path: {dotted_path}"
        )));
    }
    let mut segments = dotted_path.split('.').peekable();
    let mut current = target;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            current.insert(segment.to_owned(), replacement);
            return Ok(());
        }
        let child = current
            .entry(segment.to_owned())
            .or_insert_with(|| JsonValue::Object(JsonObject::new()));
        if !matches!(child, JsonValue::Object(_)) {
            *child = JsonValue::Object(JsonObject::new());
        }
        current = child
            .as_object_mut()
            .ok_or_else(|| invalid("binding target path could not be created"))?;
    }
    Err(invalid("binding target path is empty"))
}

fn valid_path(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::failed(TOOL.to_owned(), message.into())
}

trait JsonObjectMutation {
    fn as_object_mut(&mut self) -> Option<&mut JsonObject>;
}

impl JsonObjectMutation for JsonValue {
    fn as_object_mut(&mut self) -> Option<&mut JsonObject> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }
}
