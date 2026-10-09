// Module rationale: graph step validation is kept together so
// field-level diagnostics stay consistent across the five step kinds.
use std::collections::BTreeSet;

use runx_contracts::{JsonObject, JsonValue, TurnAllowance};

use super::binding::{parse_operand, parse_values, parse_whole};
use super::helpers::{
    gate_id, optional_non_empty_string, optional_object, optional_string, optional_string_array,
    required_object, required_string, step_id, validation_error,
};
use super::tool::validate_tool_target;
use super::types::{
    AgentTask, AskGate, DuplicatePolicy, GraphStep, GraphWhen, OperationStepTarget, SkillTarget,
    StepKind, UncertainOutcome,
};
use crate::{ValidationError, skill::validate_skill_artifact_contract};
use runx_contracts::connector::{OperationName, is_connector_id};

/// The five step kinds; a step declares exactly one.
const KIND_FIELDS: &[&str] = &["agent", "tool", "operation", "ask", "skill"];

/// Keys every step may carry.
const COMMON_FIELDS: &[&str] = &["id", "label", "when"];

/// The operation call's fields, written on the step itself.
const OPERATION_CALL_FIELDS: &[&str] = &[
    "operation_name",
    "input",
    "approval",
    "amount",
    "expected_result",
    "result_fields",
    "optional_result_fields",
    "ephemeral_result_paths",
    "readback_of",
    "idempotency_key",
];

/// The approval packet an ask step produces.
const APPROVAL_DECISION: &str = "approval_decision";
const APPROVAL_DECISION_PACKET: &str = "runx.approval.decision.v1";

fn kind_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "agent" => &["inputs", "outputs", "artifacts", "context_skills"],
        "tool" => &["inputs", "outputs", "artifacts", "scopes"],
        "skill" => &["runner", "inputs", "artifacts", "scopes"],
        "ask" => &["inputs"],
        "operation" => &[
            "operation_name",
            "input",
            "approval",
            "amount",
            "expected_result",
            "result_fields",
            "optional_result_fields",
            "ephemeral_result_paths",
            "readback_of",
            "idempotency_key",
            "on_uncertain",
            "duplicates",
        ],
        _ => &[],
    }
}

// Function rationale: step validation parses one step's fields in a
// single pass and rejects incoherent combinations inline; splitting it would scatter
// the per-field rules away from the step they validate.
pub fn validate_step(
    raw_step: &JsonObject,
    field: &str,
    previous_step_ids: &BTreeSet<String>,
) -> Result<GraphStep, ValidationError> {
    let kind_name = step_kind_name(raw_step, field)?;
    reject_fields_outside_kind(raw_step, field, kind_name)?;

    let id = validate_step_id(raw_step, field, previous_step_ids)?;
    let kind = validate_kind(raw_step, field, kind_name)?;

    // A step receives exactly what it binds: its literals, and the run's
    // input and earlier steps' results it references.
    let (inputs, bindings) = match &kind {
        StepKind::Operation(target) => {
            let parsed = parse_values(&operation_call(raw_step), previous_step_ids, field)?;
            validate_operation_binding(field, target, &parsed.0)?;
            parsed
        }
        _ => match raw_step.get("inputs") {
            Some(value) => parse_whole(value, previous_step_ids, &format!("{field}.inputs"))?,
            None => (JsonObject::new(), Vec::new()),
        },
    };

    let scopes = optional_string_array(raw_step.get("scopes"), &format!("{field}.scopes"))?
        .unwrap_or_default();
    if scopes.iter().any(|scope| scope.trim().is_empty()) {
        return Err(validation_error(format!(
            "{field}.scopes must contain only non-empty scope strings."
        )));
    }
    let outputs = optional_object(raw_step.get("outputs"), &format!("{field}.outputs"))?;
    if let Some(outputs) = &outputs {
        runx_contracts::parse_output_contract(outputs)
            .map_err(|error| validation_error(format!("{field}.outputs is invalid: {error}")))?;
    }
    let artifacts = match &kind {
        StepKind::Ask(_) => Some(approval_decision_contract(field)?),
        _ => validate_skill_artifact_contract(
            raw_step.get("artifacts"),
            &format!("{field}.artifacts"),
        )?,
    };
    let (on_uncertain, duplicates) = validate_tolerance(raw_step, field)?;

    Ok(GraphStep {
        id,
        label: optional_non_empty_string(raw_step.get("label"), &format!("{field}.label"))?,
        kind,
        artifacts,
        outputs,
        inputs,
        bindings,
        context_skills: optional_string_array(
            raw_step.get("context_skills"),
            &format!("{field}.context_skills"),
        )?
        .unwrap_or_default(),
        scopes,
        when: validate_when(
            raw_step.get("when"),
            previous_step_ids,
            &format!("{field}.when"),
        )?,
        on_uncertain,
        duplicates,
    })
}

