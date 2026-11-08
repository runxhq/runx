//! A step's output declarations: each field is its JSON Schema, required
//! unless it says `optional: true`, and kept as an artifact of the packet it
//! names.
//!
//! The standalone `output.schema.json` document is a top-level open map carrying
//! a bare `$id`; it is modeled here as the transparent map newtype [`Output`].
//! The same `BTreeMap<String, OutputField>` is embedded by the agent-context
//! envelope's `output` field.
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::PacketId;
use crate::digest::sha256_prefixed;
use crate::json::{JsonObject, JsonValue};
use crate::schema::{NamedSchemas, RunxSchema, record_named};

/// One output field: its JSON Schema as written, whether a producer may leave
/// it out, and the packet it is kept as, if it is an artifact.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputField {
    /// The field's JSON Schema. No `type` takes any value.
    pub schema: JsonObject,
    pub optional: bool,
    pub packet: Option<PacketId>,
}

impl OutputField {
    /// A required field of one JSON Schema type.
    #[must_use]
    pub fn of_type(field_type: &str) -> Self {
        Self {
            schema: JsonObject::from([(
                "type".to_owned(),
                JsonValue::String(field_type.to_owned()),
            )]),
            optional: false,
            packet: None,
        }
    }

    /// Whether a successful producer must return this field.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        !self.optional
    }
}

impl Serialize for OutputField {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut field = self.schema.clone();
        if self.optional {
            field.insert("optional".to_owned(), JsonValue::Bool(true));
        }
        if let Some(packet) = &self.packet {
            field.insert("packet".to_owned(), JsonValue::String(packet.to_string()));
        }
        field.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OutputField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut schema = JsonObject::deserialize(deserializer)?;
        let optional = match schema.remove("optional") {
            None => false,
            Some(JsonValue::Bool(optional)) => optional,
            Some(_) => return Err(D::Error::custom("optional is true or false")),
        };
        let packet = match schema.remove("packet") {
            None => None,
            Some(JsonValue::String(packet)) => {
                Some(PacketId::new(packet.clone()).ok_or_else(|| {
                    D::Error::custom(format!("packet {packet:?} is not a packet id"))
                })?)
            }
            Some(_) => return Err(D::Error::custom("packet is a packet id")),
        };
        Ok(Self {
            schema,
            optional,
            packet,
        })
    }
}

impl RunxSchema for OutputField {
    fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "optional": { "type": "boolean" },
                "packet": { "type": "string" },
            },
            "additionalProperties": true,
            "title": "OutputField",
            "description": "One output field: its JSON Schema, `optional: true` when a producer may leave it out, and the `packet` it is kept as.",
        })
    }

    fn named_schemas(named: &mut NamedSchemas) {
        record_named(named, "OutputField", Self::json_schema);
    }
}

/// A step's declared outputs: each output field by name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(transparent)]
#[runx_schema(id = "runx.output.v1")]
pub struct Output(pub BTreeMap<String, OutputField>);

/// A malformed output declaration. Syntax and nested schema validity share one
/// error because callers must reject either defect at the same contract-load
/// boundary.
#[derive(Debug, thiserror::Error)]
pub enum OutputContractParseError {
    #[error("output declaration is invalid: {0}")]
    Declaration(#[from] serde_json::Error),
    #[error("output declaration contains an invalid JSON Schema: {0}")]
    Schema(String),
}

/// Parse the canonical output map once for parser, runtime, inspection, and
/// agent invocation consumers.
pub fn parse_output_contract(
    raw: &JsonObject,
) -> Result<BTreeMap<String, OutputField>, OutputContractParseError> {
    let value = serde_json::to_value(JsonValue::Object(raw.clone()))?;
    let output = serde_json::from_value::<Output>(value)?.0;
    let schema = serde_json::to_value(output_value_schema(Some(&output)))?;
    jsonschema::draft202012::meta::validate(&schema)
        .map_err(|error| OutputContractParseError::Schema(error.to_string()))?;
    jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| OutputContractParseError::Schema(error.to_string()))?;
    Ok(output)
}

