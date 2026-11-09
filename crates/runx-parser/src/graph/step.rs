// Module rationale: step validation is kept together so field-level
// diagnostics stay consistent across the five step kinds.
use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{
    AllowedOperation, CatalogRule, JsonObject, JsonValue, PacketId, SkillAllowance, TurnAllowance,
};

use super::binding::{
    Operand, Source, parse_act, parse_call, parse_literal, parse_operand, parse_whole,
};
use super::helpers::{
    gate_id, narrowing, optional_non_empty_string, optional_object, required_array,
    required_object, required_string, step_id, validation_error,
};
use super::tool::validate_tool_target;
use super::types::{
    AgentTask, AskGate, OperationStepTarget, SkillTarget, StepAct, StepKind, WorkflowStep,
    WorkflowWhen,
};
use crate::ValidationError;
use crate::skill::SkillArtifactContract;
use runx_contracts::connector::{OperationName, is_connector_name, is_operation_name};

/// The five step kinds; a step declares exactly one.
const KIND_FIELDS: &[&str] = &["agent", "tool", "operation", "ask", "skill"];

/// Keys every step may carry.
const COMMON_FIELDS: &[&str] = &["id", "when", "guard", "effects", "reach"];

/// What an agent step's `calls` may name.
const CALLS_FIELDS: &[&str] = &["tools", "operations", "skills", "load", "ask"];

/// What an agent step's `context` gives it.
const CONTEXT_FIELDS: &[&str] = &["skills", "kinds", "entries"];

/// An operation step's `result`.
const RESULT_FIELDS: &[&str] = &["required", "optional", "equals", "ephemeral"];

/// An operation chosen per run.
const CHOSEN_OPERATION_FIELDS: &[&str] = &["connector", "choose", "from"];

/// The fields of a step's `act`. A decision is never a step's: a skill's
/// decision belongs in its own typed result.
const STEP_ACT_FIELDS: &[&str] = &["form", "purpose", "reason", "target", "previous"];

/// The approval packet an ask step produces.
const APPROVAL_DECISION: &str = "approval_decision";
const APPROVAL_DECISION_PACKET: &str = "runx.approval.decision.v1";

fn kind_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "agent" => &[
            "inputs",
            "outputs",
            "result_as",
            "calls",
            "context",
            "requires",
        ],
        "tool" => &["inputs", "outputs", "result_as"],
        "skill" => &["inputs", "outputs", "result_as", "act"],
        "ask" => &["inputs"],
        "operation" => &[
            "inputs",
            "approval",
            "idempotency_key",
            "retry_unknown_outcomes",
            "confirms",
            "act",
            "result",
        ],
        _ => &[],
    }
}

