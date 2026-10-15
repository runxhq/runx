use std::collections::BTreeMap;
use std::fmt;

use runx_contracts::JsonNumber;
use runx_contracts::{JsonObject, JsonValue};
use runx_parser::SkillInput;

/// Apply manifest defaults without overwriting a value the caller supplied.
pub(crate) fn apply_defaults(declared: &BTreeMap<String, SkillInput>, inputs: &mut JsonObject) {
    for (name, input) in declared {
        if !inputs.contains_key(name)
            && let Some(default) = &input.default
        {
            inputs.insert(name.clone(), default.clone());
        }
    }
}

/// Return required manifest inputs that are absent or explicitly null.
pub(crate) fn missing_required(
    declared: &BTreeMap<String, SkillInput>,
    inputs: &JsonObject,
) -> Vec<String> {
    declared
        .iter()
        .filter(|(name, input)| {
            input.required && input.default.is_none() && is_missing(inputs.get(*name))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

pub(crate) fn is_missing(value: Option<&JsonValue>) -> bool {
    matches!(value, None | Some(JsonValue::Null))
}

/// Build the one invocation map a declared local tool receives.
///
/// Runtime-resolved values have precedence over static values. Undeclared
/// values are not ambient tool input. Defaults, required fields, artifact
/// projection, and declared JSON types are enforced before any process starts.
pub(crate) fn materialize_tool_inputs(
    declared: &BTreeMap<String, SkillInput>,
    static_inputs: &JsonObject,
    resolved_inputs: &JsonObject,
) -> Result<JsonObject, InputContractError> {
    materialize_declared_inputs(
        declared,
        static_inputs,
        resolved_inputs,
        "tool",
        MissingRequired::Reject,
        UnknownInputs::Project,
    )
}

/// Materialize the declared boundary of a nested runner from the parent
/// graph's ambient parameter map. Parent-only parameters are not child input;
/// explicitly mapped values still receive the child's complete validation.
pub(crate) fn materialize_nested_runner_inputs(
    declared: &BTreeMap<String, SkillInput>,
    supplied: &JsonObject,
) -> Result<JsonObject, InputContractError> {
    materialize_declared_inputs(
        declared,
        supplied,
        &JsonObject::new(),
        "runner",
        MissingRequired::Reject,
        UnknownInputs::Project,
    )
}

/// Validate and normalize all values already supplied by a caller while
/// preserving absent required inputs for the existing resolution flow.
pub(crate) fn materialize_present_runner_inputs(
    declared: &BTreeMap<String, SkillInput>,
    supplied: &JsonObject,
) -> Result<JsonObject, InputContractError> {
    materialize_declared_inputs(
        declared,
        supplied,
        &JsonObject::new(),
        "runner",
        MissingRequired::Preserve,
        UnknownInputs::Reject,
    )
}

/// Admit a complete top-level runner invocation.
///
/// Unlike the preparation front, which preserves absent required values long
/// enough to return a blocked context and refusal receipt, execution and
/// harness replay must receive every required value and must reject ambient,
/// undeclared inputs.
pub(crate) fn materialize_complete_runner_inputs(
    declared: &BTreeMap<String, SkillInput>,
    supplied: &JsonObject,
) -> Result<JsonObject, InputContractError> {
    materialize_declared_inputs(
        declared,
        supplied,
        &JsonObject::new(),
        "runner",
        MissingRequired::Reject,
        UnknownInputs::Reject,
    )
}

/// Check one value an `owner` input takes, as admitting the whole invocation
/// would check it.
pub(crate) fn admit_input_value(
    owner: &'static str,
    name: &str,
    input: &SkillInput,
    value: &JsonValue,
) -> Result<(), InputContractError> {
    materialize_input(owner, name, input, Some(value), MissingRequired::Reject).map(|_| ())
}

fn materialize_declared_inputs(
    declared: &BTreeMap<String, SkillInput>,
    static_inputs: &JsonObject,
    resolved_inputs: &JsonObject,
    owner: &'static str,
    missing_required: MissingRequired,
    unknown_inputs: UnknownInputs,
) -> Result<JsonObject, InputContractError> {
    let mut supplied = static_inputs.clone();
    supplied.extend(resolved_inputs.clone());

    if unknown_inputs == UnknownInputs::Reject
        && let Some(name) = supplied.keys().find(|name| !declared.contains_key(*name))
    {
        return Err(InputContractError::new(
            owner,
            name,
            format!("/{name}"),
            format!("{owner} input '{name}' is not declared"),
            JsonValue::Object(runx_contracts::input_contract_schema(declared)),
        ));
    }

    declared
        .iter()
        .filter_map(|(name, input)| {
            materialize_input(owner, name, input, supplied.get(name), missing_required).transpose()
        })
        .collect()
}

fn materialize_input(
    owner: &'static str,
    name: &str,
    input: &SkillInput,
    supplied: Option<&JsonValue>,
    missing_required: MissingRequired,
) -> Result<Option<(String, JsonValue)>, InputContractError> {
    let value = supplied.or(input.default.as_ref());
    let Some(value) = value else {
        return if input.required && missing_required == MissingRequired::Reject {
            Err(input_error(
                owner,
                name,
                format!("{owner} input '{name}' is required"),
                input,
            ))
        } else {
            Ok(None)
        };
    };
    if matches!(value, JsonValue::Null) {
        return if input.required && missing_required == MissingRequired::Reject {
            Err(input_error(
                owner,
                name,
                format!("{owner} input '{name}' is required"),
                input,
            ))
        } else {
            Ok(None)
        };
    }

    let value = if input.artifact == Some(true) {
        unwrap_artifact(value, name).map_err(|message| input_error(owner, name, message, input))?
    } else {
        value.clone()
    };
    if matches!(value, JsonValue::Null) {
        return if input.required && missing_required == MissingRequired::Reject {
            Err(input_error(
                owner,
                name,
                format!("{owner} input '{name}' is required"),
                input,
            ))
        } else {
            Ok(None)
        };
    }
    if !input.accepts_value(&value) {
        return Err(input_error(
            owner,
            name,
            format!(
                "{owner} input '{name}' must be {}, received {}",
                input.input_type,
                json_type(&value),
            ),
            input,
        ));
    }
    if input.schema.is_some() {
        validate_schema_value(owner, name, input, &value)?;
    }
    Ok(Some((name.to_owned(), value)))
}

fn validate_schema_value(
    owner: &'static str,
    name: &str,
    input: &SkillInput,
    value: &JsonValue,
) -> Result<(), InputContractError> {
    let accepted_schema = JsonValue::Object(input.effective_schema());
    let schema = serde_json::to_value(&accepted_schema).map_err(|error| {
        input_error(
            owner,
            name,
            format!("declared input schema could not be serialized: {error}"),
            input,
        )
    })?;
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| {
            input_error(
                owner,
                name,
                format!("declared input schema is invalid: {error}"),
                input,
            )
        })?;
    let instance = serde_json::to_value(value).map_err(|error| {
        input_error(
            owner,
            name,
            format!("input value could not be serialized: {error}"),
            input,
        )
    })?;
    let Some(error) = validator.iter_errors(&instance).next() else {
        return Ok(());
    };
    let nested = error.instance_path().as_str();
    let path = if nested.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name}{nested}")
    };
    let schema_path = error.schema_path().as_str();
    let message = if schema_path.is_empty() {
        "value does not satisfy the declared schema".to_owned()
    } else {
        format!("value does not satisfy the declared schema at '{schema_path}'")
    };
    Err(InputContractError::new(
        owner,
        name,
        path,
        message,
        accepted_schema,
    ))
}

