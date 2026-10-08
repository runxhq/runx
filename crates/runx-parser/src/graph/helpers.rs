use std::collections::BTreeMap;

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

pub fn optional_string_object(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<BTreeMap<String, String>>, ValidationError> {
    let Some(object) = optional_object(value, field)? else {
        return Ok(None);
    };
    let mut output = BTreeMap::new();
    for (key, value) in object {
        let JsonValue::String(value) = value else {
            return Err(validation_error(format!("{field}.{key} must be a string.")));
        };
        output.insert(key.clone(), value.clone());
    }
    Ok(Some(output))
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
    if value.fract() == 0.0 && value >= 0.0 && value <= u64::MAX as f64 {
        Ok(value as u64)
    } else {
        Err(validation_error(format!(
            "{field} must be a non-negative integer."
        )))
    }
}