fn step_kind_name(raw_step: &JsonObject, field: &str) -> Result<&'static str, ValidationError> {
    let declared = KIND_FIELDS
        .iter()
        .copied()
        .filter(|kind| raw_step.contains_key(*kind))
        .collect::<Vec<_>>();
    match declared.as_slice() {
        [kind] => Ok(kind),
        _ => Err(validation_error(format!(
            "{field} must declare exactly one of agent, tool, operation, ask, or skill."
        ))),
    }
}

fn reject_fields_outside_kind(
    raw_step: &JsonObject,
    field: &str,
    kind: &str,
) -> Result<(), ValidationError> {
    let allowed = kind_fields(kind);
    match raw_step.keys().find(|key| {
        let key = key.as_str();
        key != kind && !COMMON_FIELDS.contains(&key) && !allowed.contains(&key)
    }) {
        Some(key) => Err(validation_error(format!(
            "{field}.{key} does not apply to this {kind} step."
        ))),
        None => Ok(()),
    }
}

fn validate_kind(
    raw_step: &JsonObject,
    field: &str,
    kind: &str,
) -> Result<StepKind, ValidationError> {
    let value = raw_step.get(kind);
    let qualified = format!("{field}.{kind}");
    match kind {
        "agent" => {
            let agent = required_object(value, &qualified)?;
            if let Some(key) = agent
                .keys()
                .find(|key| !matches!(key.as_str(), "role" | "task"))
            {
                return Err(validation_error(format!(
                    "{qualified}.{key} is not a field of an agent act."
                )));
            }
            Ok(StepKind::Agent(AgentTask {
                role: non_empty(agent.get("role"), &format!("{qualified}.role"))?,
                task: non_empty(agent.get("task"), &format!("{qualified}.task"))?,
                allowance: TurnAllowance::default(),
                connectors: Default::default(),
            }))
        }
        "tool" => validate_tool_target(
            value.ok_or_else(|| validation_error(format!("{qualified} is required.")))?,
            &qualified,
        )
        .map(StepKind::Tool),
        "operation" => parse_operation_target(value, field).map(StepKind::Operation),
        "ask" => {
            let ask = required_object(value, &qualified)?;
            if let Some(key) = ask
                .keys()
                .find(|key| !matches!(key.as_str(), "gate" | "reason" | "type"))
            {
                return Err(validation_error(format!(
                    "{qualified}.{key} is not a field of an ask."
                )));
            }
            // The reason is the text a person reads; the values they need
            // to decide are the ask's inputs, shown beside it.
            let reason = non_empty(ask.get("reason"), &format!("{qualified}.reason"))?;
            if reason.starts_with('$') && !reason.starts_with("$$") {
                return Err(validation_error(format!(
                    "{qualified}.reason is text, not a reference; bind the values a person needs in the ask's inputs."
                )));
            }
            Ok(StepKind::Ask(AskGate {
                gate: gate_id(
                    non_empty(ask.get("gate"), &format!("{qualified}.gate"))?,
                    &format!("{qualified}.gate"),
                )?,
                reason: reason
                    .strip_prefix('$')
                    .filter(|rest| rest.starts_with('$'))
                    .map_or(reason.clone(), str::to_owned),
                gate_type: optional_non_empty_string(
                    ask.get("type"),
                    &format!("{qualified}.type"),
                )?,
            }))
        }
        _ => Ok(StepKind::Skill(SkillTarget {
            reference: non_empty(value, &qualified)?,
            runner: optional_non_empty_string(raw_step.get("runner"), &format!("{field}.runner"))?,
        })),
    }
}

fn non_empty(value: Option<&JsonValue>, field: &str) -> Result<String, ValidationError> {
    optional_non_empty_string(value, field)?
        .ok_or_else(|| validation_error(format!("{field} is required.")))
}