fn input_error(
    owner: &'static str,
    name: &str,
    message: impl Into<String>,
    input: &SkillInput,
) -> InputContractError {
    InputContractError::new(
        owner,
        name,
        format!("/{name}"),
        message,
        JsonValue::Object(input.effective_schema()),
    )
}

/// An input declared `artifact` arrives in the canonical artifact envelope
/// a step emits, `{schema?, data}`; its value is the envelope's data.
fn unwrap_artifact(value: &JsonValue, name: &str) -> Result<JsonValue, String> {
    match value {
        JsonValue::Object(object)
            if object.contains_key("data")
                && object.keys().all(|key| key == "data" || key == "schema") =>
        {
            Ok(object.get("data").cloned().unwrap_or(JsonValue::Null))
        }
        _ => Err(format!(
            "input '{name}' is an artifact; pass its envelope {{\"schema\", \"data\"}}"
        )),
    }
}

fn json_type(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(JsonNumber::I64(_) | JsonNumber::U64(_)) => "integer",
        JsonValue::Number(JsonNumber::F64(_)) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MissingRequired {
    Reject,
    Preserve,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnknownInputs {
    Reject,
    Project,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InputContractError {
    owner: &'static str,
    input: String,
    path: String,
    message: String,
    accepted_schema: JsonValue,
}

impl InputContractError {
    fn new(
        owner: &'static str,
        input: impl Into<String>,
        path: impl Into<String>,
        message: impl Into<String>,
        accepted_schema: JsonValue,
    ) -> Self {
        Self {
            owner,
            input: input.into(),
            path: path.into(),
            message: message.into(),
            accepted_schema,
        }
    }

    pub(crate) fn into_runtime_error(self) -> crate::RuntimeError {
        crate::RuntimeError::InputContract {
            step_id: None,
            owner: self.owner,
            input: self.input,
            path: self.path,
            message: self.message,
            accepted_schema: Box::new(self.accepted_schema),
        }
    }
}

impl fmt::Display for InputContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} input contract failed at '{}': {}",
            self.owner, self.path, self.message
        )
    }
}

