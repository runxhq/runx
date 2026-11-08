use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::JsonObject;

use super::helpers::{
    narrowing, optional_string_array, required_array, required_object, required_string,
    validation_error,
};
use super::step::validate_step;
use super::types::{RawWorkflowIr, Workflow};
use crate::{ParseError, ValidationError, assert_yaml_supported_subset};

pub fn parse_workflow_yaml(source: &str) -> Result<RawWorkflowIr, ParseError> {
    assert_yaml_supported_subset("graph", source)?;
    let document: JsonObject =
        serde_norway::from_str(source).map_err(|error| ParseError::InvalidYaml {
            field: "graph".to_owned(),
            message: error.to_string(),
        })?;
    Ok(RawWorkflowIr { document })
}

pub fn validate_workflow(raw: RawWorkflowIr) -> Result<Workflow, ValidationError> {
    validate_workflow_document(raw.document.clone(), Some(raw))
}

pub fn validate_workflow_document(
    document: JsonObject,
    raw: Option<RawWorkflowIr>,
) -> Result<Workflow, ValidationError> {
    reject_unsupported_top_level(&document)?;

    let name = required_string(document.get("name"), "name")?;
    let result_from =
        optional_string_array(document.get("result_from"), "result_from")?.unwrap_or_default();
    let (effects, reach) = narrowing(&document, "")?;
    let raw_steps = required_array(document.get("steps"), "steps")?;
    // Each earlier step, and whether it is an operation, which a read may
    // confirm.
    let mut seen_steps = BTreeMap::new();
    let mut steps = Vec::new();

    for (index, raw_step) in raw_steps.iter().enumerate() {
        let field = format!("steps.{index}");
        let raw_step = required_object(Some(raw_step), &field)?;
        let step = validate_step(raw_step, &field, &seen_steps)?;
        seen_steps.insert(
            step.id.clone(),
            matches!(step.kind, crate::StepKind::Operation(_)),
        );
        steps.push(step);
    }
    let seen_step_ids = seen_steps.into_keys().collect::<BTreeSet<_>>();

    validate_result_producers(&result_from, &seen_step_ids)?;
    validate_gated_context(&steps)?;
    validate_ask_addresses(&steps, &seen_step_ids)?;
    let outputs = validate_method_outputs(document.get("outputs"), &result_from, &steps)?;

    Ok(Workflow {
        name,
        result_from,
        outputs,
        effects,
        reach,
        steps,
        raw: raw.unwrap_or(RawWorkflowIr { document }),
    })
}

/// A method's declared `outputs`: a valid output contract, each of whose
/// required fields one of its result producers can emit. A producer that
/// declares no outputs, such as a skill step returning its child's result,
/// can emit any.
fn validate_method_outputs(
    value: Option<&runx_contracts::JsonValue>,
    result_from: &[String],
    steps: &[crate::WorkflowStep],
) -> Result<Option<JsonObject>, ValidationError> {
    let Some(outputs) = super::helpers::optional_object(value, "outputs")? else {
        return Ok(None);
    };
    let fields = runx_contracts::parse_output_contract(&outputs)
        .map_err(|error| validation_error(format!("outputs is invalid: {error}")))?;
    if fields.is_empty() {
        return Err(validation_error("outputs lists nothing."));
    }
    let producers = steps
        .iter()
        .filter(|step| result_from.contains(&step.id))
        .map(|step| step.outputs.as_ref())
        .collect::<Vec<_>>();
    for (name, field) in &fields {
        let emitted = producers
            .iter()
            .flatten()
            .filter_map(|declared| declared.get(name))
            .collect::<Vec<_>>();
        if emitted.is_empty() && !field.optional && producers.iter().all(Option::is_some) {
            return Err(validation_error(format!(
                "outputs.{name} is emitted by none of the steps in result_from ({}).",
                result_from.join(", ")
            )));
        }
        if let Some(packet) = &field.packet {
            let same_packet = |declared: &&runx_contracts::JsonValue| {
                declared
                    .as_object()
                    .and_then(|declared| declared.get("packet"))
                    .and_then(runx_contracts::JsonValue::as_str)
                    == Some(packet.as_str())
            };
            if let Some(other) = emitted.iter().find(|declared| !same_packet(declared)) {
                return Err(validation_error(format!(
                    "outputs.{name} is packet {}, and a step in result_from emits it as {}.",
                    packet.as_str(),
                    runx_contracts::canonical_json::canonical_stable_json(other)
                        .unwrap_or_default()
                )));
            }
        }
    }
    Ok(Some(outputs))
}

