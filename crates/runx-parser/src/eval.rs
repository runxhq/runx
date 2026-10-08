use std::fmt;

use crate::{
    ParseError, SkillInstallError, SkillInstallOrigin, ValidateSkillMode, ValidateSkillOptions,
    ValidationError,
    harness_fixture::{HarnessFixtureError, parse_harness_fixture},
    parse_graph_yaml, parse_packet_schema_document, parse_runner_manifest_yaml,
    parse_skill_markdown, parse_tool_manifest_json, parse_tool_manifest_yaml,
    runner::resolve_post_run_reflect_policy,
    validate_graph, validate_runner_manifest, validate_skill_artifact_contract,
    validate_skill_install, validate_skill_with_options, validate_tool_manifest,
};
use runx_contracts::{JsonObject, JsonValue, json_string_field};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParserEvalOutput {
    Output { value: JsonValue },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParserEvalError {
    InvalidDocument(String),
    InvalidInput(String),
    Parse(String),
    Validation(String),
    SerializeOutput(String),
}

impl ParserEvalError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidDocument(_) => "invalid_document",
            Self::InvalidInput(_) => "invalid_input",
            Self::Parse(_) => "parse_error",
            Self::Validation(_) => "validation_error",
            Self::SerializeOutput(_) => "serialize_output",
        }
    }
}

impl fmt::Display for ParserEvalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument(message)
            | Self::InvalidInput(message)
            | Self::Parse(message)
            | Self::Validation(message)
            | Self::SerializeOutput(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ParserEvalError {}

impl From<ParseError> for ParserEvalError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error.to_string())
    }
}

impl From<ValidationError> for ParserEvalError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error.to_string())
    }
}

impl From<SkillInstallError> for ParserEvalError {
    fn from(error: SkillInstallError) -> Self {
        Self::Validation(error.to_string())
    }
}

impl From<HarnessFixtureError> for ParserEvalError {
    fn from(error: HarnessFixtureError) -> Self {
        Self::Validation(error.to_string())
    }
}

pub fn evaluate_parser_document_str(source: &str) -> Result<ParserEvalOutput, ParserEvalError> {
    let document = serde_json::from_str::<JsonValue>(source)
        .map_err(|error| ParserEvalError::InvalidDocument(error.to_string()))?;
    if let Some(kind) = parser_document_kind(&document)
        && !is_supported_parser_kind(kind)
    {
        return Err(ParserEvalError::InvalidInput(format!(
            "unsupported parser input kind '{kind}'"
        )));
    }
    let input = serde_json::from_str::<ParserDocument>(source)
        .map_err(|error| ParserEvalError::InvalidInput(error.to_string()))?;
    Ok(ParserEvalOutput::Output {
        value: evaluate_parser_document(input)?,
    })
}

fn parser_document_kind(document: &JsonValue) -> Option<&str> {
    let JsonValue::Object(fields) = document else {
        return None;
    };
    json_string_field(fields, "kind")
}

