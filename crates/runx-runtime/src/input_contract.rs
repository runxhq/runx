use std::collections::BTreeMap;
use std::fmt;

use runx_contracts::JsonNumber;
use runx_contracts::{JsonObject, JsonValue};
use runx_parser::SkillInput;

/// Whether an input explicitly takes the descriptor itself. Native typed
/// inputs carry their identity in the accepted schema even when they do not
/// use a manifest packet annotation. An unconstrained JSON input still takes
/// the referenced content.
pub(crate) fn takes_descriptor(input: &runx_contracts::InputDefinition, schema: &str) -> bool {
    if input
        .packet
        .as_ref()
        .is_some_and(|packet| packet.as_str() == schema)
    {
        return true;
    }
    let identity = input
        .schema
        .as_ref()
        .and_then(|shape| shape.get("properties"))
        .and_then(JsonValue::as_object)
        .and_then(|properties| properties.get("schema"))
        .and_then(JsonValue::as_object);
    identity.is_some_and(|identity| {
        identity.get("const").and_then(JsonValue::as_str) == Some(schema)
            || identity
                .get("enum")
                .and_then(JsonValue::as_array)
                .is_some_and(|values| values.len() == 1 && values[0].as_str() == Some(schema))
    })
}

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

/// Return required manifest inputs that are absent. A supplied null is a
/// concrete JSON value, checked against the input's type and schema.
pub(crate) fn missing_required(
    declared: &BTreeMap<String, SkillInput>,
    inputs: &JsonObject,
) -> Vec<String> {
    declared
        .iter()
        .filter(|(name, input)| {
            input.required && input.default.is_none() && !inputs.contains_key(*name)
        })
        .map(|(name, _)| name.clone())
        .collect()
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

/// Materialize the declared boundary of a nested method from the parent
/// workflow's ambient parameter map. Parent-only parameters are not child input;
/// explicitly mapped values still receive the child's complete validation.
pub(crate) fn materialize_nested_method_inputs(
    declared: &BTreeMap<String, SkillInput>,
    constraints: &JsonObject,
    supplied: &JsonObject,
) -> Result<JsonObject, InputContractError> {
    let inputs = materialize_declared_inputs(
        declared,
        supplied,
        &JsonObject::new(),
        "method",
        MissingRequired::Reject,
        UnknownInputs::Project,
    )?;
    admit_constraints(declared, constraints, &inputs)?;
    Ok(inputs)
}

/// Validate and normalize all values already supplied by a caller while
/// preserving absent required inputs for the existing resolution flow.
pub(crate) fn materialize_present_method_inputs(
    declared: &BTreeMap<String, SkillInput>,
    supplied: &JsonObject,
) -> Result<JsonObject, InputContractError> {
    materialize_declared_inputs(
        declared,
        supplied,
        &JsonObject::new(),
        "method",
        MissingRequired::Preserve,
        UnknownInputs::Reject,
    )
}

/// Admit a complete top-level method invocation.
///
/// Unlike the preparation front, which preserves absent required values long
/// enough to return a blocked context and refusal receipt, execution and
/// test replay must receive every required value and must reject ambient,
/// undeclared inputs.
pub(crate) fn materialize_complete_method_inputs(
    declared: &BTreeMap<String, SkillInput>,
    constraints: &JsonObject,
    supplied: &JsonObject,
) -> Result<JsonObject, InputContractError> {
    let inputs = materialize_declared_inputs(
        declared,
        supplied,
        &JsonObject::new(),
        "method",
        MissingRequired::Reject,
        UnknownInputs::Reject,
    )?;
    admit_constraints(declared, constraints, &inputs)?;
    Ok(inputs)
}

/// Check a method's whole input object against its constraints across
/// fields (`input_constraints`), once each field has passed its own check,
/// so a call that breaks them is refused before anything runs.
fn admit_constraints(
    declared: &BTreeMap<String, SkillInput>,
    constraints: &JsonObject,
    inputs: &JsonObject,
) -> Result<(), InputContractError> {
    if constraints.is_empty() {
        return Ok(());
    }
    let accepted = JsonValue::Object(runx_parser::constrained_input_schema(declared, constraints));
    let refusal = |path: String, message: String| {
        InputContractError::new(
            "method",
            "input_constraints",
            path,
            message,
            accepted.clone(),
        )
    };
    let schema = serde_json::to_value(JsonValue::Object(constraints.clone())).map_err(|error| {
        refusal(
            "/".to_owned(),
            format!("the input constraints could not be serialized: {error}"),
        )
    })?;
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| {
            refusal(
                "/".to_owned(),
                format!("the input constraints are invalid: {error}"),
            )
        })?;
    let instance = serde_json::to_value(inputs).map_err(|error| {
        refusal(
            "/".to_owned(),
            format!("the inputs could not be serialized: {error}"),
        )
    })?;
    let Some(error) = validator.iter_errors(&instance).next() else {
        return Ok(());
    };
    let path = match error.instance_path().as_str() {
        "" => "/".to_owned(),
        path => path.to_owned(),
    };
    Err(refusal(
        path,
        format!("the inputs together break the method's input constraints: {error}"),
    ))
}