fn validate_result_producers(
    result_from: &[String],
    step_ids: &BTreeSet<String>,
) -> Result<(), ValidationError> {
    let mut seen = BTreeSet::new();
    for (index, step_id) in result_from.iter().enumerate() {
        if step_id.trim().is_empty() {
            return Err(validation_error(format!(
                "result_from.{index} must not be empty."
            )));
        }
        if !step_ids.contains(step_id) {
            return Err(validation_error(format!(
                "result_from.{index} references unknown step '{step_id}'."
            )));
        }
        if !seen.insert(step_id) {
            return Err(validation_error(format!(
                "result_from contains duplicate step '{step_id}'."
            )));
        }
    }
    Ok(())
}

/// An ask is addressed by its gate and an operation's approval by its step, in
/// one run, so a gate names one ask and no step: one answer never clears two.
fn validate_ask_addresses(
    steps: &[crate::WorkflowStep],
    step_ids: &BTreeSet<String>,
) -> Result<(), ValidationError> {
    let mut gates = BTreeSet::new();
    for (index, step) in steps.iter().enumerate() {
        let crate::StepKind::Ask(ask) = &step.kind else {
            continue;
        };
        if step_ids.contains(&ask.gate) {
            return Err(validation_error(format!(
                "steps.{index}.ask.gate '{}' names a step; an ask's gate addresses its request, so it must differ from every step id.",
                ask.gate
            )));
        }
        if !gates.insert(ask.gate.as_str()) {
            return Err(validation_error(format!(
                "steps.{index}.ask.gate '{}' is already another ask's gate; each ask needs its own, so one answer clears one ask.",
                ask.gate
            )));
        }
    }
    Ok(())
}

/// A step with no gate of its own that reads a gated step's output can never
/// run when that step is skipped, so the graph would block there instead of
/// finishing. It must carry a gate.
fn validate_gated_context(steps: &[crate::WorkflowStep]) -> Result<(), ValidationError> {
    let gated = steps
        .iter()
        .filter(|step| step.when.is_some())
        .map(|step| step.id.as_str())
        .collect::<BTreeSet<_>>();
    for (index, step) in steps.iter().enumerate() {
        if step.when.is_some() {
            continue;
        }
        if let Some(source) = step
            .reads()
            .into_iter()
            .find(|source| gated.contains(source))
        {
            return Err(validation_error(format!(
                "steps.{index} ({}) reads gated step '{source}' but has no when of its own; it would block whenever that step is skipped.",
                step.id
            )));
        }
    }
    Ok(())
}

/// The fields of a workflow document: a method's steps, under its skill's
/// name.
const WORKFLOW_FIELDS: &[&str] = &[
    "name",
    "result_from",
    "outputs",
    "effects",
    "reach",
    "steps",
];

