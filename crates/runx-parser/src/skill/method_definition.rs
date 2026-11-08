use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{JsonObject, JsonValue};

use crate::ValidationError;
use crate::graph::{RawWorkflowIr, validate_workflow_document};

use super::{FIELDS, SkillMethodDefinition, validate_input_fields};

const METHOD_FIELDS: &[&str] = &[
    "description",
    "inputs",
    "input_constraints",
    "examples",
    "steps",
    "result_from",
    "offer",
];

/// The JSON Schema keys a method's `input_constraints` may use: those that
/// relate fields to each other. `properties` and `required` stay on the
/// fields themselves.
const INPUT_CONSTRAINT_KEYS: &[&str] = &[
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "if",
    "then",
    "else",
    "dependentRequired",
    "dependentSchemas",
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
    let input_constraints =
        validate_input_constraints(name, method.get("input_constraints"), &inputs)?;
    let examples = parse_method_examples(name, method.get("examples"))?;
    validate_input_examples(
        &format!("methods.{name}.examples"),
        &examples,
        &inputs,
        &input_constraints,
    )?;
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
        input_constraints,
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

/// A method's cross-field constraints: a JSON Schema object of only
/// [`INPUT_CONSTRAINT_KEYS`], naming only inputs the method declares.
fn validate_input_constraints(
    method: &str,
    value: Option<&JsonValue>,
    inputs: &BTreeMap<String, super::SkillInput>,
) -> Result<JsonObject, ValidationError> {
    let Some(value) = value else {
        return Ok(JsonObject::new());
    };
    let field = format!("methods.{method}.input_constraints");
    let constraints = FIELDS.required_object(Some(value), &field)?.clone();
    if constraints.is_empty() {
        return Err(FIELDS.validation_error(format!("{field} lists nothing.")));
    }
    if let Some(key) = constraints
        .keys()
        .find(|key| !INPUT_CONSTRAINT_KEYS.contains(&key.as_str()))
    {
        return Err(FIELDS.validation_error(match key.as_str() {
            "properties" | "required" => format!(
                "{field}.{key} belongs on the inputs themselves; constraints relate fields to each other."
            ),
            _ => format!(
                "{field}.{key} is not a constraint across inputs; use {}.",
                INPUT_CONSTRAINT_KEYS.join(", ")
            ),
        }));
    }
    let mut named = BTreeSet::new();
    named_inputs(&JsonValue::Object(constraints.clone()), &mut named);
    if let Some(name) = named
        .iter()
        .find(|name| !inputs.contains_key(name.as_str()))
    {
        return Err(FIELDS.validation_error(format!(
            "{field} names input {name}, which method {method} does not declare."
        )));
    }
    let schema = serde_json::to_value(super::constrained_input_schema(inputs, &constraints))
        .map_err(|error| FIELDS.validation_error(format!("{field} is invalid: {error}")))?;
    jsonschema::draft202012::meta::validate(&schema).map_err(|error| {
        FIELDS.validation_error(format!("{field} is not valid JSON Schema: {error}"))
    })?;
    Ok(constraints)
}

/// Every input a constraint schema names: in `required`, `properties`,
/// `dependentRequired` and `dependentSchemas` at the input object's own
/// level, through its combinators. A field's own value schema is not
/// entered, since names there are its own.
fn named_inputs(schema: &JsonValue, named: &mut BTreeSet<String>) {
    let JsonValue::Object(schema) = schema else {
        return;
    };
    let strings = |value: &JsonValue| -> Vec<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(JsonValue::as_str)
            .map(str::to_owned)
            .collect()
    };
    if let Some(required) = schema.get("required") {
        named.extend(strings(required));
    }
    if let Some(JsonValue::Object(properties)) = schema.get("properties") {
        named.extend(properties.keys().cloned());
    }
    if let Some(JsonValue::Object(dependent)) = schema.get("dependentRequired") {
        for (name, requires) in dependent {
            named.insert(name.clone());
            named.extend(strings(requires));
        }
    }
    if let Some(JsonValue::Object(dependent)) = schema.get("dependentSchemas") {
        for (name, nested) in dependent {
            named.insert(name.clone());
            named_inputs(nested, named);
        }
    }
    for key in ["allOf", "anyOf", "oneOf"] {
        for nested in schema
            .get(key)
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
        {
            named_inputs(nested, named);
        }
    }
    for key in ["not", "if", "then", "else"] {
        if let Some(nested) = schema.get(key) {
            named_inputs(nested, named);
        }
    }
}

/// What a paid method charges. A price reads only inputs the method requires:
/// each `by_input` input an `enum`, with a case for every combination of
/// their values, and the `by_size` input a value holding the stored
/// artifact's reference. Whose lease a buyer must hold is checked against
/// the package's connectors once they're known.
fn validate_offer(
    method: &str,
    value: &JsonValue,
    inputs: &BTreeMap<String, super::SkillInput>,
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
    inputs: &BTreeMap<String, super::SkillInput>,
    constraints: &JsonObject,
) -> Result<(), ValidationError> {
    let schema = serde_json::to_value(super::constrained_input_schema(inputs, constraints))
        .map_err(|error| {
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