/// A deterministic output-contract violation at the native trust boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputValidationError {
    path: String,
    message: String,
}

impl OutputValidationError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for OutputValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for OutputValidationError {}

/// Build the exact JSON schema presented to an agent for its final result.
/// Runtime validation below is derived from the same declarations, keeping the
/// model hint and the enforced boundary on one semantic source.
#[must_use]
pub fn output_value_schema(output: Option<&BTreeMap<String, OutputField>>) -> JsonValue {
    let Some(output) = output.filter(|fields| !fields.is_empty()) else {
        return object_schema();
    };

    let mut properties = JsonObject::new();
    let mut required = Vec::new();
    for (name, field) in output {
        properties.insert(name.clone(), output_field_schema(field));
        if field.is_required() {
            required.push(JsonValue::String(name.clone()));
        }
    }

    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), JsonValue::String("object".to_owned()));
    schema.insert("properties".to_owned(), JsonValue::Object(properties));
    schema.insert("additionalProperties".to_owned(), JsonValue::Bool(false));
    if !required.is_empty() {
        schema.insert("required".to_owned(), JsonValue::Array(required));
    }
    JsonValue::Object(schema)
}

/// Hash the enforced output contract using the deterministic boundary JSON
/// representation. This digest is suitable for signed receipt evidence.
pub fn output_contract_digest(
    output: Option<&BTreeMap<String, OutputField>>,
) -> Result<String, serde_json::Error> {
    serde_json::to_vec(&output_value_schema(output)).map(|bytes| sha256_prefixed(&bytes))
}

/// Validate an agent's final result against its declared output contract.
/// Undeclared fields are rejected whenever a declaration is present; an absent
/// declaration still requires a JSON object but intentionally leaves it open.
pub fn validate_output_value(
    output: Option<&BTreeMap<String, OutputField>>,
    value: &JsonValue,
) -> Result<(), OutputValidationError> {
    let schema = serde_json::to_value(output_value_schema(output)).map_err(|error| {
        OutputValidationError::new(
            "$",
            format!("declared output schema could not be serialized: {error}"),
        )
    })?;
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| {
            OutputValidationError::new("$", format!("declared output schema is invalid: {error}"))
        })?;
    let instance = serde_json::to_value(value).map_err(|error| {
        OutputValidationError::new(
            "$",
            format!("output value could not be serialized: {error}"),
        )
    })?;
    let Some(error) = validator.iter_errors(&instance).next() else {
        return Ok(());
    };
    Err(OutputValidationError::new(
        validation_error_path(&error),
        error.to_string(),
    ))
}

fn object_schema() -> JsonValue {
    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), JsonValue::String("object".to_owned()));
    JsonValue::Object(schema)
}

fn output_field_schema(field: &OutputField) -> JsonValue {
    JsonValue::Object(field.schema.clone())
}