fn reject_unsupported_top_level(document: &JsonObject) -> Result<(), ValidationError> {
    match document
        .keys()
        .find(|key| !WORKFLOW_FIELDS.contains(&key.as_str()))
    {
        Some(key) => Err(validation_error(format!("{key} is not a graph field."))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_workflow_yaml, validate_workflow};

    #[test]
    fn a_step_reads_earlier_steps_and_the_run_input_by_reference() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: references
steps:
  - id: select
    agent: review
  - id: review
    agent: review
    inputs:
      bounty: $steps.select.result
      claim: $input.claim
      literal: select.result
      url: https://example.com/a.b
"#,
        )?;
        let review = &graph.steps[1];
        assert_eq!(review.reads(), ["select"]);
        assert_eq!(review.bindings.len(), 2);
        assert_eq!(
            review
                .inputs
                .get("literal")
                .and_then(|value| value.as_str()),
            Some("select.result"),
            "a bare step.field string is a literal"
        );
        let message = validate_err(
            r#"
name: forward
steps:
  - id: review
    agent: review
    inputs:
      bounty: $steps.select.result
  - id: select
    agent: review
"#,
        )?;
        assert!(
            message.contains("unknown or later step 'select'"),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn a_step_has_no_context_or_key_outside_an_operation() -> Result<(), String> {
        for (fragment, expected) in [
            (
                "    context: {readme: scan.result}\n",
                "context.readme is not a field here",
            ),
            (
                "    idempotency_key: review-1\n",
                "idempotency_key does not apply to this agent step",
            ),
        ] {
            let source = format!(
                "name: review\nsteps:\n  - id: scan\n    tool: data.digest\n  - id: review\n    agent: review\n{fragment}"
            );
            let message = validate_err(&source)?;
            assert!(message.contains(expected), "{fragment}: {message}");
        }
        Ok(())
    }

    #[test]
    fn typed_workflow_ir_preserves_step_output_contracts() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: typed-step-outputs
steps:
  - id: analyze
    agent: review
    outputs:
      verdict:
        type: string
        enum: [ready, blocked]
"#,
        )?;
        let outputs = graph.steps[0]
            .outputs
            .as_ref()
            .ok_or_else(|| "typed graph step omitted outputs".to_owned())?;
        let verdict = outputs
            .get("verdict")
            .and_then(runx_contracts::JsonValue::as_object)
            .ok_or_else(|| "typed graph step omitted verdict schema".to_owned())?;

        assert_eq!(
            verdict
                .get("type")
                .and_then(runx_contracts::JsonValue::as_str),
            Some("string")
        );
        assert_eq!(
            verdict
                .get("enum")
                .and_then(runx_contracts::JsonValue::as_array)
                .map(Vec::len),
            Some(2)
        );
        Ok(())
    }

    #[test]
    fn result_from_rejects_unknown_steps() -> Result<(), String> {
        let error = validate_err(
            r#"
name: unknown-result
result_from:
  - missing
steps:
  - id: present
    agent: review
"#,
        )?;
        assert!(error.contains("result_from.0 references unknown step 'missing'"));
        Ok(())
    }

    #[test]
    fn a_step_reading_a_gated_step_carries_a_gate() -> Result<(), String> {
        let graph = |consumer_when: &str| {
            format!(
                r#"
name: gated
steps:
  - id: admit
    agent: review
  - id: draft
    when: {{value: $steps.admit.result.decision, equals: ready}}
    agent: review
  - id: finalize{consumer_when}
    inputs:
      draft: $steps.draft.result
    agent: review
"#
            )
        };
        let error = validate_err(&graph(""))?;
        assert!(
            error.contains("steps.2 (finalize) reads gated step 'draft'"),
            "{error}"
        );
        validate_yaml(&graph(
            "\n    when: {value: $steps.admit.result.decision, equals: ready}",
        ))?;
        Ok(())
    }

    #[test]
    fn result_from_rejects_duplicate_steps() -> Result<(), String> {
        let error = validate_err(
            r#"
name: duplicate-result
result_from:
  - result
  - result
steps:
  - id: result
    agent: review
"#,
        )?;
        assert!(error.contains("result_from contains duplicate step 'result'"));
        Ok(())
    }

    #[test]
    fn an_ask_gate_cannot_share_a_step_address() -> Result<(), String> {
        let graph = |gate: &str| {
            format!(
                r#"
name: approved-send
steps:
  - id: confirm
    ask: {{gate: {gate}, reason: Approve the send.}}
  - id: send
    operation: example/message.send
    idempotency_key: send-1
    inputs: {{text: hello}}
"#
            )
        };
        let error = validate_err(&graph("send"))?;
        assert!(
            error.contains("steps.0.ask.gate 'send' names a step"),
            "{error}"
        );
        validate_yaml(&graph("send.approval"))?;
        Ok(())
    }

    #[test]
    fn two_asks_cannot_share_a_gate() -> Result<(), String> {
        let graph = |second: &str| {
            format!(
                "name: twice\nsteps:\n  - id: first\n    ask: {{gate: approve.send, reason: Send it?}}\n  - id: second\n    ask: {{gate: {second}, reason: Send it again?}}\n"
            )
        };
        let error = validate_err(&graph("approve.send"))?;
        assert!(
            error.contains("steps.1.ask.gate 'approve.send' is already another ask's gate"),
            "{error}"
        );
        validate_yaml(&graph("approve.resend"))?;
        Ok(())
    }

    #[test]
    fn an_ask_reason_is_text_and_its_values_are_its_inputs() -> Result<(), String> {
        let message = validate_err(
            "name: ask\nsteps:\n  - id: approve\n    ask: {gate: approve.send, reason: $input.title}\n",
        )?;
        assert!(
            message.contains("reason is text, not a reference"),
            "{message}"
        );
        let graph = validate_yaml(
            "name: ask\nsteps:\n  - id: approve\n    ask: {gate: approve.send, reason: $$5 to send}\n    inputs: {title: $input.title}\n",
        )?;
        let crate::StepKind::Ask(ask) = &graph.steps[0].kind else {
            return Err("an ask step".to_owned());
        };
        assert_eq!(ask.reason, "$5 to send");
        assert_eq!(graph.steps[0].bindings.len(), 1);
        Ok(())
    }

    fn validate_yaml(source: &str) -> Result<crate::Workflow, String> {
        let raw = parse_workflow_yaml(source).map_err(|error| error.to_string())?;
        validate_workflow(raw).map_err(|error| error.to_string())
    }

    fn validate_err(source: &str) -> Result<String, String> {
        let raw = parse_workflow_yaml(source).map_err(|error| error.to_string())?;
        validate_workflow(raw)
            .err()
            .map(|error| error.to_string())
            .ok_or_else(|| "graph unexpectedly validated".to_owned())
    }

    #[test]
    fn operation_steps_validate_with_their_retry_tolerance() -> Result<(), String> {
        // `duplicates: forbidden` was the default, so it converts to nothing:
        // recovery never resends a write whose outcome is unknown unless the
        // step says `retry_unknown_outcomes: true`.
        let source = r#"
name: deliver
steps:
  - id: post
    operation: slack/channel.post
    idempotency_key: notify-1
    inputs:
      channel_locator: slack://acme/C1
      text: hello
"#;
        let graph = validate_yaml(source)?;
        let step = &graph.steps[0];
        let operation = step.operation().ok_or("expected an operation")?;
        assert_eq!(operation.requirement, "slack");
        assert_eq!(
            operation.operation.as_ref().map(|name| name.as_str()),
            Some("channel.post")
        );
        assert!(!step.retry_unknown_outcomes);
        let visible = validate_yaml(&source.replace(
            "    idempotency_key: notify-1\n",
            "    retry_unknown_outcomes: true\n    idempotency_key: notify-1\n",
        ))?;
        assert!(visible.steps[0].retry_unknown_outcomes);
        Ok(())
    }

    #[test]
    fn an_operation_chosen_at_run_time_is_its_choice() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: sync