fn is_supported_parser_kind(kind: &str) -> bool {
    matches!(
        kind,
        "parser.validateSkillMarkdown"
            | "parser.validateRunnerManifestYaml"
            | "parser.validateGraphYaml"
            | "parser.validateToolManifestYaml"
            | "parser.validateToolManifestJson"
            | "parser.validateHarnessFixtureYaml"
            | "parser.parsePacketSchemaDocument"
            | "parser.validateSkillArtifactContract"
            | "parser.resolvePostRunReflectPolicy"
            | "parser.validateSkillInstall"
    )
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ParserDocument {
    Batch {
        inputs: Vec<ParserInput>,
        #[serde(default, rename = "returnErrors")]
        return_errors: bool,
    },
    Input(Box<ParserInput>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
enum ParserInput {
    #[serde(rename = "parser.validateSkillMarkdown")]
    ValidateSkillMarkdown {
        markdown: String,
        #[serde(default)]
        mode: ParserSkillMode,
    },
    #[serde(rename = "parser.validateRunnerManifestYaml")]
    ValidateRunnerManifestYaml { yaml: String },
    #[serde(rename = "parser.validateGraphYaml")]
    ValidateGraphYaml { yaml: String },
    #[serde(rename = "parser.validateToolManifestYaml")]
    ValidateToolManifestYaml { yaml: String },
    #[serde(rename = "parser.validateToolManifestJson")]
    ValidateToolManifestJson { json: String },
    #[serde(rename = "parser.validateHarnessFixtureYaml")]
    ValidateHarnessFixtureYaml { yaml: String },
    #[serde(rename = "parser.parsePacketSchemaDocument")]
    ParsePacketSchemaDocument { path: String, source: String },
    #[serde(rename = "parser.validateSkillArtifactContract")]
    ValidateSkillArtifactContract {
        #[serde(default)]
        artifacts: Option<JsonValue>,
        #[serde(default = "default_artifact_field")]
        field: String,
    },
    #[serde(rename = "parser.resolvePostRunReflectPolicy")]
    ResolvePostRunReflectPolicy {
        #[serde(default)]
        runx: Option<JsonObject>,
        #[serde(default = "default_runx_field")]
        field: String,
    },
    #[serde(rename = "parser.validateSkillInstall")]
    ValidateSkillInstall {
        markdown: String,
        origin: SkillInstallOrigin,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ParserSkillMode {
    #[default]
    Strict,
    Lenient,
}

impl From<ParserSkillMode> for ValidateSkillOptions {
    fn from(mode: ParserSkillMode) -> Self {
        match mode {
            ParserSkillMode::Strict => Self {
                mode: ValidateSkillMode::Strict,
            },
            ParserSkillMode::Lenient => Self {
                mode: ValidateSkillMode::Lenient,
            },
        }
    }
}

fn evaluate_parser_document(document: ParserDocument) -> Result<JsonValue, ParserEvalError> {
    match document {
        ParserDocument::Batch {
            inputs,
            return_errors: true,
        } => Ok(JsonValue::Array(
            inputs
                .into_iter()
                .map(|input| match evaluate_parser_input(input) {
                    Ok(value) => parser_batch_result("success", Some(value), None),
                    Err(error) => parser_batch_result(
                        "failure",
                        None,
                        Some((error.code(), error.to_string())),
                    ),
                })
                .collect(),
        )),
        ParserDocument::Batch {
            inputs,
            return_errors: false,
        } => inputs
            .into_iter()
            .map(evaluate_parser_input)
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        ParserDocument::Input(input) => evaluate_parser_input(*input),
    }
}

fn parser_batch_result(
    status: &str,
    value: Option<JsonValue>,
    error: Option<(&str, String)>,
) -> JsonValue {
    let mut result = JsonObject::new();
    result.insert("status".to_owned(), JsonValue::String(status.to_owned()));
    if let Some(value) = value {
        result.insert("value".to_owned(), value);
    }
    if let Some((code, message)) = error {
        result.insert(
            "error".to_owned(),
            JsonValue::Object(
                [
                    ("code".to_owned(), JsonValue::String(code.to_owned())),
                    ("message".to_owned(), JsonValue::String(message)),
                ]
                .into_iter()
                .collect(),
            ),
        );
    }
    JsonValue::Object(result)
}

fn evaluate_parser_input(input: ParserInput) -> Result<JsonValue, ParserEvalError> {
    match input {
        ParserInput::ValidateSkillMarkdown { markdown, mode } => {
            let raw = parse_skill_markdown(&markdown)?;
            to_json_value(validate_skill_with_options(raw, mode.into())?)
        }
        ParserInput::ValidateRunnerManifestYaml { yaml } => {
            let raw = parse_runner_manifest_yaml(&yaml)?;
            to_json_value(validate_runner_manifest(raw)?)
        }
        ParserInput::ValidateGraphYaml { yaml } => {
            let raw = parse_graph_yaml(&yaml)?;
            to_json_value(validate_graph(raw)?)
        }
        ParserInput::ValidateToolManifestYaml { yaml } => {
            let raw = parse_tool_manifest_yaml(&yaml)?;
            to_json_value(validate_tool_manifest(raw)?)
        }
        ParserInput::ValidateToolManifestJson { json } => {
            let raw = parse_tool_manifest_json(&json)?;
            to_json_value(validate_tool_manifest(raw)?)
        }
        ParserInput::ValidateHarnessFixtureYaml { yaml } => {
            to_json_value(parse_harness_fixture(&yaml)?)
        }
        ParserInput::ParsePacketSchemaDocument { path, source } => {
            let parsed = parse_packet_schema_document(path, &source)
                .map_err(|error| ParserEvalError::Validation(error.to_string()))?;
            to_json_value(parsed)
        }
        ParserInput::ValidateSkillArtifactContract { artifacts, field } => to_json_value(
            validate_skill_artifact_contract(artifacts.as_ref(), &field)?,
        ),
        ParserInput::ResolvePostRunReflectPolicy { runx, field } => {
            to_json_value(resolve_post_run_reflect_policy(runx.as_ref(), &field)?)
        }
        ParserInput::ValidateSkillInstall { markdown, origin } => {
            to_json_value(validate_skill_install(&markdown, origin)?)
        }
    }
}

fn to_json_value<T: Serialize>(value: T) -> Result<JsonValue, ParserEvalError> {
    let serialized = serde_json::to_value(value)
        .map_err(|error| ParserEvalError::SerializeOutput(error.to_string()))?;
    serde_json::from_value(serialized)
        .map_err(|error| ParserEvalError::SerializeOutput(error.to_string()))
}

fn default_artifact_field() -> String {
    "runx.artifacts".to_owned()
}

fn default_runx_field() -> String {
    "runx".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_skill_markdown_validation() -> Result<(), String> {
        let output = evaluate_parser_document_str(
            r#"{
              "kind": "parser.validateSkillMarkdown",
              "markdown": "---\nname: parser-demo\ndescription: Parser demo.\n---\n# Parser Demo\n",
              "mode": "strict"
            }"#,
        )
        .map_err(|error| error.to_string())?;
        let ParserEvalOutput::Output { value } = output;
        let JsonValue::Object(skill) = value else {
            return Err("expected validated skill object".into());
        };
        assert_eq!(
            skill.get("name"),
            Some(&JsonValue::String("parser-demo".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn rejects_unsupported_parser_kind_before_deserializing() -> Result<(), String> {
        let error = match evaluate_parser_document_str(r#"{"kind":"parser.unknown"}"#) {
            Ok(_) => return Err("unsupported parser kind must fail closed".into()),
            Err(error) => error,
        };
        assert_eq!(error.code(), "invalid_input");
        assert!(error.to_string().contains("unsupported parser input kind"));
        Ok(())
    }

    #[test]
    fn evaluates_parser_inputs_in_one_batch() -> Result<(), String> {
        let output = evaluate_parser_document_str(
            r#"{
              "inputs": [
                {
                  "kind": "parser.validateSkillMarkdown",
                  "markdown": "---\nname: first\ndescription: First.\n---\n# First\n"
                },
                {
                  "kind": "parser.validateSkillMarkdown",
                  "markdown": "---\nname: second\ndescription: Second.\n---\n# Second\n"
                }
              ]
            }"#,
        )
        .map_err(|error| error.to_string())?;
        let ParserEvalOutput::Output { value } = output;
        let JsonValue::Array(values) = value else {
            return Err("expected parser batch array".into());
        };
        assert_eq!(values.len(), 2);
        let JsonValue::Object(second) = &values[1] else {
            return Err("expected second validated skill object".into());
        };
        assert_eq!(
            second.get("name"),
            Some(&JsonValue::String("second".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn evaluates_harness_fixture_validation() -> Result<(), String> {
        let output = evaluate_parser_document_str(
            r#"{
              "kind": "parser.validateHarnessFixtureYaml",
              "yaml": "name: fixture\nkind: skill\ntarget: ..\nexpect:\n  status: sealed\n"
            }"#,
        )
        .map_err(|error| error.to_string())?;
        let ParserEvalOutput::Output { value } = output;
        let JsonValue::Object(fixture) = value else {
            return Err("expected validated fixture object".into());
        };
        assert_eq!(
            fixture.get("name"),
            Some(&JsonValue::String("fixture".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn evaluates_packet_schema_parsing_through_the_canonical_owner() -> Result<(), String> {
        let output = evaluate_parser_document_str(
            r#"{
              "kind": "parser.parsePacketSchemaDocument",
              "path": "packets/demo.schema.json",
              "source": "{\"x-runx-packet-id\":\"runx.demo.v1\",\"type\":\"object\"}"
            }"#,
        )
        .map_err(|error| error.to_string())?;
        let ParserEvalOutput::Output { value } = output;
        let JsonValue::Object(schema) = value else {
            return Err("expected validated packet schema object".into());
        };
        assert_eq!(
            schema.get("packetId"),
            Some(&JsonValue::String("runx.demo.v1".to_owned()))
        );
        assert!(schema.contains_key("sha256"));
        Ok(())
    }
}