/// Validate one step. `previous_steps` names each earlier step and whether
/// it is an operation step, the only kind a read can confirm.
// Function rationale: step validation parses one step's fields in a single
// pass and rejects incoherent combinations inline; splitting it would scatter
// the per-field rules away from the step they validate.
pub fn validate_step(
    raw_step: &JsonObject,
    field: &str,
    previous_steps: &BTreeMap<String, bool>,
) -> Result<WorkflowStep, ValidationError> {
    let previous_step_ids = previous_steps.keys().cloned().collect::<BTreeSet<_>>();
    let kind_name = step_kind_name(raw_step, field)?;
    reject_fields_outside_kind(raw_step, field, kind_name)?;

    let id = validate_step_id(raw_step, field, &previous_step_ids)?;
    let kind = validate_kind(raw_step, field, kind_name, &previous_step_ids)?;

    // A step receives exactly what it binds: its literals, and the run's
    // input and earlier steps' results it references.
    let (inputs, bindings) = match &kind {
        StepKind::Operation(_) => {
            let call = operation_call(raw_step, field, previous_steps)?;
            parse_call(&call, &previous_step_ids, field)?
        }
        _ => match raw_step.get("inputs") {
            Some(value) => parse_whole(value, &previous_step_ids, &format!("{field}.inputs"))?,
            None => (JsonObject::new(), Vec::new()),
        },
    };

    // An operation step's act is its call's; a skill step's is its child
    // run's `$run.act`. Only an act reads `$run.act`.
    let act = raw_step
        .get("act")
        .map(|value| {
            if let JsonValue::Object(act) = value {
                if let Some(key) = act
                    .keys()
                    .find(|key| !STEP_ACT_FIELDS.contains(&key.as_str()))
                {
                    return Err(validation_error(format!(
                        "{field}.act.{key} is not a field of an act; it takes {}.",
                        STEP_ACT_FIELDS.join(", ")
                    )));
                }
                if !act.contains_key("purpose") {
                    return Err(validation_error(format!(
                        "{field}.act states why the call is made: it needs a purpose."
                    )));
                }
            }
            parse_act(value, &previous_step_ids, &format!("{field}.act"))
        })
        .transpose()?
        .map(|(values, bindings)| StepAct { values, bindings });

    let outputs = optional_object(raw_step.get("outputs"), &format!("{field}.outputs"))?;
    if let Some(outputs) = &outputs {
        runx_contracts::parse_output_contract(outputs)
            .map_err(|error| validation_error(format!("{field}.outputs is invalid: {error}")))?;
        if matches!(kind, StepKind::Agent(_)) && outputs.contains_key("closure") {
            return Err(validation_error(format!(
                "{field}.outputs cannot declare closure: an agent's answer states how its act closed under that name."
            )));
        }
    }
    let artifacts = match &kind {
        StepKind::Ask(_) => Some(SkillArtifactContract {
            emits: None,
            named_emits: None,
            packets: None,
            wrap_as: Some(APPROVAL_DECISION.to_owned()),
            packet: PacketId::new(APPROVAL_DECISION_PACKET.to_owned()),
        }),
        _ => output_artifacts(
            outputs.as_ref(),
            optional_non_empty_string(raw_step.get("result_as"), &format!("{field}.result_as"))?,
            field,
        )?,
    };
    let context_skills = match raw_step.get("context") {
        None => Vec::new(),
        Some(value) => {
            let context = required_object(Some(value), &format!("{field}.context"))?;
            reject_unknown(context, &format!("{field}.context"), CONTEXT_FIELDS)?;
            string_list(context.get("skills"), &format!("{field}.context.skills"))?
        }
    };

    let (effects, reach) = narrowing(raw_step, field)?;

    Ok(WorkflowStep {
        id,
        kind,
        artifacts,
        outputs,
        inputs,
        bindings,
        act,
        context_skills,
        when: optional_object(raw_step.get("when"), &format!("{field}.when"))?
            .map(|record| condition(&record, &previous_step_ids, &format!("{field}.when")))
            .transpose()?,
        guard: validate_guard(
            raw_step.get("guard"),
            &previous_step_ids,
            &format!("{field}.guard"),
        )?,
        retry_unknown_outcomes: match raw_step.get("retry_unknown_outcomes") {
            None => false,
            Some(JsonValue::Bool(retry)) => *retry,
            Some(_) => {
                return Err(validation_error(format!(
                    "{field}.retry_unknown_outcomes must be true or false."
                )));
            }
        },
        effects,
        reach,
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

fn reject_unknown(
    record: &JsonObject,
    field: &str,
    allowed: &[&str],
) -> Result<(), ValidationError> {
    match record.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(validation_error(format!(
            "{field}.{key} is not a field here; it takes {}.",
            allowed.join(", ")
        ))),
        None => Ok(()),
    }
}

fn validate_kind(
    raw_step: &JsonObject,
    field: &str,
    kind: &str,
    previous_step_ids: &BTreeSet<String>,
) -> Result<StepKind, ValidationError> {
    let value = raw_step.get(kind);
    let qualified = format!("{field}.{kind}");
    match kind {
        "agent" => Ok(StepKind::Agent(AgentTask {
            task: non_empty(value, &qualified)?,
            allowance: match raw_step.get("calls") {
                Some(calls) => turn_allowance(calls, &format!("{field}.calls"))?,
                None => TurnAllowance::default(),
            },
            connectors: BTreeMap::new(),
            requires: turn_requirements(raw_step.get("requires"), &format!("{field}.requires"))?,
            context_kinds: context_kinds(
                raw_step.get("context"),
                &format!("{field}.context.kinds"),
            )?,
            context_entries: context_entries(
                raw_step.get("context"),
                previous_step_ids,
                &format!("{field}.context.entries"),
            )?,
        })),
        "tool" => validate_tool_target(
            value.ok_or_else(|| validation_error(format!("{qualified} is required.")))?,
            &qualified,
        )
        .map(StepKind::Tool),
        "operation" => {
            parse_operation_target(value, field, previous_step_ids).map(StepKind::Operation)
        }
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
        _ => skill_target(&non_empty(value, &qualified)?, &qualified).map(StepKind::Skill),
    }
}

/// `skill: <reference>`, or `<reference>#<method>`: a registry name, a
/// relative path, or `.` for this package.
fn skill_target(reference: &str, field: &str) -> Result<SkillTarget, ValidationError> {
    match reference.split_once('#') {
        None => Ok(SkillTarget {
            skill: reference.to_owned(),
            method: None,
        }),
        Some((skill, method)) if !skill.is_empty() && !method.is_empty() => Ok(SkillTarget {
            skill: skill.to_owned(),
            method: Some(method.to_owned()),
        }),
        Some(_) => Err(validation_error(format!(
            "{field} {reference:?} names a skill and its method as <skill>#<method>."
        ))),
    }
}

fn non_empty(value: Option<&JsonValue>, field: &str) -> Result<String, ValidationError> {
    optional_non_empty_string(value, field)?
        .ok_or_else(|| validation_error(format!("{field} is required.")))
}

/// A list of distinct non-empty strings, or none.
/// The kinds of context an agent step's turn asks its host to supply (N22,
/// D31), each named once: lowercase words joined by `_` or `-`.
fn context_kinds(context: Option<&JsonValue>, field: &str) -> Result<Vec<String>, ValidationError> {
    let kinds = string_list(
        context
            .and_then(JsonValue::as_object)
            .and_then(|context| context.get("kinds")),
        field,
    )?;
    for (index, kind) in kinds.iter().enumerate() {
        let named = kind.starts_with(|first: char| first.is_ascii_lowercase())
            && kind
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-');
        if !named {
            return Err(validation_error(format!(
                "{field}[{index}] is {kind}: a kind is lowercase words joined by _ or -."
            )));
        }
    }
    Ok(kinds)
}

/// The context entries an agent step's turn is given from earlier steps
/// (memory.md): each an earlier step's output of `ContextEntry` values,
/// `$steps.<id>.<field>`, named once. The entries keep their own labels.
fn context_entries(
    context: Option<&JsonValue>,
    previous_step_ids: &BTreeSet<String>,
    field: &str,
) -> Result<Vec<Source>, ValidationError> {
    let Some(entries) = context
        .and_then(JsonValue::as_object)
        .and_then(|context| context.get("entries"))
    else {
        return Ok(Vec::new());
    };
    let JsonValue::Array(entries) = entries else {
        return Err(validation_error(format!(
            "{field} is a list of $steps.<id>.<field> references."
        )));
    };
    let mut sources = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let entry_field = format!("{field}[{index}]");
        let source = match parse_operand(entry, previous_step_ids, &entry_field)? {
            Operand::Reference(source @ Source::Step { .. }) => source,
            Operand::Reference(_) | Operand::Literal(_) => {
                return Err(validation_error(format!(
                    "{entry_field} reads an earlier step's output of context entries: write $steps.<id>.<field>."
                )));
            }
        };
        if sources.contains(&source) {
            return Err(validation_error(format!(
                "{entry_field} names {} again.",
                source.reference()
            )));
        }
        sources.push(source);
    }
    Ok(sources)
}

/// What an agent step's turn needs from its agent harness. Each is named once, and
/// a name that isn't a requirement is refused by name.
fn turn_requirements(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Vec<runx_contracts::TurnRequirement>, ValidationError> {
    string_list(value, field)?
        .iter()
        .enumerate()
        .map(|(index, name)| {
            runx_contracts::TurnRequirement::named(name).ok_or_else(|| {
                validation_error(format!(
                    "{field}[{index}] is {name}, which no agent harness is asked for: a turn requires {}.",
                    runx_contracts::TurnRequirement::ALL
                        .map(runx_contracts::TurnRequirement::as_str)
                        .join(", ")
                ))
            })
        })
        .collect()
}

fn string_list(value: Option<&JsonValue>, field: &str) -> Result<Vec<String>, ValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for (index, entry) in required_array(Some(value), field)?.iter().enumerate() {
        let entry = non_empty(Some(entry), &format!("{field}[{index}]"))?;
        if entries.contains(&entry) {
            return Err(validation_error(format!(
                "{field}[{index}] repeats {entry}."
            )));
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// What an agent turn may call. Plural keys list names; `load` lists the
/// catalog rules a turn may load packages under, and `ask: true` lets it ask
/// a person. Writing them grants nothing.
fn turn_allowance(value: &JsonValue, field: &str) -> Result<TurnAllowance, ValidationError> {
    let calls = required_object(Some(value), field)?;
    reject_unknown(calls, field, CALLS_FIELDS)?;
    let mut operations = Vec::new();
    for (index, reference) in string_list(calls.get("operations"), &format!("{field}.operations"))?
        .into_iter()
        .enumerate()
    {
        let (requirement, operation) = reference
            .rsplit_once('/')
            .filter(|(requirement, operation)| {
                is_connector_name(requirement) && is_operation_name(operation)
            })
            .ok_or_else(|| {
                validation_error(format!(
                    "{field}.operations[{index}] {reference:?} names <connector>/<operation>."
                ))
            })?;
        operations.push(AllowedOperation {
            requirement: requirement.into(),
            operation: operation.into(),
        });
    }
    let mut tools = Vec::new();
    for (index, tool) in string_list(calls.get("tools"), &format!("{field}.tools"))?
        .into_iter()
        .enumerate()
    {
        let admission = runx_core::policy::admit_agent_tool(&tool);
        if !admission.allowed {
            return Err(validation_error(format!(
                "{field}.tools[{index}] {tool:?}: {}.",
                admission.reason
            )));
        }
        tools.push(tool.as_str().into());
    }
    let mut skills = string_list(calls.get("skills"), &format!("{field}.skills"))?
        .into_iter()
        .map(|reference| SkillAllowance::Skill(reference.as_str().into()))
        .collect::<Vec<_>>();
    if let Some(load) = calls.get("load") {
        for (index, rule) in required_array(Some(load), &format!("{field}.load"))?
            .iter()
            .enumerate()
        {
            let entry = format!("{field}.load[{index}]");
            let rule = rule
                .clone()
                .deserialize_into::<CatalogRule>()
                .map_err(|error| {
                    validation_error(format!("{entry} is not a catalog rule: {error}."))
                })?;
            rule.check()
                .map_err(|reason| validation_error(format!("{entry}: {reason}.")))?;
            let rule = SkillAllowance::Rule(rule);
            if skills.contains(&rule) {
                return Err(validation_error(format!(
                    "{entry} repeats an earlier rule."
                )));
            }
            skills.push(rule);
        }
    }
    let asks = match calls.get("ask") {
        None => false,
        Some(JsonValue::Bool(true)) => true,
        Some(_) => {
            return Err(validation_error(format!(
                "{field}.ask is true, or left out."
            )));
        }
    };
    Ok(TurnAllowance {
        operations,
        tools,
        skills,
        asks,
    })
}

/// `operation: <connector>/<operation>`, or an operation chosen per run:
/// `operation: { connector, choose: <binding>, from: [<operation>, ...] }`.
/// An operation name always has a dot and a connector name never does, so
/// the last slash splits off the operation: `google/calendar/events.list`.
fn parse_operation_target(
    value: Option<&JsonValue>,
    field: &str,
    previous_step_ids: &BTreeSet<String>,
) -> Result<OperationStepTarget, ValidationError> {
    let qualified = format!("{field}.operation");
    if let Some(JsonValue::Object(chosen)) = value {
        reject_unknown(chosen, &qualified, CHOSEN_OPERATION_FIELDS)?;
        let requirement = non_empty(chosen.get("connector"), &format!("{qualified}.connector"))?;
        if !is_connector_name(&requirement) {
            return Err(validation_error(format!(
                "{qualified}.connector '{requirement}' is not a connector name."
            )));
        }
        let choose = non_empty(chosen.get("choose"), &format!("{qualified}.choose"))?;
        parse_operand(
            &JsonValue::String(choose),
            previous_step_ids,
            &format!("{qualified}.choose"),
        )?;
        let mut from = Vec::new();
        for (index, operation) in string_list(chosen.get("from"), &format!("{qualified}.from"))?
            .iter()
            .enumerate()
        {
            from.push(OperationName::new(operation).ok_or_else(|| {
                validation_error(format!(
                    "{qualified}.from[{index}] '{operation}' is not a valid operation name."
                ))
            })?);
        }
        if from.is_empty() {
            return Err(validation_error(format!(
                "{qualified}.from lists the operations the run may choose."
            )));
        }
        return Ok(OperationStepTarget {
            requirement,
            operation: None,
            from,
            declared: None,
            loaded: None,
        });
    }
    let reference = non_empty(value, &qualified)?;
    let (requirement, operation) = reference
        .rsplit_once('/')
        .filter(|(requirement, operation)| {
            is_connector_name(requirement) && is_operation_name(operation)
        })
        .ok_or_else(|| {
            validation_error(format!(
                "{qualified} '{reference}' names <connector>/<operation>, or chooses one per run with {{connector, choose, from}}."
            ))
        })?;
    Ok(OperationStepTarget {
        requirement: requirement.to_owned(),
        operation: Some(OperationName::new(operation).ok_or_else(|| {
            validation_error(format!(
                "{qualified} '{operation}' is not a valid operation name."
            ))
        })?),
        from: Vec::new(),
        declared: None,
        loaded: None,
    })
}

/// The operation call: the step's own call fields, as the runtime decodes
/// them, each binding left in place for the call's bindings to fill. A per-run
/// choice reads its binding as the call's `operation`, and `confirms: <id>`
/// reads the confirmed step's provider operation.
fn operation_call(
    raw_step: &JsonObject,
    field: &str,
    previous_steps: &BTreeMap<String, bool>,
) -> Result<JsonObject, ValidationError> {
    let mut call = JsonObject::new();
    for key in ["inputs", "approval", "idempotency_key"] {
        if let Some(value) = raw_step.get(key) {
            call.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(JsonValue::Object(chosen)) = raw_step.get("operation")
        && let Some(choose) = chosen.get("choose")
    {
        call.insert("operation".to_owned(), choose.clone());
    }
    if let Some(result) = raw_step.get("result") {
        let record = required_object(Some(result), &format!("{field}.result"))?;
        reject_unknown(record, &format!("{field}.result"), RESULT_FIELDS)?;
        call.insert("result".to_owned(), result.clone());
    }
    if let Some(confirms) = raw_step.get("confirms") {
        let confirmed = non_empty(Some(confirms), &format!("{field}.confirms"))?;
        match previous_steps.get(&confirmed) {
            Some(true) => {}
            Some(false) => {
                return Err(validation_error(format!(
                    "{field}.confirms '{confirmed}' is not an operation step; a read confirms another operation's effect."
                )));
            }
            None => {
                return Err(validation_error(format!(
                    "{field}.confirms names unknown or later step '{confirmed}'."
                )));
            }
        }
        call.insert(
            "confirms".to_owned(),
            JsonValue::String(format!("$steps.{confirmed}.provider_operation")),
        );
    }
    Ok(call)
}

/// The outputs a step keeps as artifacts: each output naming a `packet`, and
/// `result_as`, the output the whole result becomes, with its packet.
fn output_artifacts(
    outputs: Option<&JsonObject>,
    result_as: Option<String>,
    field: &str,
) -> Result<Option<SkillArtifactContract>, ValidationError> {
    let mut packets = BTreeMap::new();
    for (name, declaration) in outputs.into_iter().flatten() {
        let Some(packet) = declaration.as_object().and_then(|spec| spec.get("packet")) else {
            continue;
        };
        let packet = packet
            .as_str()
            .and_then(|packet| PacketId::new(packet.to_owned()))
            .ok_or_else(|| {
                validation_error(format!(
                    "{field}.outputs.{name}.packet is not a packet id; {}.",
                    runx_contracts::PACKET_ID_EXPECTED
                ))
            })?;
        packets.insert(name.clone(), packet);
    }
    if let Some(result_as) = &result_as
        && !outputs.is_some_and(|outputs| outputs.contains_key(result_as))
    {
        return Err(validation_error(format!(
            "{field}.result_as names {result_as}, which the step's outputs don't declare."
        )));
    }
    let packet = result_as
        .as_ref()
        .and_then(|result_as| packets.remove(result_as));
    if packets.is_empty() && result_as.is_none() {
        return Ok(None);
    }
    Ok(Some(SkillArtifactContract {
        emits: None,
        named_emits: (!packets.is_empty()).then(|| {
            packets
                .keys()
                .map(|name| (name.clone(), name.clone()))
                .collect()
        }),
        packets: (!packets.is_empty()).then_some(packets),
        wrap_as: result_as,
        packet,
    }))
}

/// A list of conditions, all of which must hold before the step may run.
fn validate_guard(
    value: Option<&JsonValue>,
    previous_step_ids: &BTreeSet<String>,
    field: &str,
) -> Result<Vec<WorkflowWhen>, ValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    required_array(Some(value), field)?
        .iter()
        .enumerate()
        .map(|(index, condition_value)| {
            let field = format!("{field}[{index}]");
            condition(
                required_object(Some(condition_value), &field)?,
                previous_step_ids,
                &field,
            )
        })
        .collect()
}

/// `{ value, equals }` or `{ value, not_equals }`.
fn condition(
    record: &JsonObject,
    previous_step_ids: &BTreeSet<String>,
    field: &str,
) -> Result<WorkflowWhen, ValidationError> {
    if let Some(key) = record
        .keys()
        .find(|key| !matches!(key.as_str(), "value" | "equals" | "not_equals"))
    {
        return Err(validation_error(format!(
            "{field}.{key} is not a field of a condition; it takes value and equals or not_equals."
        )));
    }
    let equals = record
        .get("equals")
        .map(|value| parse_literal(value, &format!("{field}.equals")))
        .transpose()?;
    let not_equals = record
        .get("not_equals")
        .map(|value| parse_literal(value, &format!("{field}.not_equals")))
        .transpose()?;
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
    Ok(WorkflowWhen {
        value: parse_operand(value, previous_step_ids, &format!("{field}.value"))?,
        equals,
        not_equals,
    })
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
