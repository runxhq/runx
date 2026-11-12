//! Shared execution declarations and governed outcome contracts.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{JsonNumber, JsonObject, JsonValue, PacketId};

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
    #[serde(
        default,
        deserialize_with = "deserialize_input_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub default: Option<JsonValue>,
    /// Canonical packet contract for a JSON-valued input. The runtime resolves
    /// this identifier through the packet-schema catalog before inspection or
    /// execution; authors never duplicate the packet schema inline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet: Option<PacketId>,
    /// Additional JSON Schema 2020-12 keywords for this input. The scalar
    /// `type`, description, and default above remain the canonical top-level
    /// declaration; this object owns nested shape, bounds, enums, examples,
    /// and composition without creating a second schema file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<JsonObject>,
}

/// A missing default is None; a present JSON null is a declared default.
fn deserialize_input_default<'de, D>(deserializer: D) -> Result<Option<JsonValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    JsonValue::deserialize(deserializer).map(Some)
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

    /// Produce the complete schema consumed by inspection, adapters, exports,
    /// and runtime validation from the one authored input declaration.
    #[must_use]
    pub fn effective_schema(&self) -> JsonObject {
        let mut schema = self.schema.clone().unwrap_or_default();
        if let Some(packet) = &self.packet {
            schema.insert(
                "x-runx-packet-id".to_owned(),
                JsonValue::String(packet.to_string()),
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

/// Build the complete JSON Schema for one method or tool input object. Every
/// public projection calls this function so required fields, defaults, nested
/// constraints, and closed-object behavior cannot drift by surface.
#[must_use]
pub fn input_contract_schema(inputs: &BTreeMap<String, InputDefinition>) -> JsonObject {
    input_contract_schema_with_examples(inputs, &[])
}

/// Build a method input schema and retain its parser-validated, copy-valid
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

/// The input schema a turn offers for a call: the input contract, where an
/// input of a packet type may instead be the `runx.artifact.v1` a
/// call of the turn returned for such a packet.
pub fn call_input_schema(
    inputs: &BTreeMap<String, InputDefinition>,
) -> Result<JsonObject, crate::json::JsonValueError> {
    let mut schema = input_contract_schema(inputs);
    let reference = JsonValue::from_serialize(&crate::Artifact::json_schema())?;
    if let Some(JsonValue::Object(properties)) = schema.get_mut("properties") {
        for (name, input) in inputs {
            if input.packet.is_none() {
                continue;
            }
            if let Some(packet) = properties.remove(name) {
                let common_type = packet
                    .as_object()
                    .and_then(|schema| schema.get("type"))
                    .filter(|kind| {
                        reference.as_object().and_then(|schema| schema.get("type")) == Some(*kind)
                    })
                    .cloned();
                let mut accepted = JsonObject::from([(
                    "anyOf".to_owned(),
                    JsonValue::Array(vec![packet, reference.clone()]),
                )]);
                // Preserve a type shared by both alternatives. Tool transports
                // can then decode an object without resolving the whole union.
                if let Some(kind) = common_type {
                    accepted.insert("type".to_owned(), kind);
                }
                properties.insert(name.clone(), JsonValue::Object(accepted));
            }
        }
    }
    Ok(schema)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{InputDefinition, JsonValue, call_input_schema};

    fn declared(input_type: &str) -> InputDefinition {
        InputDefinition {
            input_type: input_type.to_owned(),
            required: true,
            description: None,
            default: None,
            packet: None,
            schema: None,
        }
    }

    #[test]
    fn input_defaults_preserve_null_and_absence_through_serialization()
    -> Result<(), Box<dyn std::error::Error>> {
        for value in [None, Some(JsonValue::Null), Some(JsonValue::Bool(false))] {
            let mut input = declared("json");
            input.default = value;
            let serialized = serde_json::to_value(&input)?;
            assert_eq!(serialized.get("default").is_some(), input.default.is_some());
            let decoded: InputDefinition = serde_json::from_value(serialized)?;
            assert_eq!(decoded, input);
            let decoded =
                JsonValue::from_serialize(&input)?.deserialize_into::<InputDefinition>()?;
            assert_eq!(decoded, input);
        }
        Ok(())
    }

    /// The text a command line passes reads as a value of its declared type.
    #[test]
    fn text_reads_as_its_declared_type() -> Result<(), String> {
        for (input_type, text, expected) in [
            ("string", "Sydney, AU", r#""Sydney, AU""#),
            ("string", r#"{"not":"json"}"#, r#""{\"not\":\"json\"}""#),
            ("number", "1.5", "1.5"),
            ("integer", "42", "42"),
            ("boolean", "true", "true"),
            (
                "object",
                r#"{"provider":"example","periods":[]}"#,
                r#"{"provider":"example","periods":[]}"#,
            ),
            ("array", "[1,2]", "[1,2]"),
            ("json", r#""fixture""#, r#""fixture""#),
            ("json", "null", "null"),
        ] {
            let expected: JsonValue =
                serde_json::from_str(expected).map_err(|error| error.to_string())?;
            assert_eq!(
                declared(input_type).value_from_text(text)?,
                expected,
                "{input_type} {text}"
            );
        }
        Ok(())
    }

    #[test]
    fn packet_calls_preserve_only_the_type_both_alternatives_accept()
    -> Result<(), Box<dyn std::error::Error>> {
        for input_type in ["object", "string", "json"] {
            let mut input = declared(input_type);
            input.packet = Some(serde_json::from_str(r#""runx.source_packet.v1""#)?);
            let schema = serde_json::to_value(call_input_schema(&BTreeMap::from([(
                "source".to_owned(),
                input,
            )]))?)?;
            let source = &schema["properties"]["source"];
            assert_eq!(source["anyOf"].as_array().map(Vec::len), Some(2));
            assert_eq!(source["anyOf"][1]["type"], "object");
            if input_type == "object" {
                assert_eq!(source["type"], "object");
            } else {
                assert!(source.get("type").is_none(), "{source}");
            }
        }
        Ok(())
    }
}
