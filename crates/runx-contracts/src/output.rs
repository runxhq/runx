//! Skill output declaration types: the value shape of the `runx.ai/spec`
//! output map (a field is either a bare type name or a typed field spec).
//!
//! The standalone `output.schema.json` document is a top-level open map carrying
//! a bare `$id`; it is modeled here as the transparent map newtype [`Output`],
//! whose `RunxSchema` derive emits the committed `patternProperties` shape. The
//! same `BTreeMap<String, OutputField>` is embedded by the agent-context
//! envelope's `output` field.
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::digest::sha256_prefixed;
use crate::json::{JsonObject, JsonValue};
use crate::schema::{NonEmptyString, RunxSchema};

/// A declared output value type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputType {
    String,
    Number,
    Integer,
    Boolean,
    Array,
    Object,
    Null,
}

/// The expanded form of an output field declaration. Committed with
/// `additionalProperties: false` and `minProperties: 1` (the latter is a
/// numeric bound the emitter does not express).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputFieldSpec {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub field_type: Option<OutputType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap_as: Option<NonEmptyString>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    /// Additional JSON Schema 2020-12 keywords for this output value. The
    /// scalar type, description, and enum above remain the canonical
    /// top-level declaration; this object owns nested shape and bounds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<JsonObject>,
}

/// A single output field declaration: either a bare type name or a typed spec.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(untagged)]
pub enum OutputField {
    Type(OutputType),
    Spec(OutputFieldSpec),
}

impl OutputField {
    /// Whether a successful producer must return this field.
    ///
    /// Bare type declarations and expanded declarations without an explicit
    /// `required` value are required by default.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        match self {
            Self::Type(_) => true,
            Self::Spec(spec) => match spec.required {
                Some(required) => required,
                None => true,
            },
        }
    }
}

/// A runner's declared outputs: each output field by name.
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

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
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
    let mut schema = match field {
        OutputField::Spec(spec) => spec.schema.clone().unwrap_or_default(),
        OutputField::Type(_) => JsonObject::new(),
    };
    match field {
        OutputField::Type(field_type) => {
            schema.insert(
                "type".to_owned(),
                JsonValue::String(output_type_name(field_type).to_owned()),
            );
        }
        OutputField::Spec(spec) => {
            if let Some(field_type) = spec.field_type.as_ref() {
                schema.insert(
                    "type".to_owned(),
                    JsonValue::String(output_type_name(field_type).to_owned()),
                );
            }
            if let Some(values) = spec.enum_values.as_ref() {
                schema.insert(
                    "enum".to_owned(),
                    JsonValue::Array(values.iter().cloned().map(JsonValue::String).collect()),
                );
            }
            if let Some(description) = spec.description.as_ref() {
                schema.insert(
                    "description".to_owned(),
                    JsonValue::String(description.clone()),
                );
            }
        }
    }
    JsonValue::Object(schema)
}

const fn output_type_name(field_type: &OutputType) -> &'static str {
    match field_type {
        OutputType::String => "string",
        OutputType::Number => "number",
        OutputType::Integer => "integer",
        OutputType::Boolean => "boolean",
        OutputType::Array => "array",
        OutputType::Object => "object",
        OutputType::Null => "null",
    }
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
        OutputField, OutputFieldSpec, OutputType, output_contract_digest, output_value_schema,
        parse_output_contract, validate_output_value,
    };
    use crate::{JsonNumber, JsonValue};
    use std::collections::BTreeMap;

    fn declared_output() -> BTreeMap<String, OutputField> {
        [
            (
                "notify_plan".to_owned(),
                OutputField::Type(OutputType::Object),
            ),
            (
                "status".to_owned(),
                OutputField::Spec(OutputFieldSpec {
                    field_type: Some(OutputType::String),
                    description: None,
                    required: Some(false),
                    wrap_as: None,
                    enum_values: Some(vec!["ready".to_owned(), "blocked".to_owned()]),
                    schema: None,
                }),
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
        assert_eq!(error.path(), "$.message");
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
        assert_eq!(error.path(), "$.notify_plan");
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
            OutputField::Spec(OutputFieldSpec {
                field_type: Some(OutputType::Object),
                description: None,
                required: None,
                wrap_as: None,
                enum_values: None,
                schema: Some(serde_json::from_value(serde_json::json!({
                    "required": ["decision"],
                    "properties": {
                        "decision": { "type": "string", "enum": ["ready", "blocked"] }
                    },
                    "additionalProperties": false
                }))?),
            }),
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
        assert_eq!(error.path(), "$.result/decision");
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
                "schema": { "required": "decision" }
            }
        }))?;

        let Err(error) = parse_output_contract(&raw) else {
            return Err("invalid schema must fail at load".into());
        };
        assert!(error.to_string().contains("invalid JSON Schema"));
        Ok(())
    }
}
