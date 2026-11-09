use std::collections::BTreeMap;

use runx_contracts::tools::ToolManifestSchema;
use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::skill::{OutputArtifacts, SkillInput, validate_input_fields};
use crate::workflow::{ProcessTool, output_artifacts, validate_process_tool};
use crate::{
    ParseError, ValidationError, assert_yaml_supported_subset, json_fields::JsonFieldReader,
};

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
    /// The tool's executable: a trusted host process.
    pub process: ProcessTool,
    /// Its inputs, in a method's form.
    pub inputs: BTreeMap<String, SkillInput>,
    /// Its outputs as written, in a method's form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<JsonObject>,
    /// The output the whole result becomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_as: Option<String>,
    /// The outputs kept as artifacts: each output naming a `packet`, and
    /// `result_as` with its packet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<OutputArtifacts>,
}

pub fn parse_tool_manifest_yaml(yaml: &str) -> Result<RawToolManifestIr, ParseError> {
    assert_yaml_supported_subset("tool_manifest", yaml)?;
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
            "process",
            "inputs",
            "outputs",
            "result_as",
        ],
    )?;
    validate_required_contract::<ToolManifestSchema>(raw.document.get("schema"), "schema")?;
    let process = validate_process_tool(
        FIELDS.required_object(raw.document.get("process"), "process")?,
        "process",
    )?;
    let outputs = FIELDS.optional_object(raw.document.get("outputs"), "outputs")?;
    if let Some(outputs) = &outputs {
        runx_contracts::parse_output_contract(outputs)
            .map_err(|error| FIELDS.validation_error(format!("outputs is invalid: {error}")))?;
    }
    let result_as = FIELDS.optional_non_empty_string(raw.document.get("result_as"), "result_as")?;
    let artifacts = output_artifacts(outputs.as_ref(), result_as.clone(), "")?;
    Ok(ValidatedTool {
        name: FIELDS.required_string(raw.document.get("name"), "name")?,
        version: FIELDS.optional_string(raw.document.get("version"), "version")?,
        description: FIELDS.optional_string(raw.document.get("description"), "description")?,
        process,
        inputs: validate_input_fields(
            &FIELDS
                .optional_object(raw.document.get("inputs"), "inputs")?
                .unwrap_or_default(),
            "inputs",
        )?,
        outputs,
        result_as,
        artifacts,
    })
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
