use runx_contracts::{JsonObject, JsonValue};

use crate::ValidationError;
use crate::graph::{RawWorkflowIr, validate_workflow_document};

use super::{FIELDS, SkillMethodDefinition, validate_input_fields};

const METHOD_FIELDS: &[&str] = &[
    "description",
    "inputs",
    "examples",
    "steps",
    "result_from",
    "offer",
];

/// The workflow keys a method carries for its steps.
const WORKFLOW_FIELDS: &[&str] = &["result_from", "steps"];

/// Validate one method. `inputs` are its inputs as written, each shared input
/// it names already taken from `shared_inputs`.
pub(crate) fn validate_method_definition(
    skill: &str,
    name: &str,
    method: JsonObject,
    inputs: &JsonObject,
) -> Result<SkillMethodDefinition, ValidationError> {
    FIELDS.reject_unknown_fields(&method, &format!("methods.{name}"), METHOD_FIELDS)?;
    let inputs = validate_input_fields(inputs, &format!("methods.{name}.inputs"))?;
    let examples = parse_method_examples(name, method.get("examples"))?;
    validate_input_examples(&format!("methods.{name}.examples"), &examples, &inputs)?;
    Ok(SkillMethodDefinition {
        name: name.to_owned(),
        description: FIELDS
            .optional_non_empty_string(
                method.get("description"),
                &format!("methods.{name}.description"),
            )?
            .map(|description| description.trim().to_owned()),
        offer: method
            .get("offer")
            .map(|offer| validate_offer(name, offer, &inputs))
            .transpose()?,
        inputs,
        examples,
        graph: method_workflow(skill, name, &method)?,
        raw: method,
    })
}

/// The method's steps as the workflow the engine runs, named for its skill so
/// receipts carry the skill's identity. A method is always its steps, and
/// `result_from` defaults to the last step.
fn method_workflow(
    skill: &str,
    name: &str,
    method: &JsonObject,
) -> Result<crate::Workflow, ValidationError> {
    if !method.contains_key("steps") {
        return Err(FIELDS.validation_error(format!(
            "methods.{name} has no steps; a method is always its steps, and one agent turn is one agent step."
        )));
    }
    let mut document = method
        .iter()
        .filter(|(key, _)| WORKFLOW_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<JsonObject>();
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
    validate_workflow_document(document.clone(), Some(RawWorkflowIr { document }))
        .map_err(|error| FIELDS.validation_error(format!("methods.{name}: {error}")))
}

/// What a paid method charges. A price reads only inputs the method requires:
/// each `by_input` input an `enum`, with a case for every combination of
/// their values, and the `by_size` input a value holding the stored
/// artifact's reference. Whose lease a buyer must hold is checked against
/// the package's connectors once they're known.
fn validate_offer(
    method: &str,
    value: &JsonValue,
    inputs: &std::collections::BTreeMap<String, super::SkillInput>,
) -> Result<runx_contracts::PaidSkillOfferTerms, ValidationError> {
    let field = format!("methods.{method}.offer");
    let terms = serde_json::to_value(value)
        .and_then(serde_json::from_value::<runx_contracts::PaidSkillOfferTerms>)
        .map_err(|error| FIELDS.validation_error(format!("{field} is invalid: {error}")))?;
    for name in terms.price.priced_inputs() {
        if !inputs.get(name).is_some_and(|input| input.required) {
            return Err(FIELDS.validation_error(format!(
                "{field}.price reads input {name}, which method {method} does not require."
            )));
        }
    }
    match &terms.price {
        runx_contracts::PaidSkillPrice::Fixed { .. } => {}
        runx_contracts::PaidSkillPrice::BySize { input, .. } => {
            if !matches!(
                inputs[input.as_str()].input_type.as_str(),
                "json" | "object"
            ) {
                return Err(FIELDS.validation_error(format!(
                    "{field}.price prices input {input} by the bytes of the stored artifact it references, so method {method} declares it an object or leaves its type open."
                )));
            }
        }
        runx_contracts::PaidSkillPrice::ByInput { cases } => {
            let mut combinations = 1_usize;
            for name in terms.price.priced_inputs() {
                let choices = enum_choices(&inputs[name]).ok_or_else(|| {
                    FIELDS.validation_error(format!(
                        "{field}.price reads input {name}, so method {method} declares its enum."
                    ))
                })?;
                combinations = combinations.saturating_mul(choices.len());
                if let Some(case) = cases
                    .iter()
                    .find(|case| !choices.contains(&case.inputs[name].as_value()))
                {
                    return Err(FIELDS.validation_error(format!(
                        "{field}.price has a case for {name} = {}, which is not one of its enum values.",
                        runx_contracts::canonical_json::canonical_stable_json(
                            case.inputs[name].as_value()
                        )
                        .unwrap_or_default()
                    )));
                }
            }
            // Cases are unique and each value is a declared one, so a case per
            // combination is an exact cover.
            if cases.len() != combinations {
                return Err(FIELDS.validation_error(format!(
                    "{field}.price prices {} of the {combinations} combinations of its inputs' values; it prices each once.",
                    cases.len()
                )));
            }
        }
    }
    if let Some(lease) = &terms.requires_lease {
        for pointer in lease.target.argument_pointers() {
            let input = pointer
                .strip_prefix('/')
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default();
            if !inputs.get(input).is_some_and(|input| input.required) {
                return Err(FIELDS.validation_error(format!(
                    "{field}.requires_lease reads input {input}, which method {method} does not require."
                )));
            }
        }
    }
    Ok(terms)
}

/// The distinct values an input's schema enumerates, if it enumerates any.
fn enum_choices(input: &super::SkillInput) -> Option<Vec<&JsonValue>> {
    let JsonValue::Array(values) = input.schema.as_ref()?.get("enum")? else {
        return None;
    };
    let mut choices = Vec::<&JsonValue>::new();
    for value in values {
        if !choices.contains(&value) {
            choices.push(value);
        }
    }
    (!choices.is_empty()).then_some(choices)
}

fn parse_method_examples(
    name: &str,
    value: Option<&JsonValue>,
) -> Result<Vec<JsonObject>, ValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let field = format!("methods.{name}.examples");
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
                "{field}[{index}]{} does not match the method's input contract: {error}",
                error.instance_path()
            )));
        }
    }
    Ok(())
}
