use runx_contracts::{JsonObject, JsonValue};

use crate::ValidationError;

#[derive(Clone, Copy)]
pub(crate) struct JsonFieldReader {
    object: &'static str,
}

impl JsonFieldReader {
    pub(crate) const fn new(object: &'static str) -> Self {
        Self { object }
    }

    pub(crate) fn validation_error(&self, message: impl Into<String>) -> ValidationError {
        ValidationError::InvalidField {
            field: self.object.to_owned(),
            message: message.into(),
        }
    }

    pub(crate) fn required_string(
        &self,
        value: Option<&JsonValue>,
        field: &str,
    ) -> Result<String, ValidationError> {
        match self.optional_string(value, field)? {
            Some(value) if !value.is_empty() => Ok(value),
            _ => Err(ValidationError::MissingField {
                field: field.to_owned(),
            }),
        }
    }

    pub(crate) fn optional_string(
        &self,
        value: Option<&JsonValue>,
        field: &str,
    ) -> Result<Option<String>, ValidationError> {
        match value {
            None | Some(JsonValue::Null) => Ok(None),
            Some(JsonValue::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(self.validation_error(format!("{field} must be a string."))),
        }
    }

    pub(crate) fn optional_non_empty_string(
        &self,
        value: Option<&JsonValue>,
        field: &str,
    ) -> Result<Option<String>, ValidationError> {
        let Some(value) = self.optional_string(value, field)? else {
            return Ok(None);
        };
        if value.trim().is_empty() {
            return Err(self.validation_error(format!("{field} must not be empty.")));
        }
        Ok(Some(value))
    }

    pub(crate) fn required_object<'a>(
        &self,
        value: Option<&'a JsonValue>,
        field: &str,
    ) -> Result<&'a JsonObject, ValidationError> {
        match value {
            Some(JsonValue::Object(value)) => Ok(value),
            None | Some(JsonValue::Null) => {
                Err(self.validation_error(format!("{field} is required.")))
            }
            Some(_) => Err(self.validation_error(format!("{field} must be an object."))),
        }
    }

    pub(crate) fn optional_object(
        &self,
        value: Option<&JsonValue>,
        field: &str,
    ) -> Result<Option<JsonObject>, ValidationError> {
        match value {
            None | Some(JsonValue::Null) => Ok(None),
            Some(JsonValue::Object(value)) => Ok(Some(value.clone())),
            Some(_) => Err(self.validation_error(format!("{field} must be an object."))),
        }
    }

    pub(crate) fn required_plain_array<'a>(
        &self,
        value: Option<&'a JsonValue>,
        field: &str,
    ) -> Result<&'a [JsonValue], ValidationError> {
        match value {
            Some(JsonValue::Array(values)) => Ok(values),
            None | Some(JsonValue::Null) => {
                Err(self.validation_error(format!("{field} is required.")))
            }
            Some(_) => Err(self.validation_error(format!("{field} must be an array."))),
        }
    }

    pub(crate) fn optional_string_array(
        &self,
        value: Option<&JsonValue>,
        field: &str,
    ) -> Result<Option<Vec<String>>, ValidationError> {
        match value {
            None | Some(JsonValue::Null) => Ok(None),
            Some(JsonValue::Array(values)) => values
                .iter()
                .map(|value| match value {
                    JsonValue::String(value) => Ok(value.clone()),
                    _ => {
                        Err(self.validation_error(format!("{field} must be an array of strings.")))
                    }
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(self.validation_error(format!("{field} must be an array of strings."))),
        }
    }

    pub(crate) fn optional_bool(
        &self,
        value: Option<&JsonValue>,
        field: &str,
    ) -> Result<Option<bool>, ValidationError> {
        match value {
            None | Some(JsonValue::Null) => Ok(None),
            Some(JsonValue::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(self.validation_error(format!("{field} must be a boolean."))),
        }
    }

    pub(crate) fn reject_unknown_fields(
        &self,
        object: &JsonObject,
        field: &str,
        allowed: &[&str],
    ) -> Result<(), ValidationError> {
        for key in object.keys() {
            if !allowed.contains(&key.as_str()) {
                return Err(self.validation_error(format!(
                    "{field}.{key} is not supported; allowed fields: {}.",
                    allowed.join(", ")
                )));
            }
        }
        Ok(())
    }
}
