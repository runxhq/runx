use runx_contracts::{JsonObject, JsonValue};

use crate::ValidationError;

use super::{
    FIELDS, SkillGovernance, SkillRunnerDefinition, field_value, first_value,
    flattened_source_record, validate_allowed_tools, validate_artifact_contract,
    validate_execution_semantics, validate_idempotency, validate_inputs, validate_retry,
    validate_source, validate_source_fields,
};

const RUNNER_FIELDS: &[&str] = &[
    "act",
    "agent",
    "allowed_tools",
    "args",
    "artifacts",
    "auth",
    "command",
    "context",
    "context_skills",
    "cwd",
    "default",
    "environment",
    "examples",
    "execution",
    "export",
    "graph",
    "headers",
    "hook",
    "http",
    "idempotency",
    "input_mode",
    "inputs",
    "method",
    "module",
    "outputs",
    "pages",
    "policy",
    "retry",
    "risk",
    "runx",
    "runtime",
    "scopes",
    "source",
    "task",
    "timeout_seconds",
    "type",
    "url",
    "allow_private_network",
];

pub(crate) fn validate_runner_definition(
    name: &str,
    runner: JsonObject,
) -> Result<SkillRunnerDefinition, ValidationError> {
    if runner.contains_key("instructions") {
        return Err(FIELDS.validation_error(format!(
            "runners.{name}.instructions is not supported; put agent operating instructions in the owning SKILL.md"
        )));
    }
    FIELDS.reject_unknown_fields(&runner, &format!("runners.{name}"), RUNNER_FIELDS)?;
    let runx = FIELDS.optional_object(runner.get("runx"), &format!("runners.{name}.runx"))?;
    if runner
        .get("risk")
        .and_then(JsonValue::as_object)
        .is_some_and(|risk| risk.contains_key("mutating"))
    {
        return Err(FIELDS.validation_error(format!(
            "runners.{name}.risk.mutating is not supported; approval and authority are owned by invoked capabilities"
        )));
    }
    for field in [
        "auth",
        "credential",
        "environment",
        "mutating",
        "runtime",
        "sandbox",
        "scopes",
    ] {
        if runx
            .as_ref()
            .is_some_and(|metadata| metadata.contains_key(field))
        {
            return Err(FIELDS.validation_error(format!(
                "runners.{name}.runx contains unknown field '{field}'; execution requirements belong on the runner or its source"
            )));
        }
    }
    crate::runner::resolve_post_run_reflect_policy(runx.as_ref(), &format!("runners.{name}.runx"))?;
    let source_record =
        match FIELDS.optional_object(runner.get("source"), &format!("runners.{name}.source"))? {
            Some(source) => {
                validate_source_fields(&source, &format!("runners.{name}.source"))?;
                source
            }
            None => flattened_source_record(&runner),
        };
    let governance = validate_runner_governance(name, &runner, runx.as_ref())?;
    let source = validate_source(&source_record)?;
    validate_runner_lane_constraints(name, &source, governance.artifacts.as_ref())?;
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
        source,
        inputs,
        examples,
        scopes: validate_scopes(
            FIELDS
                .optional_string_array(runner.get("scopes"), &format!("runners.{name}.scopes"))?
                .unwrap_or_default(),
            &format!("runners.{name}.scopes"),
        )?,
        auth: runner.get("auth").cloned(),
        risk: runner.get("risk").cloned(),
        runtime: runner.get("runtime").cloned(),
        retry: governance.retry,
        idempotency: governance.idempotency,
        artifacts: governance.artifacts,
        allowed_tools: governance.allowed_tools,
        execution: governance.execution,
        runx,
        raw: runner,
    })
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

fn validate_runner_lane_constraints(
    name: &str,
    source: &super::SkillSource,
    artifacts: Option<&super::SkillArtifactContract>,
) -> Result<(), ValidationError> {
    if source.source_type == super::SourceKind::Graph && artifacts.is_some() {
        return Err(FIELDS.validation_error(format!(
            "runners.{name}.artifacts is ambiguous for a graph source; declare the packet on the graph's terminal output-producing step"
        )));
    }
    if source.source_type == super::SourceKind::Graph && !source.environment.is_empty() {
        return Err(FIELDS.validation_error(format!(
            "runners.{name}.environment is ambiguous for a graph source; declare environment requirements on the executable run step"
        )));
    }
    if source.source_type == super::SourceKind::Graph && source.outputs.is_some() {
        return Err(FIELDS.validation_error(format!(
            "runners.{name}.outputs is ambiguous for a graph source; declare outputs on the graph's terminal output-producing step"
        )));
    }
    if source.source_type == super::SourceKind::Graph
        && source
            .graph
            .as_ref()
            .is_none_or(|graph| graph.result_from.is_empty())
    {
        return Err(FIELDS.validation_error(format!(
            "runners.{name}.graph.result_from must name at least one intentional public result producer"
        )));
    }
    Ok(())
}

fn validate_scopes(scopes: Vec<String>, field: &str) -> Result<Vec<String>, ValidationError> {
    if scopes.iter().any(|scope| scope.trim().is_empty()) {
        return Err(
            FIELDS.validation_error(format!("{field} must contain only non-empty scope strings"))
        );
    }
    Ok(scopes)
}

fn validate_runner_governance(
    name: &str,
    runner: &JsonObject,
    runx: Option<&JsonObject>,
) -> Result<SkillGovernance, ValidationError> {
    Ok(SkillGovernance {
        retry: validate_retry(
            first_value(runner.get("retry"), field_value(runx, "retry")),
            &format!("runners.{name}.retry"),
        )?,
        idempotency: validate_idempotency(
            first_value(runner.get("idempotency"), field_value(runx, "idempotency")),
            &format!("runners.{name}.idempotency"),
        )?,
        artifacts: validate_artifact_contract(
            first_value(runner.get("artifacts"), field_value(runx, "artifacts")),
            &format!("runners.{name}.artifacts"),
        )?,
        allowed_tools: validate_allowed_tools(
            field_value(runx, "allowed_tools"),
            &format!("runners.{name}.runx.allowed_tools"),
        )?,
        execution: validate_execution_semantics(
            first_value(runner.get("execution"), field_value(runx, "execution")),
            &format!("runners.{name}.execution"),
        )?,
    })
}