impl std::error::Error for InputContractError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use runx_contracts::{InputDefinition, JsonNumber, JsonObject, JsonValue};

    use super::{
        materialize_complete_runner_inputs, materialize_nested_runner_inputs,
        materialize_present_runner_inputs,
    };

    fn string_input(required: bool) -> InputDefinition {
        InputDefinition {
            input_type: "string".to_owned(),
            required,
            description: None,
            default: None,
            artifact: None,
            packet: None,
            schema: None,
        }
    }

    #[test]
    fn nested_runner_projects_its_contract_from_parent_graph_parameters()
    -> Result<(), Box<dyn std::error::Error>> {
        let declared = BTreeMap::from([("objective".to_owned(), string_input(true))]);
        let supplied = JsonObject::from([
            (
                "objective".to_owned(),
                JsonValue::String("Bounded question".to_owned()),
            ),
            (
                "parent_only".to_owned(),
                JsonValue::String("must not cross the child boundary".to_owned()),
            ),
        ]);

        let nested = materialize_nested_runner_inputs(&declared, &supplied)?;
        assert_eq!(nested.len(), 1);
        assert_eq!(
            nested.get("objective").and_then(JsonValue::as_str),
            Some("Bounded question")
        );
        assert!(materialize_present_runner_inputs(&declared, &supplied).is_err());
        Ok(())
    }

    #[test]
    fn nested_contract_error_names_the_failing_path_and_accepted_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let resources = InputDefinition {
            input_type: "object".to_owned(),
            required: true,
            description: Some("Bounded issue selector.".to_owned()),
            default: None,
            artifact: None,
            packet: None,
            schema: Some(JsonObject::from([
                (
                    "required".to_owned(),
                    JsonValue::Array(vec![JsonValue::String("filters".to_owned())]),
                ),
                (
                    "properties".to_owned(),
                    JsonValue::Object(JsonObject::from([(
                        "filters".to_owned(),
                        JsonValue::Object(JsonObject::from([
                            ("type".to_owned(), JsonValue::String("object".to_owned())),
                            (
                                "properties".to_owned(),
                                JsonValue::Object(JsonObject::from([(
                                    "limit".to_owned(),
                                    JsonValue::Object(JsonObject::from([
                                        (
                                            "type".to_owned(),
                                            JsonValue::String("integer".to_owned()),
                                        ),
                                        (
                                            "maximum".to_owned(),
                                            JsonValue::Number(JsonNumber::U64(25)),
                                        ),
                                    ])),
                                )])),
                            ),
                        ])),
                    )])),
                ),
            ])),
        };
        let declared = BTreeMap::from([("resources".to_owned(), resources)]);
        let supplied = JsonObject::from([(
            "resources".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "filters".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "limit".to_owned(),
                    JsonValue::Number(JsonNumber::U64(26)),
                )])),
            )])),
        )]);

        let Err(error) = materialize_present_runner_inputs(&declared, &supplied) else {
            return Err("out-of-range nested input must fail".into());
        };
        let error = error.into_runtime_error();
        let crate::RuntimeError::InputContract {
            path,
            accepted_schema,
            ..
        } = error
        else {
            return Err("expected input-contract error".into());
        };
        assert_eq!(path, "/resources/filters/limit");
        let accepted_schema = serde_json::to_value(accepted_schema)?;
        assert_eq!(
            accepted_schema["properties"]["filters"]["properties"]["limit"]["maximum"],
            25
        );
        Ok(())
    }

    #[test]
    fn complete_runner_rejects_missing_required_and_conditional_nested_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let resources = InputDefinition {
            input_type: "object".to_owned(),
            required: true,
            description: Some("Bounded GitHub resources.".to_owned()),
            default: None,
            artifact: None,
            packet: None,
            schema: Some(JsonObject::from([
                ("type".to_owned(), JsonValue::String("object".to_owned())),
                (
                    "required".to_owned(),
                    JsonValue::Array(vec![JsonValue::String("kind".to_owned())]),
                ),
                (
                    "properties".to_owned(),
                    JsonValue::Object(JsonObject::from([(
                        "kind".to_owned(),
                        JsonValue::Object(JsonObject::from([(
                            "type".to_owned(),
                            JsonValue::String("string".to_owned()),
                        )])),
                    )])),
                ),
                (
                    "allOf".to_owned(),
                    JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                        (
                            "if".to_owned(),
                            JsonValue::Object(JsonObject::from([(
                                "properties".to_owned(),
                                JsonValue::Object(JsonObject::from([(
                                    "kind".to_owned(),
                                    JsonValue::Object(JsonObject::from([(
                                        "const".to_owned(),
                                        JsonValue::String("prs".to_owned()),
                                    )])),
                                )])),
                            )])),
                        ),
                        (
                            "then".to_owned(),
                            JsonValue::Object(JsonObject::from([(
                                "properties".to_owned(),
                                JsonValue::Object(JsonObject::from([(
                                    "base".to_owned(),
                                    JsonValue::Object(JsonObject::from([(
                                        "type".to_owned(),
                                        JsonValue::String("string".to_owned()),
                                    )])),
                                )])),
                            )])),
                        ),
                    ]))]),
                ),
            ])),
        };
        let direction = string_input(true);
        let declared = BTreeMap::from([
            ("direction".to_owned(), direction),
            ("resources".to_owned(), resources),
        ]);
        let missing_direction = JsonObject::from([(
            "resources".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "kind".to_owned(),
                JsonValue::String("issues".to_owned()),
            )])),
        )]);
        assert!(
            materialize_complete_runner_inputs(&declared, &missing_direction).is_err(),
            "execution admission must not preserve missing required inputs"
        );

        let invalid_conditional = JsonObject::from([
            ("direction".to_owned(), JsonValue::String("push".to_owned())),
            (
                "resources".to_owned(),
                JsonValue::Object(JsonObject::from([
                    ("kind".to_owned(), JsonValue::String("prs".to_owned())),
                    ("base".to_owned(), JsonValue::Bool(true)),
                ])),
            ),
        ]);
        let Err(error) = materialize_complete_runner_inputs(&declared, &invalid_conditional) else {
            return Err("conditional nested schemas must be enforced".into());
        };
        let error = error.into_runtime_error();
        assert!(matches!(
            error,
            crate::RuntimeError::InputContract { ref path, .. } if path == "/resources/base"
        ));
        Ok(())
    }
}
