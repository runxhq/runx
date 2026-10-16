use runx_contracts::{JsonObject, JsonValue};

use crate::ValidationError;
use crate::graph::{RawGraphIr, validate_graph_document};

use super::{FIELDS, SkillRunnerDefinition, validate_inputs};

const RUNNER_FIELDS: &[&str] = &[
    "default",
    "continuations",
    "examples",
    "inputs",
    "outputs",
    "policy",
    "result_from",
    "steps",
];

/// The graph keys a runner carries for its steps.
const GRAPH_FIELDS: &[&str] = &["policy", "result_from", "steps"];

pub(crate) fn validate_runner_definition(
    skill: &str,
    name: &str,
    runner: JsonObject,
) -> Result<SkillRunnerDefinition, ValidationError> {
    FIELDS.reject_unknown_fields(&runner, &format!("runners.{name}"), RUNNER_FIELDS)?;
    let inputs = validate_inputs(
        FIELDS
            .optional_object(runner.get("inputs"), &format!("runners.{name}.inputs"))?
            .unwrap_or_default(),
        &format!("runners.{name}.inputs"),
    )?;
    let examples = parse_runner_examples(name, runner.get("examples"))?;
    validate_input_examples(&format!("runners.{name}.examples"), &examples, &inputs)?;
    Ok(SkillRunnerDefinition {
        name: name.to_owned(),
        continuations: FIELDS
            .optional_string_array(
                runner.get("continuations"),
                &format!("runners.{name}.continuations"),
            )?
            .unwrap_or_default(),
        default: FIELDS
            .optional_bool(runner.get("default"), &format!("runners.{name}.default"))?
            .unwrap_or(false),
        inputs,
        examples,
        graph: runner_graph(skill, name, &runner)?,
        raw: runner,
    })
}

/// The runner's steps as the graph the engine runs, named for its skill so
/// receipts carry the skill's identity. `result_from` defaults to the last
/// step. A runner without steps is one agent turn: its id and task are the
/// runner's name, its role the skill's, its inputs the run's, and its outputs
/// the runner's.
fn runner_graph(
    skill: &str,
    name: &str,
    runner: &JsonObject,
) -> Result<crate::ExecutionGraph, ValidationError> {
    let mut document = runner
        .iter()
        .filter(|(key, _)| GRAPH_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<JsonObject>();
    match (runner.contains_key("steps"), runner.get("outputs")) {
        (true, Some(_)) => {
            return Err(FIELDS.validation_error(format!(
                "runners.{name}.outputs belong to a runner without steps; a composed runner returns its result_from steps."
            )));
        }
        (true, None) => {}
        (false, None) => {
            return Err(FIELDS.validation_error(format!(
                "runners.{name} has no steps, so it is one agent turn and declares its outputs."
            )));
        }
        (false, outputs) => {
            let mut turn = JsonObject::from([
                ("id".to_owned(), JsonValue::String(name.to_owned())),
                (
                    "agent".to_owned(),
                    JsonValue::Object(JsonObject::from([
                        ("role".to_owned(), JsonValue::String(skill.to_owned())),
                        ("task".to_owned(), JsonValue::String(name.to_owned())),
                    ])),
                ),
                ("inputs".to_owned(), JsonValue::String("$input".to_owned())),
            ]);
            if let Some(outputs) = outputs {
                turn.insert("outputs".to_owned(), outputs.clone());
            }
            document.insert(
                "steps".to_owned(),
                JsonValue::Array(vec![JsonValue::Object(turn)]),
            );
        }
    }
    document.insert("name".to_owned(), JsonValue::String(skill.to_owned()));
    if !document.contains_key("result_from")
        && let Some(last) = document
            .get("steps")
            .and_then(JsonValue::as_array)
            .and_then(|steps| steps.last())
            .and_then(JsonValue::as_object)
            .and_then(|step| step.get("id"))
            .cloned()
    {
        document.insert("result_from".to_owned(), JsonValue::Array(vec![last]));
    }
    validate_graph_document(document.clone(), Some(RawGraphIr { document }))
        .map_err(|error| FIELDS.validation_error(format!("runners.{name}: {error}")))
}

fn parse_runner_examples(
    name: &str,
    value: Option<&JsonValue>,
) -> Result<Vec<JsonObject>, ValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let field = format!("runners.{name}.examples");
    let values = FIELDS.required_plain_array(Some(value), &field)?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            FIELDS
                .required_object(Some(value), &format!("{field}[{index}]"))
                .cloned()
        })
        .collect::<Result<Vec<_>, _>>()
}

/// Validate already-parsed, copy-valid examples against the canonical input
/// contract. Runtime packet hydration calls this same function after replacing
/// packet references with their catalog-owned schemas.
pub fn validate_input_examples(
    field: &str,
    examples: &[JsonObject],
    inputs: &std::collections::BTreeMap<String, super::SkillInput>,
) -> Result<(), ValidationError> {
    let schema =
        serde_json::to_value(runx_contracts::input_contract_schema(inputs)).map_err(|error| {
            FIELDS.validation_error(format!("{field} schema could not be serialized: {error}"))
        })?;
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| FIELDS.validation_error(format!("{field} schema is invalid: {error}")))?;
    for (index, example) in examples.iter().enumerate() {
        let instance = serde_json::to_value(example).map_err(|error| {
            FIELDS.validation_error(format!("{field}[{index}] could not be serialized: {error}"))
        })?;
        if let Some(error) = validator.iter_errors(&instance).next() {
            return Err(FIELDS.validation_error(format!(
                "{field}[{index}]{} does not match the runner input contract: {error}",
                error.instance_path()
            )));
        }
    }
    Ok(())
}