/// Check one value an `declared_by` input takes, as admitting the whole invocation
/// would check it.
pub(crate) fn admit_input_value(
    declared_by: &'static str,
    name: &str,
    input: &SkillInput,
    value: &JsonValue,
) -> Result<(), InputContractError> {
    materialize_input(
        declared_by,
        name,
        input,
        Some(value),
        MissingRequired::Reject,
    )
    .map(|_| ())
}

fn materialize_declared_inputs(
    declared: &BTreeMap<String, SkillInput>,
    static_inputs: &JsonObject,
    resolved_inputs: &JsonObject,
    declared_by: &'static str,
    missing_required: MissingRequired,
    unknown_inputs: UnknownInputs,
) -> Result<JsonObject, InputContractError> {
    let mut supplied = static_inputs.clone();
    supplied.extend(resolved_inputs.clone());

    if unknown_inputs == UnknownInputs::Reject {
        reject_unknown_inputs(declared, &supplied, declared_by)?;
    }

    declared
        .iter()
        .filter_map(|(name, input)| {
            materialize_input(
                declared_by,
                name,
                input,
                supplied.get(name),
                missing_required,
            )
            .transpose()
        })
        .collect()
}

/// An explicit call has arguments, not an ambient parameter map. Reject an
/// unknown name before projection could discard a misspelled constraint.
pub(crate) fn reject_unknown_inputs(
    declared: &BTreeMap<String, SkillInput>,
    supplied: &JsonObject,
    declared_by: &'static str,
) -> Result<(), InputContractError> {
    if let Some(name) = supplied.keys().find(|name| !declared.contains_key(*name)) {
        return Err(InputContractError::new(
            declared_by,
            name,
            format!("/{name}"),
            format!("{declared_by} input '{name}' is not declared"),
            JsonValue::Object(runx_contracts::input_contract_schema(declared)),
        ));
    }
    Ok(())
}

fn materialize_input(
    declared_by: &'static str,
    name: &str,
    input: &SkillInput,
    supplied: Option<&JsonValue>,
    missing_required: MissingRequired,
) -> Result<Option<(String, JsonValue)>, InputContractError> {
    let value = supplied.or(input.default.as_ref());
    let Some(value) = value else {
        return if input.required && missing_required == MissingRequired::Reject {
            Err(input_error(
                declared_by,
                name,
                format!("{declared_by} input '{name}' is required"),
                input,
            ))
        } else {
            Ok(None)
        };
    };
    let value = value.clone();
    if !input.accepts_value(&value) {
        return Err(input_error(
            declared_by,
            name,
            format!(
                "{declared_by} input '{name}' must be {}, received {}",
                input.input_type,
                json_type(&value),
            ),
            input,
        ));
    }
    if input.schema.is_some() {
        validate_schema_value(declared_by, name, input, &value)?;
    }
    Ok(Some((name.to_owned(), value)))
}

fn validate_schema_value(
    declared_by: &'static str,
    name: &str,
    input: &SkillInput,
    value: &JsonValue,
) -> Result<(), InputContractError> {
    let accepted_schema = JsonValue::Object(input.effective_schema());
    let schema = serde_json::to_value(&accepted_schema).map_err(|error| {
        input_error(
            declared_by,
            name,
            format!("declared input schema could not be serialized: {error}"),
            input,
        )
    })?;
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| {
            input_error(
                declared_by,
                name,
                format!("declared input schema is invalid: {error}"),
                input,
            )
        })?;
    let instance = serde_json::to_value(value).map_err(|error| {
        input_error(
            declared_by,
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
        declared_by,
        name,
        path,
        message,
        accepted_schema,
    ))
}

