use runx_contracts::{JsonObject, JsonValue};

use crate::ValidationError;

pub fn validation_error(message: impl Into<String>) -> ValidationError {
    let message = message.into();
    ValidationError::InvalidField {
        field: field_from_message(&message),
        message,
    }
}

fn field_from_message(message: &str) -> String {
    message
        .split_whitespace()
        .next()
        .map(|field| field.trim_end_matches([':', '.']).to_owned())
        .filter(|field| !field.is_empty())
        .unwrap_or_else(|| "document".to_owned())
}

/// A step id, `[a-z][a-z0-9_-]*`. Step ids and call indexes make up the path
/// of every run in a run's tree, and an address joins a path to an id with
/// `#`, so neither holds `/` or `#`: what a run waits on is unique at any
/// depth.
pub fn step_id(value: String, field: &str) -> Result<String, ValidationError> {
    grammar(
        value,
        field,
        "[a-z][a-z0-9_-]*",
        |first| first.is_ascii_lowercase(),
        |character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        },
    )
}

/// A gate id, `[a-z0-9][a-z0-9_.-]*`: the local id of an ask's request.
pub fn gate_id(value: String, field: &str) -> Result<String, ValidationError> {
    grammar(
        value,
        field,
        "[a-z0-9][a-z0-9_.-]*",
        |first| first.is_ascii_lowercase() || first.is_ascii_digit(),
        |character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '.' | '-')
        },
    )
}

fn grammar(
    value: String,
    field: &str,
    pattern: &str,
    first: impl Fn(char) -> bool,
    rest: impl Fn(char) -> bool,
) -> Result<String, ValidationError> {
    let mut characters = value.chars();
    if characters.next().is_some_and(first) && characters.all(rest) {
        return Ok(value);
    }
    Err(validation_error(format!(
        "{field} {value:?} must match {pattern}."
    )))
}

pub fn required_string(value: Option<&JsonValue>, field: &str) -> Result<String, ValidationError> {
    match optional_string(value, field)? {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(ValidationError::MissingField {
            field: field.to_owned(),
        }),
    }
}

pub fn optional_string(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<String>, ValidationError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(validation_error(format!("{field} must be a string."))),
    }
}

pub fn optional_non_empty_string(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<String>, ValidationError> {
    let Some(value) = optional_string(value, field)? else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Err(validation_error(format!("{field} must not be empty.")));
    }
    Ok(Some(value))
}

pub fn required_object<'a>(
    value: Option<&'a JsonValue>,
    field: &str,
) -> Result<&'a JsonObject, ValidationError> {
    match value {
        Some(JsonValue::Object(value)) => Ok(value),
        _ => Err(validation_error(format!("{field} must be an object."))),
    }
}

pub fn optional_object(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<JsonObject>, ValidationError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Object(value)) => Ok(Some(value.clone())),
        Some(_) => Err(validation_error(format!("{field} must be an object."))),
    }
}

pub fn required_array<'a>(
    value: Option<&'a JsonValue>,
    field: &str,
) -> Result<&'a [JsonValue], ValidationError> {
    let Some(JsonValue::Array(value)) = value else {
        return Err(validation_error(format!("{field} must be an array.")));
    };
    if value.is_empty() {
        return Err(validation_error(format!(
            "{field} must contain at least one step."
        )));
    }
    Ok(value)
}

pub fn optional_string_array(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<Vec<String>>, ValidationError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                JsonValue::String(value) => Ok(value.clone()),
                _ => Err(validation_error(format!(
                    "{field} must be an array of strings."
                ))),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(validation_error(format!(
            "{field} must be an array of strings."
        ))),
    }
}

pub fn optional_number(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<f64>, ValidationError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Number(number)) => number
            .as_f64()
            .ok_or_else(|| validation_error(format!("{field} must be a finite number.")))
            .map(Some),
        Some(_) => Err(validation_error(format!(
            "{field} must be a finite number."
        ))),
    }
}

pub fn number_to_non_negative_integer(value: f64, field: &str) -> Result<u64, ValidationError> {
    // `u64::MAX` rounds up to 2^64 as a float, so the bound is exclusive.
    if value.fract() == 0.0 && value >= 0.0 && value < u64::MAX as f64 {
        Ok(value as u64)
    } else {
        Err(validation_error(format!(
            "{field} must be a non-negative integer."
        )))
    }
}