steps:
  - id: plan
    tool: data.digest
  - id: push
    operation:
      connector: github
      choose: $steps.plan.sync_plan.provider_operation
      from: [issues.list, issues.get]
"#,
        )?;
        assert_eq!(
            graph.steps[1]
                .operation()
                .map(|target| target.operation.is_none()),
            Some(true)
        );
        assert_eq!(graph.steps[1].reads(), ["plan"]);
        let message = validate_err(
            r#"
name: sync
steps:
  - id: push
    operation: github
"#,
        )?;
        assert!(
            message.contains("chooses one per run with {connector, choose, from}"),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn operation_steps_reject_what_the_connector_owns() -> Result<(), String> {
        for (fragment, expected) in [
            (
                "    scopes: [channel.post]\n",
                "does not apply to this operation step",
            ),
            (
                "    input: {}\n",
                "input does not apply to this operation step",
            ),
            (
                "    operation_name: $steps.plan.digest\n",
                "operation_name does not apply to this operation step",
            ),
        ] {
            let source = format!(
                "name: deliver\nsteps:\n  - id: plan\n    tool: data.digest\n  - id: post\n    operation: slack/channel.post\n{fragment}"
            );
            let message = validate_err(&source)?;
            assert!(message.contains(expected), "{fragment}: {message}");
        }
        Ok(())
    }

    #[test]
    fn tolerance_applies_only_to_operation_steps() -> Result<(), String> {
        let message = validate_err(
            r#"
name: tool-step
steps:
  - id: digest
    tool: data.digest
    retry_unknown_outcomes: true
"#,
        )?;
        assert!(
            message.contains("retry_unknown_outcomes does not apply to this tool step"),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn unknown_step_fields_fail_closed() -> Result<(), String> {
        let message = validate_err(
            r#"
name: typo
steps:
  - id: digest
    tool: data.digest
    contxt:
      value: input.x
"#,
        )?;
        assert!(message.contains("contxt does not apply to this tool step"));
        Ok(())
    }

    #[test]
    fn a_method_and_its_steps_read_effects_and_reach_as_written() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: narrow
effects: ask
reach: workspace
steps:
  - id: quiet
    tool: data.digest
    effects: none
    inputs: {value: 1}
  - id: wide
    tool: data.digest
    effects: allow
    reach: inputs
    inputs: {value: 2}
  - id: plain
    tool: data.digest
    inputs: {value: 3}
"#,
        )?;
        assert_eq!(graph.effects, Some(runx_contracts::Effects::Ask));
        assert_eq!(graph.reach, Some(runx_contracts::Reach::Workspace));
        assert_eq!(graph.steps[0].effects, Some(runx_contracts::Effects::None));
        assert_eq!(graph.steps[0].reach, None);
        // A value wider than its run's parses; it only never widens.
        assert_eq!(graph.steps[1].effects, Some(runx_contracts::Effects::Allow));
        assert_eq!(graph.steps[1].reach, Some(runx_contracts::Reach::Inputs));
        assert_eq!((graph.steps[2].effects, graph.steps[2].reach), (None, None));

        for (source, expected) in [
            (
                "name: n\neffects: free\nsteps:\n  - {id: a, tool: data.digest}\n",
                "effects must be one of none, ask and allow.",
            ),
            (
                "name: n\nsteps:\n  - {id: a, tool: data.digest, effects: read_only}\n",
                "steps.0.effects must be one of none, ask and allow.",
            ),
            (
                "name: n\nsteps:\n  - {id: a, tool: data.digest, reach: agent}\n",
                "steps.0.reach must be one of workspace and inputs.",
            ),
        ] {
            let message = validate_err(source)?;
            assert!(message.contains(expected), "{message}");
        }
        Ok(())
    }
}