fn validation_error_path(error: &jsonschema::ValidationError<'_>) -> String {
    use jsonschema::error::ValidationErrorKind;

    let nested = error.instance_path().as_str();
    let base = if nested.is_empty() {
        "$".to_owned()
    } else {
        format!("$.{}", nested.trim_start_matches('/'))
    };
    match error.kind() {
        ValidationErrorKind::Required { property } => property
            .as_str()
            .map_or(base.clone(), |property| format!("{base}.{property}")),
        ValidationErrorKind::AdditionalProperties { unexpected } => unexpected
            .first()
            .map_or(base.clone(), |property| format!("{base}.{property}")),
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputField, output_contract_digest, output_value_schema, parse_output_contract,
        validate_output_value,
    };
    use crate::{JsonNumber, JsonValue};
    use std::collections::BTreeMap;

    fn declared_output() -> BTreeMap<String, OutputField> {
        [
            ("notify_plan".to_owned(), OutputField::of_type("object")),
            (
                "status".to_owned(),
                OutputField {
                    schema: serde_json::from_value(serde_json::json!({
                        "type": "string",
                        "enum": ["ready", "blocked"]
                    }))
                    .unwrap_or_default(),
                    optional: true,
                    packet: None,
                },
            ),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn output_schema_and_validator_share_strict_declared_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = declared_output();
        let valid = JsonValue::Object(
            [("notify_plan".to_owned(), JsonValue::Object(BTreeMap::new()))]
                .into_iter()
                .collect(),
        );
        let extra = JsonValue::Object(
            [
                ("notify_plan".to_owned(), JsonValue::Object(BTreeMap::new())),
                ("message".to_owned(), JsonValue::String("secret".to_owned())),
            ]
            .into_iter()
            .collect(),
        );

        assert!(validate_output_value(Some(&output), &valid).is_ok());
        let Err(error) = validate_output_value(Some(&output), &extra) else {
            return Err("extra field must fail".into());
        };
        assert!(error.to_string().starts_with("$.message: "));
        assert!(
            serde_json::to_string(&output_value_schema(Some(&output)))?
                .contains("\"additionalProperties\":false")
        );
        Ok(())
    }

    #[test]
    fn output_validator_enforces_required_type_and_enum() -> Result<(), Box<dyn std::error::Error>>
    {
        let output = declared_output();
        let missing = JsonValue::Object(BTreeMap::new());
        let wrong_enum = JsonValue::Object(
            [
                ("notify_plan".to_owned(), JsonValue::Object(BTreeMap::new())),
                ("status".to_owned(), JsonValue::String("sent".to_owned())),
            ]
            .into_iter()
            .collect(),
        );
        let wrong_type = JsonValue::Object(
            [(
                "notify_plan".to_owned(),
                JsonValue::Number(JsonNumber::I64(1)),
            )]
            .into_iter()
            .collect(),
        );

        let Err(error) = validate_output_value(Some(&output), &missing) else {
            return Err("missing required field must fail".into());
        };
        assert!(error.to_string().starts_with("$.notify_plan: "));
        assert!(validate_output_value(Some(&output), &wrong_enum).is_err());
        assert!(validate_output_value(Some(&output), &wrong_type).is_err());
        Ok(())
    }

    #[test]
    fn output_contract_digest_is_stable() -> Result<(), serde_json::Error> {
        let output = declared_output();
        assert_eq!(
            output_contract_digest(Some(&output))?,
            output_contract_digest(Some(&output))?
        );
        Ok(())
    }

    #[test]
    fn output_schema_and_validator_preserve_nested_contracts()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = BTreeMap::from([(
            "result".to_owned(),
            OutputField {
                schema: serde_json::from_value(serde_json::json!({
                    "type": "object",
                    "required": ["decision"],
                    "properties": {
                        "decision": { "type": "string", "enum": ["ready", "blocked"] }
                    },
                    "additionalProperties": false
                }))?,
                optional: false,
                packet: None,
            },
        )]);
        let valid = serde_json::from_value(serde_json::json!({
            "result": { "decision": "ready" }
        }))?;
        let invalid = serde_json::from_value(serde_json::json!({
            "result": { "decision": "unknown" }
        }))?;

        assert!(validate_output_value(Some(&output), &valid).is_ok());
        let Err(error) = validate_output_value(Some(&output), &invalid) else {
            return Err("nested enum must be enforced".into());
        };
        assert!(error.to_string().starts_with("$.result/decision: "));
        let schema = serde_json::to_value(output_value_schema(Some(&output)))?;
        assert_eq!(
            schema["properties"]["result"]["required"][0],
            serde_json::Value::String("decision".to_owned())
        );
        Ok(())
    }

    #[test]
    fn output_contract_parser_rejects_invalid_nested_schema()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw = serde_json::from_value(serde_json::json!({
            "result": {
                "type": "object",
                "required": "decision"
            }
        }))?;

        let Err(error) = parse_output_contract(&raw) else {
            return Err("invalid schema must fail at load".into());
        };
        assert!(error.to_string().contains("invalid JSON Schema"));
        Ok(())
    }
}
