use std::collections::BTreeMap;

use runx_contracts::tools::ToolManifestSchema;
use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::skill::{
    SkillArtifactContract, SkillIdempotencyPolicy, SkillInput, SkillRetryPolicy, SkillSource,
    SourceKind, validate_inputs, validate_skill_artifact_contract, validate_skill_source,
};
use crate::{ParseError, ValidationError, assert_yaml_parity_subset, json_fields::JsonFieldReader};

const FIELDS: JsonFieldReader = JsonFieldReader::new("tool_manifest");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawToolManifestIr {
    pub document: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source: SkillSource,
    pub inputs: BTreeMap<String, SkillInput>,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<SkillRetryPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency: Option<SkillIdempotencyPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<SkillArtifactContract>,
}

impl ValidatedTool {
    /// Return the exact non-secret execution requirements selected by this
    /// tool manifest. Runtime callers consume this typed projection instead of
    /// rescanning the source or normalizing provider scope strings.
    #[must_use]
    pub fn execution_requirements(&self) -> runx_contracts::ExecutionRequirements {
        runx_contracts::ExecutionRequirements {
            scopes: self.scopes.clone(),
            environment: self.source.environment.clone(),
            ..runx_contracts::ExecutionRequirements::default()
        }
    }
}

pub fn parse_tool_manifest_yaml(yaml: &str) -> Result<RawToolManifestIr, ParseError> {
    assert_yaml_parity_subset("tool_manifest", yaml)?;
    let parsed: JsonValue =
        serde_norway::from_str(yaml).map_err(|error| ParseError::InvalidYaml {
            field: "tool_manifest".to_owned(),
            message: error.to_string(),
        })?;
    manifest_from_value(parsed, "Tool manifest YAML must parse to an object.")
}

pub fn parse_tool_manifest_json(json: &str) -> Result<RawToolManifestIr, ParseError> {
    let parsed: JsonValue =
        serde_json::from_str(json).map_err(|error| ParseError::InvalidJson {
            field: "tool_manifest".to_owned(),
            message: format!("Tool manifest JSON is invalid: {error}"),
        })?;
    manifest_from_value(parsed, "Tool manifest JSON must parse to an object.")
}

pub fn validate_tool_manifest(raw: RawToolManifestIr) -> Result<ValidatedTool, ValidationError> {
    FIELDS.reject_unknown_fields(
        &raw.document,
        "tool_manifest",
        &[
            "schema",
            "name",
            "version",
            "description",
            "source",
            "inputs",
            "scopes",
            "risk",
            "retry",
            "idempotency",
            "artifacts",
        ],
    )?;
    validate_required_contract::<ToolManifestSchema>(raw.document.get("schema"), "schema")?;
    let risk = raw.document.get("risk").cloned();
    if risk
        .as_ref()
        .and_then(JsonValue::as_object)
        .is_some_and(|risk| risk.contains_key("mutating"))
    {
        return Err(FIELDS.validation_error(
            "risk.mutating is not supported; effect ownership belongs to the invoking capability",
        ));
    }
    let source = validate_tool_source(
        validate_skill_source(
            &FIELDS
                .required_object(raw.document.get("source"), "source")?
                .clone(),
        )?,
        "source.type",
    )?;
    Ok(ValidatedTool {
        name: FIELDS.required_string(raw.document.get("name"), "name")?,
        version: FIELDS.optional_string(raw.document.get("version"), "version")?,
        description: FIELDS.optional_string(raw.document.get("description"), "description")?,
        source,
        inputs: validate_inputs(
            FIELDS
                .optional_object(raw.document.get("inputs"), "inputs")?
                .unwrap_or_default(),
            "inputs",
        )?,
        scopes: validate_scopes(
            FIELDS
                .optional_string_array(raw.document.get("scopes"), "scopes")?
                .unwrap_or_default(),
        )?,
        risk,
        retry: validate_retry(raw.document.get("retry"), "retry")?,
        idempotency: validate_idempotency(raw.document.get("idempotency"), "idempotency")?,
        artifacts: validate_skill_artifact_contract(raw.document.get("artifacts"), "artifacts")?,
    })
}

fn validate_scopes(scopes: Vec<String>) -> Result<Vec<String>, ValidationError> {
    if scopes.iter().any(|scope| scope.trim().is_empty()) {
        return Err(FIELDS.validation_error("scopes must contain only non-empty scope strings"));
    }
    Ok(scopes)
}

fn validate_tool_source(source: SkillSource, field: &str) -> Result<SkillSource, ValidationError> {
    if matches!(
        source.source_type,
        SourceKind::CliTool | SourceKind::JavaScript
    ) {
        return Ok(source);
    }
    Err(FIELDS.validation_error(format!(
        "{field} must be one of cli-tool or javascript for tool manifests."
    )))
}

fn manifest_from_value(
    value: JsonValue,
    object_error: &str,
) -> Result<RawToolManifestIr, ParseError> {
    let JsonValue::Object(document) = value else {
        return Err(ParseError::InvalidDocument {
            field: "tool_manifest".to_owned(),
            message: object_error.to_owned(),
        });
    };
    Ok(RawToolManifestIr { document })
}

fn validate_required_contract<T>(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<T, ValidationError>
where
    T: serde::de::DeserializeOwned,
{
    let value = value.ok_or_else(|| FIELDS.validation_error(format!("{field} is required")))?;
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|error| FIELDS.validation_error(format!("{field} is invalid: {error}")))
}

fn validate_retry(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<SkillRetryPolicy>, ValidationError> {
    let Some(retry) = FIELDS.optional_object(value, field)? else {
        return Ok(None);
    };
    let max_attempts = FIELDS
        .optional_u64(retry.get("max_attempts"), &format!("{field}.max_attempts"))?
        .unwrap_or(1);
    if max_attempts == 0 {
        return Err(
            FIELDS.validation_error(format!("{field}.max_attempts must be a positive integer."))
        );
    }
    Ok(Some(SkillRetryPolicy { max_attempts }))
}

fn validate_idempotency(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<SkillIdempotencyPolicy>, ValidationError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) if value.trim().is_empty() => {
            Err(FIELDS.validation_error(format!("{field} must not be empty.")))
        }
        Some(JsonValue::String(value)) => Ok(Some(SkillIdempotencyPolicy {
            key: Some(value.clone()),
        })),
        Some(value) => {
            let record = FIELDS.required_object(Some(value), field)?;
            Ok(Some(SkillIdempotencyPolicy {
                key: FIELDS
                    .optional_non_empty_string(record.get("key"), &format!("{field}.key"))?,
            }))
        }
    }
}
