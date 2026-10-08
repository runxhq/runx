use runx_contracts::{JsonObject, JsonValue};

use crate::ValidationError;
use crate::graph::{RawGraphIr, validate_graph_document};

use super::{ActDeclaration, FIELDS, SkillRunnerDefinition, validate_inputs};

const RUNNER_FIELDS: &[&str] = &[
    "act",
    "default",
    "examples",
    "inputs",
    "policy",
    "result_from",
    "runx",
    "steps",
];

/// The graph keys a runner carries for its steps.
const GRAPH_FIELDS: &[&str] = &["policy", "result_from", "steps"];

const RUNX_FIELDS: &[&str] = &["post_run"];

pub(crate) fn validate_runner_definition(
    skill: &str,
    name: &str,
    runner: JsonObject,
) -> Result<SkillRunnerDefinition, ValidationError> {
    FIELDS.reject_unknown_fields(&runner, &format!("runners.{name}"), RUNNER_FIELDS)?;
    let runx = FIELDS.optional_object(runner.get("runx"), &format!("runners.{name}.runx"))?;
    if let Some(runx) = &runx {
        FIELDS.reject_unknown_fields(runx, &format!("runners.{name}.runx"), RUNX_FIELDS)?;
    }
    crate::runner::resolve_post_run_reflect_policy(runx.as_ref(), &format!("runners.{name}.runx"))?;
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
        default: FIELDS
            .optional_bool(runner.get("default"), &format!("runners.{name}.default"))?
            .unwrap_or(false),
        inputs,
        examples,
        act: validate_act_declaration(runner.get("act"))?,
        runx,
        graph: runner_graph(skill, name, &runner)?,
        raw: runner,
    })
}

/// The runner's steps as the graph the engine runs, named for its skill so
/// receipts carry the skill's identity. `result_from` defaults to the last
/// step.
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
    document.insert("name".to_owned(), JsonValue::String(skill.to_owned()));
    if !document.contains_key("result_from")
        && let Some(last) = runner
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

/// Validate a declared `act:` block at load: deserialize it into the typed
/// `ActDeclaration` and fail closed if it is present but malformed, so a skill
/// author sees the error instead of silently sealing a generic observation act.
pub(crate) fn validate_act_declaration(
    value: Option<&JsonValue>,
) -> Result<Option<ActDeclaration>, ValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let act = serde_json::to_value(value)
        .and_then(serde_json::from_value::<ActDeclaration>)
        .map_err(|error| FIELDS.validation_error(format!("act is malformed: {error}")))?;
    validate_act_reason_coherence(&act)?;
    Ok(Some(act))
}

/// Reject incoherent reason declarations: the operator-authored path
/// (`reason_from_input`) and the step-authored path (`reason_step`/`reason_from`)
/// are mutually exclusive; the sealed reason has exactly one author.
fn validate_act_reason_coherence(act: &ActDeclaration) -> Result<(), ValidationError> {
    if act.reason_from_input.is_some() && (act.reason_step.is_some() || act.reason_from.is_some()) {
        return Err(FIELDS.validation_error(
            "act.reason_from_input (operator-authored) is mutually exclusive with the step-authored reason_step / reason_from keys.",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use runx_contracts::JsonValue;

    use super::validate_act_declaration;
    use serde_json::json;

    fn act_value(value: serde_json::Value) -> JsonValue {
        serde_json::from_value(value).expect("convertible act value")
    }

    fn act_err(value: serde_json::Value) -> String {
        validate_act_declaration(Some(&act_value(value)))
            .err()
            .map(|error| error.to_string())
            .expect("act unexpectedly validated")
    }

    #[test]
    fn operator_reason_input_validates() {
        let act = validate_act_declaration(Some(&act_value(json!({
            "reason_from_input": "line",
        }))))
        .expect("valid act")
        .expect("present act");
        assert_eq!(act.reason_from_input.as_deref(), Some("line"));
    }

    #[test]
    fn operator_reason_input_conflicts_with_reason_step() {
        let message = act_err(json!({
            "reason_from_input": "line",
            "reason_step": "voice",
        }));
        assert!(message.contains("mutually exclusive"));
    }

    #[test]
    fn operator_reason_input_conflicts_with_reason_from() {
        let message = act_err(json!({
            "reason_from_input": "line",
            "reason_from": "line",
        }));
        assert!(message.contains("mutually exclusive"));
    }
}