fn input_error(
    declared_by: &'static str,
    name: &str,
    message: impl Into<String>,
    input: &SkillInput,
) -> InputContractError {
    InputContractError::new(
        declared_by,
        name,
        format!("/{name}"),
        message,
        JsonValue::Object(input.effective_schema()),
    )
}

/// An input declared `artifact` arrives in the canonical artifact envelope
/// a step emits, `{schema?, data}`; its value is the envelope's data.
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
    declared_by: &'static str,
    input: String,
    path: String,
    message: String,
    accepted_schema: JsonValue,
}

impl InputContractError {
    fn new(
        declared_by: &'static str,
        input: impl Into<String>,
        path: impl Into<String>,
        message: impl Into<String>,
        accepted_schema: JsonValue,
    ) -> Self {
        Self {
            declared_by,
            input: input.into(),
            path: path.into(),
            message: message.into(),
            accepted_schema,
        }
    }

    pub(crate) fn into_runtime_error(self) -> crate::RuntimeError {
        crate::RuntimeError::InputContract {
            step_id: None,
            declared_by: self.declared_by,
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
            self.declared_by, self.path, self.message
        )
    }
}

impl std::error::Error for InputContractError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use runx_contracts::{InputDefinition, JsonNumber, JsonObject, JsonValue};

    use super::{
        materialize_complete_method_inputs, materialize_nested_method_inputs,
        materialize_present_method_inputs, materialize_tool_inputs, missing_required,
    };

    fn string_input(required: bool) -> InputDefinition {
        InputDefinition {
            input_type: "string".to_owned(),
            required,
            description: None,
            default: None,
            packet: None,
            schema: None,
        }
    }

    #[test]
    fn declared_json_null_is_preserved_across_input_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut definition = string_input(true);
        definition.input_type = "json".to_owned();
        let supplied = JsonObject::from([("value".to_owned(), JsonValue::Null)]);
        for default in [None, Some(JsonValue::Null), Some(JsonValue::Bool(false))] {
            definition.default = default;
            let declared = BTreeMap::from([("value".to_owned(), definition.clone())]);
            assert!(missing_required(&declared, &supplied).is_empty());
            for actual in [
                materialize_present_method_inputs(&declared, &supplied)?,
                materialize_complete_method_inputs(&declared, &JsonObject::new(), &supplied)?,
                materialize_nested_method_inputs(&declared, &JsonObject::new(), &supplied)?,
                materialize_tool_inputs(&declared, &supplied, &JsonObject::new())?,
            ] {
                assert_eq!(actual, supplied);
            }
        }
        Ok(())
    }

    #[test]
    fn explicit_null_is_checked_against_type_and_nullable_schema()
    -> Result<(), Box<dyn std::error::Error>> {
        let supplied = JsonObject::from([("value".to_owned(), JsonValue::Null)]);
        for required in [false, true] {
            let declared = BTreeMap::from([("value".to_owned(), string_input(required))]);
            assert!(materialize_present_method_inputs(&declared, &supplied).is_err());
        }
        let mut definition = string_input(true);
        definition.input_type = "json".to_owned();
        for (schema, accepted) in [
            (
                serde_json::json!({"anyOf": [{"type": "string"}, {"type": "null"}]}),
                true,
            ),
            (serde_json::json!({"type": "object"}), false),
        ] {
            definition.schema = Some(serde_json::from_value(schema)?);
            let declared = BTreeMap::from([("value".to_owned(), definition.clone())]);
            assert_eq!(
                materialize_present_method_inputs(&declared, &supplied).is_ok(),
                accepted
            );
        }
        Ok(())
    }

    #[test]
    fn an_absent_input_uses_a_declared_null_default_but_is_otherwise_absent()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut definition = string_input(false);
        definition.input_type = "json".to_owned();
        let absent = JsonObject::new();
        let declared = BTreeMap::from([("value".to_owned(), definition.clone())]);
        assert_eq!(
            materialize_present_method_inputs(&declared, &absent)?,
            absent
        );
        definition.required = true;
        let declared = BTreeMap::from([("value".to_owned(), definition.clone())]);
        assert_eq!(missing_required(&declared, &absent), ["value"]);
        assert!(materialize_complete_method_inputs(&declared, &absent, &absent).is_err());
        definition.default = Some(JsonValue::Null);
        let declared = BTreeMap::from([("value".to_owned(), definition)]);
        assert!(missing_required(&declared, &absent).is_empty());
        assert_eq!(
            materialize_complete_method_inputs(&declared, &absent, &absent)?,
            JsonObject::from([("value".to_owned(), JsonValue::Null)])
        );
        Ok(())
    }

    #[test]
    fn nested_method_projects_its_contract_from_parent_workflow_parameters()
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

        let nested = materialize_nested_method_inputs(&declared, &JsonObject::new(), &supplied)?;
        assert_eq!(nested.len(), 1);
        assert_eq!(
            nested.get("objective").and_then(JsonValue::as_str),
            Some("Bounded question")
        );
        assert!(materialize_present_method_inputs(&declared, &supplied).is_err());
        Ok(())
    }

    #[test]
    fn workflow_tool_inputs_still_project_ambient_parameters()
    -> Result<(), Box<dyn std::error::Error>> {
        let declared = BTreeMap::from([("objective".to_owned(), string_input(true))]);
        let supplied = JsonObject::from([
            (
                "objective".to_owned(),
                JsonValue::String("selected".to_owned()),
            ),
            ("parent_only".to_owned(), JsonValue::Bool(true)),
        ]);
        assert_eq!(
            materialize_tool_inputs(&declared, &supplied, &JsonObject::new())?,
            JsonObject::from([(
                "objective".to_owned(),
                JsonValue::String("selected".to_owned())
            )]),
        );
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

        let Err(error) = materialize_present_method_inputs(&declared, &supplied) else {
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
    fn complete_method_rejects_missing_required_and_conditional_nested_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let resources = InputDefinition {
            input_type: "object".to_owned(),
            required: true,
            description: Some("Bounded GitHub resources.".to_owned()),
            default: None,
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
            materialize_complete_method_inputs(&declared, &JsonObject::new(), &missing_direction)
                .is_err(),
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
        let Err(error) =
            materialize_complete_method_inputs(&declared, &JsonObject::new(), &invalid_conditional)
        else {
            return Err("conditional nested schemas must be enforced".into());
        };
        let error = error.into_runtime_error();
        assert!(matches!(
            error,
            crate::RuntimeError::InputContract { ref path, .. } if path == "/resources/base"
        ));
        Ok(())
    }

    #[test]
    fn a_method_admits_only_inputs_that_meet_its_constraints_across_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let declared = BTreeMap::from([
            ("url".to_owned(), string_input(false)),
            ("owner".to_owned(), string_input(false)),
            ("name".to_owned(), string_input(false)),
        ]);
        let required = |names: &[&str]| {
            JsonValue::Object(JsonObject::from([(
                "required".to_owned(),
                JsonValue::Array(
                    names
                        .iter()
                        .map(|name| JsonValue::String((*name).to_owned()))
                        .collect(),
                ),
            )]))
        };
        let constraints = JsonObject::from([(
            "oneOf".to_owned(),
            JsonValue::Array(vec![required(&["url"]), required(&["owner", "name"])]),
        )]);
        let call = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), JsonValue::String((*value).to_owned())))
                .collect::<JsonObject>()
        };

        for admitted in [
            call(&[("url", "https://github.com/acme/app")]),
            call(&[("owner", "acme"), ("name", "app")]),
        ] {
            materialize_complete_method_inputs(&declared, &constraints, &admitted)?;
            materialize_nested_method_inputs(&declared, &constraints, &admitted)?;
        }
        for refused in [
            call(&[("owner", "acme")]),
            call(&[
                ("url", "https://github.com/acme/app"),
                ("owner", "acme"),
                ("name", "app"),
            ]),
            JsonObject::new(),
        ] {
            for outcome in [
                materialize_complete_method_inputs(&declared, &constraints, &refused),
                materialize_nested_method_inputs(&declared, &constraints, &refused),
            ] {
                let Err(error) = outcome else {
                    return Err(format!("{refused:?} met constraints it breaks").into());
                };
                assert!(
                    matches!(
                        error.clone().into_runtime_error(),
                        crate::RuntimeError::InputContract { ref input, ref path, .. }
                            if input == "input_constraints" && path == "/"
                    ),
                    "{error}"
                );
            }
        }
        Ok(())
    }
}
