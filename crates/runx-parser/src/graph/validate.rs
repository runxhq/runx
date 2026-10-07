use std::collections::BTreeSet;

use runx_contracts::JsonObject;

use super::fanout::{validate_fanout_groups, validate_fanout_step_bindings};
use super::helpers::{
    optional_string, optional_string_array, required_array, required_object, required_string,
    validation_error,
};
use super::policy::validate_graph_policy;
use super::step::validate_step;
use super::types::{ExecutionGraph, RawGraphIr};
use crate::{ParseError, ValidationError, assert_yaml_parity_subset};

pub fn parse_graph_yaml(source: &str) -> Result<RawGraphIr, ParseError> {
    assert_yaml_parity_subset("graph", source)?;
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
    let owner = optional_string(document.get("owner"), "owner")?;
    let result_from =
        optional_string_array(document.get("result_from"), "result_from")?.unwrap_or_default();
    let charter_from = optional_string(document.get("charter_from"), "charter_from")?;
    let raw_steps = required_array(document.get("steps"), "steps")?;
    let fanout_groups = validate_fanout_groups(document.get("fanout"), "fanout")?;
    let policy = validate_graph_policy(document.get("policy"), "policy")?;
    let mut seen_step_ids = BTreeSet::new();
    let mut steps = Vec::new();

    for (index, raw_step) in raw_steps.iter().enumerate() {
        let field = format!("steps.{index}");
        let raw_step = required_object(Some(raw_step), &field)?;
        let step = validate_step(raw_step, &field, &seen_step_ids)?;
        seen_step_ids.insert(step.id.clone());
        steps.push(step);
    }

    validate_fanout_step_bindings(&steps, &fanout_groups)?;
    validate_result_producers(&result_from, &seen_step_ids)?;
    validate_gated_context(&steps)?;

    Ok(ExecutionGraph {
        name,
        owner,
        result_from,
        charter_from,
        steps,
        fanout_groups,
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
        if let Some(edge) = step
            .context_edges
            .iter()
            .find(|edge| gated.contains(edge.from_step.as_str()))
        {
            return Err(validation_error(format!(
                "steps.{index} ({}) reads context from gated step '{}' but has no when of its own; it would block whenever that step is skipped.",
                step.id, edge.from_step
            )));
        }
    }
    Ok(())
}

fn reject_unsupported_top_level(document: &JsonObject) -> Result<(), ValidationError> {
    for field in ["sync", "schedule", "schedules"] {
        if document.contains_key(field) {
            return Err(validation_error(format!(
                "{field} is not supported by the local sequential graph runner."
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_graph_yaml, validate_graph};

    #[test]
    fn inputs_reject_previous_step_output_references() -> Result<(), String> {
        let raw = parse_graph_yaml(
            r#"
name: bad-input-ref
steps:
  - id: select
    run:
      type: agent-task
  - id: review
    run:
      type: agent-task
    inputs:
      bounty: select.result
"#,
        )
        .map_err(|error| error.to_string())?;
        let error = validate_graph(raw)
            .err()
            .ok_or_else(|| "graph unexpectedly validated".to_owned())?;
        let message = error.to_string();
        assert!(message.contains("steps.1.inputs.bounty"));
        assert!(message.contains("move it to context"));
        Ok(())
    }

    #[test]
    fn inputs_allow_literals_that_are_not_previous_step_refs() -> Result<(), String> {
        let raw = parse_graph_yaml(
            r#"
name: literal-input
steps:
  - id: review
    run:
      type: agent-task
    inputs:
      literal: select.result
      variable: $input.claim
      url: https://example.com/a.b
"#,
        )
        .map_err(|error| error.to_string())?;
        validate_graph(raw).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn typed_graph_ir_preserves_step_output_contracts() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: typed-step-outputs
steps:
  - id: analyze
    run:
      type: agent-task
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
    fn inputs_reject_retired_double_brace_bindings() -> Result<(), String> {
        let error = validate_err(
            r#"
name: retired-input-binding
steps:
  - id: review
    run:
      type: agent-task
    inputs:
      claim: "{{ claim }}"
"#,
        )?;
        assert!(error.contains("steps.0.inputs.claim"));
        assert!(error.contains("retired graph input binding"));
        assert!(error.contains("$input.claim"));
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
    run:
      type: agent-task
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
    run:
      type: agent-task
  - id: draft
    when: {{field: admit.result.decision, equals: ready}}
    run:
      type: agent-task
  - id: finalize{consumer_when}
    context:
      draft: draft.result
    run:
      type: agent-task
"#
            )
        };
        let error = validate_err(&graph(""))?;
        assert!(
            error.contains("steps.2 (finalize) reads context from gated step 'draft'"),
            "{error}"
        );
        validate_yaml(&graph(
            "\n    when: {field: admit.result.decision, equals: ready}",
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
    run:
      type: agent-task
"#,
        )?;
        assert!(error.contains("result_from contains duplicate step 'result'"));
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
    fn operation_steps_validate_with_tolerance() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: deliver
steps:
  - id: post
    operation: slack/channel.post
    on_uncertain: expire_after 2h
    duplicates: forbidden
    inputs:
      target: slack://acme/C1
      idempotency_key: notify-1
      input:
        channel_locator: slack://acme/C1
        text: hello
"#,
        )?;
        let step = &graph.steps[0];
        let operation = step.operation.as_ref().ok_or("expected an operation")?;
        assert_eq!(operation.requirement, "slack");
        assert_eq!(
            operation.operation.as_ref().map(|name| name.as_str()),
            Some("channel.post")
        );
        assert_eq!(
            step.on_uncertain,
            Some(crate::UncertainOutcome::ExpireAfter { seconds: 7_200 })
        );
        assert_eq!(step.duplicates, Some(crate::DuplicatePolicy::Forbidden));
        Ok(())
    }

    #[test]
    fn an_operation_chosen_at_run_time_is_bound_as_an_input() -> Result<(), String> {
        let graph = validate_yaml(
            r#"
name: sync
steps:
  - id: plan
    tool: data.digest
  - id: push
    operation: github
    context:
      operation: plan.sync_plan.data.provider_operation
"#,
        )?;
        assert_eq!(
            graph.steps[1]
                .operation
                .as_ref()
                .map(|target| target.operation.is_none()),
            Some(true)
        );
        let message = validate_err(
            r#"
name: sync
steps:
  - id: push
    operation: github
"#,
        )?;
        assert!(message.contains("bind the operation name"));
        Ok(())
    }

    #[test]
    fn operation_steps_reject_what_the_connector_owns() -> Result<(), String> {
        for (fragment, expected) in [
            (
                "    scopes: [channel.post]\n",
                "does not apply to an operation step",
            ),
            (
                "    policy: {provider_permission: {verb: write}}\n",
                "does not apply to an operation step",
            ),
            ("    inputs: {expected_provider: slack}\n", "is retired"),
            (
                "    inputs: {operation: channel.post}\n",
                "also binds an operation input",
            ),
        ] {
            let source = format!(
                "name: deliver\nsteps:\n  - id: post\n    operation: slack/channel.post\n{fragment}"
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
    on_uncertain: hold
"#,
        )?;
        assert!(message.contains("apply only to operation steps"));
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
        assert!(message.contains("contxt is not a graph step field"));
        Ok(())
    }
}
