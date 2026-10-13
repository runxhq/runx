//! Shared execution declarations and governed outcome contracts.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{CanonicalJsonError, JsonNumber, JsonObject, JsonValue, canonical_stable_json};

/// One declared input shared by skills, local tools, inspection, and runtime
/// materialization. Keeping this at the contract boundary prevents each
/// parser or catalog surface from inventing its own type/default semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct InputDefinition {
    #[serde(rename = "type")]
    pub input_type: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<bool>,
    /// Canonical packet contract for a JSON-valued input. The runtime resolves
    /// this identifier through the packet-schema catalog before inspection or
    /// execution; authors never duplicate the packet schema inline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet: Option<String>,
    /// Additional JSON Schema 2020-12 keywords for this input. The scalar
    /// `type`, description, and default above remain the canonical top-level
    /// declaration; this object owns nested shape, bounds, enums, examples,
    /// and composition without creating a second schema file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<JsonObject>,
}

impl InputDefinition {
    /// Whether a concrete JSON value satisfies this parser-validated input
    /// type. Manifest validation and runtime materialization use this same
    /// predicate.
    #[must_use]
    pub fn accepts_value(&self, value: &JsonValue) -> bool {
        match self.input_type.as_str() {
            "json" => true,
            "string" => matches!(value, JsonValue::String(_)),
            "number" => matches!(value, JsonValue::Number(_)),
            "integer" => matches!(
                value,
                JsonValue::Number(JsonNumber::I64(_) | JsonNumber::U64(_))
            ),
            "boolean" => matches!(value, JsonValue::Bool(_)),
            "object" => matches!(value, JsonValue::Object(_)),
            "array" => matches!(value, JsonValue::Array(_)),
            _ => false,
        }
    }

    /// The value command-line `text` gives this input, read by its declared
    /// type and never guessed: a `string` is the text itself; any other type
    /// is the text read as JSON, which must be of that type.
    pub fn value_from_text(&self, text: &str) -> Result<JsonValue, String> {
        if self.input_type == "string" {
            return Ok(JsonValue::String(text.to_owned()));
        }
        let value = serde_json::from_str::<JsonValue>(text)
            .map_err(|error| format!("is {}, written as JSON: {error}", self.input_type))?;
        if self.accepts_value(&value) {
            Ok(value)
        } else {
            Err(format!("is {}, not {text}", self.input_type))
        }
    }

    /// The command-line text that gives this input `value`, which
    /// [`Self::value_from_text`] reads back: a `string` is its own text; any
    /// other value is written as JSON.
    pub fn text_for_value(&self, value: &JsonValue) -> Result<String, CanonicalJsonError> {
        match value {
            JsonValue::String(text) if self.input_type == "string" => Ok(text.clone()),
            value => canonical_stable_json(value),
        }
    }

    /// Produce the complete schema consumed by inspection, adapters, exports,
    /// and runtime validation from the one authored input declaration.
    #[must_use]
    pub fn effective_schema(&self) -> JsonObject {
        let mut schema = self.schema.clone().unwrap_or_default();
        if let Some(packet) = &self.packet {
            schema.insert(
                "x-runx-packet-id".to_owned(),
                JsonValue::String(packet.clone()),
            );
        }
        if self.input_type != "json" {
            schema.insert(
                "type".to_owned(),
                JsonValue::String(self.input_type.clone()),
            );
        }
        if let Some(description) = &self.description {
            schema.insert(
                "description".to_owned(),
                JsonValue::String(description.clone()),
            );
        }
        if let Some(default) = &self.default {
            schema.insert("default".to_owned(), default.clone());
        }
        schema
    }
}

/// Build the complete JSON Schema for one runner or tool input object. Every
/// public projection calls this function so required fields, defaults, nested
/// constraints, and closed-object behavior cannot drift by surface.
#[must_use]
pub fn input_contract_schema(inputs: &BTreeMap<String, InputDefinition>) -> JsonObject {
    input_contract_schema_with_examples(inputs, &[])
}

/// Build a runner input schema and retain its parser-validated, copy-valid
/// examples as standard JSON Schema annotations. Tool contracts use the same
/// projection without examples.
#[must_use]
pub fn input_contract_schema_with_examples(
    inputs: &BTreeMap<String, InputDefinition>,
    examples: &[JsonObject],
) -> JsonObject {
    let properties = inputs
        .iter()
        .map(|(name, input)| (name.clone(), JsonValue::Object(input.effective_schema())))
        .collect();
    let required = inputs
        .iter()
        .filter(|(_name, input)| input.required && input.default.is_none())
        .map(|(name, _input)| JsonValue::String(name.clone()))
        .collect();
    let mut schema = JsonObject::from([
        ("type".to_owned(), JsonValue::String("object".to_owned())),
        ("properties".to_owned(), JsonValue::Object(properties)),
        ("required".to_owned(), JsonValue::Array(required)),
        ("additionalProperties".to_owned(), JsonValue::Bool(false)),
    ]);
    if !examples.is_empty() {
        schema.insert(
            "examples".to_owned(),
            JsonValue::Array(examples.iter().cloned().map(JsonValue::Object).collect()),
        );
    }
    schema
}

/// How a successful execution result is exposed to downstream graph context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactContract {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emits: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub named_emits: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packets: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct IdempotencyPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{InputDefinition, JsonValue};

    fn declared(input_type: &str) -> InputDefinition {
        InputDefinition {
            input_type: input_type.to_owned(),
            required: true,
            description: None,
            default: None,
            artifact: None,
            packet: None,
            schema: None,
        }
    }

    /// The text a command line writes for a value reads back as that value,
    /// for every declared type.
    #[test]
    fn a_value_written_as_text_reads_back_by_its_declared_type() -> Result<(), String> {
        for (input_type, json) in [
            ("string", r#""Sydney, AU""#),
            ("string", r#""{\"not\":\"json\"}""#),
            ("number", "1.5"),
            ("integer", "42"),
            ("boolean", "true"),
            ("object", r#"{"provider":"example","periods":[]}"#),
            ("array", "[1,2]"),
            ("json", r#""fixture""#),
        ] {
            let declared = declared(input_type);
            let value: JsonValue = serde_json::from_str(json).map_err(|error| error.to_string())?;
            let text = declared
                .text_for_value(&value)
                .map_err(|error| error.to_string())?;
            assert_eq!(
                declared.value_from_text(&text)?,
                value,
                "{input_type} {json}"
            );
        }
        Ok(())
    }
}
