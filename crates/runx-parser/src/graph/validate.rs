use std::collections::BTreeSet;

use runx_contracts::JsonObject;

use super::helpers::{
    optional_string, optional_string_array, required_array, required_object, required_string,
    validation_error,
};
use super::policy::validate_graph_policy;
use super::step::validate_step;
use super::types::{ExecutionGraph, RawGraphIr};
use crate::{ParseError, ValidationError, assert_yaml_supported_subset};

pub fn parse_graph_yaml(source: &str) -> Result<RawGraphIr, ParseError> {
    assert_yaml_supported_subset("graph", source)?;
    let document: JsonObject =
        serde_norway::from_str(source).map_err(|error| ParseError::InvalidYaml {
            field: "graph".to_owned(),
            message: error.to_string(),
        })?;
    Ok(RawGraphIr { document })
}

pub fn validate_graph(raw: RawGraphIr) -> Result<ExecutionGraph, ValidationError> {
    validate_graph_document(raw.document.clone(), Some(raw))
}

pub fn validate_graph_document(
    document: JsonObject,
    raw: Option<RawGraphIr>,
) -> Result<ExecutionGraph, ValidationError> {
    reject_unsupported_top_level(&document)?;

    let name = required_string(document.get("name"), "name")?;
    let publisher = optional_string(document.get("publisher"), "publisher")?;
    let result_from =
        optional_string_array(document.get("result_from"), "result_from")?.unwrap_or_default();
    let raw_steps = required_array(document.get("steps"), "steps")?;
    let mut seen_step_ids = BTreeSet::new();
    let mut steps = Vec::new();

    for (index, raw_step) in raw_steps.iter().enumerate() {
        let field = format!("steps.{index}");
        let raw_step = required_object(Some(raw_step), &field)?;
        let step = validate_step(raw_step, &field, &seen_step_ids)?;
        seen_step_ids.insert(step.id.clone());
        steps.push(step);
    }

    validate_result_producers(&result_from, &seen_step_ids)?;
    validate_gated_context(&steps)?;
    let order = steps
        .iter()
        .map(|step| step.id.as_str())
        .collect::<Vec<_>>();
    let policy = validate_graph_policy(document.get("policy"), &order, "policy")?;
    validate_ask_addresses(&steps, &seen_step_ids)?;

    Ok(ExecutionGraph {
        name,
        publisher,
        result_from,
        steps,
        policy,
        raw: raw.unwrap_or(RawGraphIr { document }),
    })
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
    steps: &[crate::GraphStep],
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
fn validate_gated_context(steps: &[crate::GraphStep]) -> Result<(), ValidationError> {
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

/// The fields of a standalone graph document.
const GRAPH_FIELDS: &[&str] = &["name", "publisher", "result_from", "steps", "policy"];

fn reject_unsupported_top_level(document: &JsonObject) -> Result<(), ValidationError> {
    match document
        .keys()
        .find(|key| !GRAPH_FIELDS.contains(&key.as_str()))
    {
        Some(key) => Err(validation_error(format!("{key} is not a graph field."))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_graph_yaml, validate_graph};

    #[test]
    fn a_step_reads_earlier_steps_and_the_run_input_by_reference() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: references
steps:
  - id: select
    agent: {role: reviewer, task: review}
  - id: review
    agent: {role: reviewer, task: review}
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
    agent: {role: reviewer, task: review}
    inputs:
      bounty: $steps.select.result
  - id: select
    agent: {role: reviewer, task: review}
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
                "context does not apply to this agent step",
            ),
            (
                "    idempotency_key: review-1\n",
                "idempotency_key does not apply to this agent step",
            ),
        ] {
            let source = format!(
                "name: review\nsteps:\n  - id: scan\n    tool: data.digest\n  - id: review\n    agent: {{role: reviewer, task: review}}\n{fragment}"
            );
            let message = validate_err(&source)?;
            assert!(message.contains(expected), "{fragment}: {message}");
        }
        Ok(())
    }

    #[test]
    fn typed_graph_ir_preserves_step_output_contracts() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: typed-step-outputs
steps:
  - id: analyze
    agent: {role: reviewer, task: review}
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
    agent: {role: reviewer, task: review}
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
    agent: {{role: reviewer, task: review}}
  - id: draft
    when: {{value: $steps.admit.result.decision, equals: ready}}
    agent: {{role: reviewer, task: review}}
  - id: finalize{consumer_when}
    inputs:
      draft: $steps.draft.result
    agent: {{role: reviewer, task: review}}
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
    agent: {role: reviewer, task: review}
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
    input: {{text: hello}}
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

    fn validate_yaml(source: &str) -> Result<crate::ExecutionGraph, String> {
        let raw = parse_graph_yaml(source).map_err(|error| error.to_string())?;
        validate_graph(raw).map_err(|error| error.to_string())
    }

    fn validate_err(source: &str) -> Result<String, String> {
        let raw = parse_graph_yaml(source).map_err(|error| error.to_string())?;
        validate_graph(raw)
            .err()
            .map(|error| error.to_string())
            .ok_or_else(|| "graph unexpectedly validated".to_owned())
    }

    #[test]
    fn operation_steps_validate_with_their_duplicate_tolerance() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: deliver
steps:
  - id: post
    operation: slack/channel.post
    duplicates: forbidden
    idempotency_key: notify-1
    input:
      channel_locator: slack://acme/C1
      text: hello
"#,
        )?;
        let step = &graph.steps[0];
        let operation = step.operation().ok_or("expected an operation")?;
        assert_eq!(operation.requirement, "slack");
        assert_eq!(
            operation.operation.as_ref().map(|name| name.as_str()),
            Some("channel.post")
        );
        assert_eq!(step.duplicates, Some(crate::DuplicatePolicy::Forbidden));
        Ok(())
    }

    #[test]
    fn an_operation_chosen_at_run_time_is_its_operation_name() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: sync
steps:
  - id: plan
    tool: data.digest
  - id: push
    operation: github
    operation_name: $steps.plan.sync_plan.provider_operation
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
        assert!(message.contains("choose the operation with operation_name"));
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
                "    inputs: {input: {}}\n",
                "does not apply to this operation step",
            ),
            (
                "    operation_name: $steps.plan.digest\n",
                "names its operation and also an operation_name",
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
    duplicates: visible
"#,
        )?;
        assert!(message.contains("does not apply to this tool step"));
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
}