/// The operation call: the step's own call fields.
fn operation_call(raw_step: &JsonObject) -> JsonObject {
    raw_step
        .iter()
        .filter(|(key, _)| OPERATION_CALL_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// `operation: <requirement>/<operation>`, or `operation: <requirement>` with
/// the operation chosen at run time by the step's `operation_name`.
fn parse_operation_target(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<OperationStepTarget, ValidationError> {
    let value = non_empty(value, &format!("{field}.operation"))?;
    let (requirement, operation) = match value.split_once('/') {
        Some((requirement, operation)) => (requirement, Some(operation)),
        None => (value.as_str(), None),
    };
    if !is_connector_id(requirement) {
        return Err(validation_error(format!(
            "{field}.operation requirement '{requirement}' must be a connectors key."
        )));
    }
    let operation = operation
        .map(|operation| {
            OperationName::new(operation).ok_or_else(|| {
                validation_error(format!(
                    "{field}.operation '{operation}' is not a valid operation name."
                ))
            })
        })
        .transpose()?;
    Ok(OperationStepTarget {
        requirement: requirement.to_owned(),
        operation,
        declared: None,
    })
}

fn validate_operation_binding(
    field: &str,
    operation: &OperationStepTarget,
    call: &JsonObject,
) -> Result<(), ValidationError> {
    match (&operation.operation, call.contains_key("operation_name")) {
        (Some(_), true) => Err(validation_error(format!(
            "{field} names its operation and also an operation_name."
        ))),
        (None, false) => Err(validation_error(format!(
            "{field}.operation names only a requirement; choose the operation with operation_name."
        ))),
        _ => Ok(()),
    }
}

fn approval_decision_contract(
    field: &str,
) -> Result<crate::skill::SkillArtifactContract, ValidationError> {
    let contract = JsonValue::Object(JsonObject::from([
        (
            "wrap_as".to_owned(),
            JsonValue::String(APPROVAL_DECISION.to_owned()),
        ),
        (
            "packet".to_owned(),
            JsonValue::String(APPROVAL_DECISION_PACKET.to_owned()),
        ),
    ]));
    validate_skill_artifact_contract(Some(&contract), &format!("{field}.ask"))?
        .ok_or_else(|| validation_error(format!("{field}.ask has no decision packet.")))
}

fn validate_tolerance(
    raw_step: &JsonObject,
    field: &str,
) -> Result<(Option<UncertainOutcome>, Option<DuplicatePolicy>), ValidationError> {
    let on_uncertain = optional_string(raw_step.get("on_uncertain"), &format!("{field}.on_uncertain"))?
        .map(|value| UncertainOutcome::parse(&value).ok_or_else(|| {
            validation_error(format!(
                "{field}.on_uncertain '{value}' must be hold, reconcile, or expire_after <n>s|m|h|d."
            ))
        }))
        .transpose()?;
    let duplicates = optional_string(raw_step.get("duplicates"), &format!("{field}.duplicates"))?
        .map(|value| match value.as_str() {
            "forbidden" => Ok(DuplicatePolicy::Forbidden),
            "visible" => Ok(DuplicatePolicy::Visible),
            _ => Err(validation_error(format!(
                "{field}.duplicates '{value}' must be forbidden or visible."
            ))),
        })
        .transpose()?;
    Ok((on_uncertain, duplicates))
}

fn validate_when(
    value: Option<&JsonValue>,
    previous_step_ids: &BTreeSet<String>,
    field: &str,
) -> Result<Option<GraphWhen>, ValidationError> {
    let Some(record) = optional_object(value, field)? else {
        return Ok(None);
    };
    if let Some(key) = record
        .keys()
        .find(|key| !matches!(key.as_str(), "value" | "equals" | "not_equals"))
    {
        return Err(validation_error(format!(
            "{field}.{key} is not a field of a condition; it takes value and equals or not_equals."
        )));
    }
    let equals = record.get("equals").cloned();
    let not_equals = record.get("not_equals").cloned();
    if equals.is_some() && not_equals.is_some() {
        return Err(validation_error(format!(
            "{field} must not declare both equals and not_equals."
        )));
    }
    if equals.is_none() && not_equals.is_none() {
        return Err(validation_error(format!(
            "{field} must declare equals or not_equals."
        )));
    }
    let value = record
        .get("value")
        .ok_or_else(|| validation_error(format!("{field}.value is required.")))?;
    Ok(Some(GraphWhen {
        value: parse_operand(value, previous_step_ids, &format!("{field}.value"))?,
        equals,
        not_equals,
    }))
}

fn validate_step_id(
    raw_step: &JsonObject,
    field: &str,
    previous_step_ids: &BTreeSet<String>,
) -> Result<String, ValidationError> {
    let id = step_id(
        required_string(raw_step.get("id"), &format!("{field}.id"))?,
        &format!("{field}.id"),
    )?;
    if previous_step_ids.contains(&id) {
        return Err(validation_error(format!(
            "{field}.id '{id}' must be unique."
        )));
    }
    Ok(id)
}
