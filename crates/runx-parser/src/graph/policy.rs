use std::collections::BTreeSet;

use runx_contracts::JsonValue;

use super::binding::parse_operand;
use super::helpers::{
    optional_object, required_array, required_object, required_string, validation_error,
};
use super::types::{GraphGuard, GraphPolicy};
use crate::ValidationError;

/// The graph's guards. `steps` is the graph's step ids in order: a guard
/// gates one of them and reads only the steps before it.
pub fn validate_graph_policy(
    value: Option<&JsonValue>,
    steps: &[&str],
    field: &str,
) -> Result<Option<GraphPolicy>, ValidationError> {
    let Some(policy) = optional_object(value, field)? else {
        return Ok(None);
    };
    // Fail closed on unknown policy fields: `guards` is the only supported key.
    // Silently ignoring others drops gates without warning; a stale `transitions`
    // key (since renamed to `guards`) once disabled every payment gate this way.
    for key in policy.keys() {
        if key.as_str() != "guards" {
            return Err(validation_error(format!(
                "{field} has unknown field '{key}'; only 'guards' is supported."
            )));
        }
    }
    let Some(guards_value) = policy.get("guards") else {
        return Ok(None);
    };
    if matches!(guards_value, JsonValue::Null) {
        return Ok(None);
    }
    let guards = required_array(Some(guards_value), &format!("{field}.guards"))?
        .iter()
        .enumerate()
        .map(|(index, raw_gate)| guard(raw_gate, steps, &format!("{field}.guards.{index}")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(GraphPolicy { guards }))
}

fn guard(
    raw_gate: &JsonValue,
    steps: &[&str],
    gate_field: &str,
) -> Result<GraphGuard, ValidationError> {
    let gate = required_object(Some(raw_gate), gate_field)?;
    if let Some(key) = gate
        .keys()
        .find(|key| !matches!(key.as_str(), "step" | "value" | "equals" | "not_equals"))
    {
        return Err(validation_error(format!(
            "{gate_field}.{key} is not a field of a guard; it takes step, value and equals or not_equals."
        )));
    }
    let equals = gate.get("equals").cloned();
    let not_equals = gate.get("not_equals").cloned();
    if equals.is_some() && not_equals.is_some() {
        return Err(validation_error(format!(
            "{gate_field} must not declare both equals and not_equals."
        )));
    }
    if equals.is_none() && not_equals.is_none() {
        return Err(validation_error(format!(
            "{gate_field} must declare equals or not_equals."
        )));
    }
    let step = required_string(gate.get("step"), &format!("{gate_field}.step"))?;
    let Some(position) = steps.iter().position(|id| *id == step) else {
        return Err(validation_error(format!(
            "{gate_field}.step '{step}' is not a step of this graph."
        )));
    };
    let earlier = steps[..position]
        .iter()
        .map(|id| (*id).to_owned())
        .collect::<BTreeSet<_>>();
    let value = gate
        .get("value")
        .ok_or_else(|| validation_error(format!("{gate_field}.value is required.")))?;
    Ok(GraphGuard {
        value: parse_operand(value, &earlier, &format!("{gate_field}.value"))?,
        step,
        equals,
        not_equals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_value(json: &str) -> Result<JsonValue, String> {
        serde_json::from_str(json)
            .map_err(|error| format!("valid json fixture failed to parse: {error}"))
    }

    #[test]
    fn rejects_unknown_policy_field() -> Result<(), String> {
        // A stale `transitions` key (the pre-rename name) must be rejected, not
        // silently dropped: dropping it would disable the gate it declares.
        let policy = policy_value(
            r#"{"transitions":[{"step":"fulfill","value":"$steps.approve-spend.data.approved","equals":true}]}"#,
        )?;
        let error =
            match validate_graph_policy(Some(&policy), &["approve-spend", "fulfill"], "policy") {
                Ok(_) => return Err("unknown policy field must be rejected".to_owned()),
                Err(error) => error,
            };
        assert!(
            error.to_string().contains("transitions"),
            "error should name the unknown field, got: {error}"
        );
        Ok(())
    }

    #[test]
    fn accepts_guards_policy() -> Result<(), String> {
        let policy = policy_value(
            r#"{"guards":[{"step":"fulfill","value":"$steps.approve-spend.data.approved","equals":true}]}"#,
        )?;
        let parsed =
            match validate_graph_policy(Some(&policy), &["approve-spend", "fulfill"], "policy") {
                Ok(Some(parsed)) => parsed,
                Ok(None) => return Err("guards present".to_owned()),
                Err(error) => return Err(format!("a guards policy must parse: {error}")),
            };
        assert_eq!(parsed.guards.len(), 1);
        Ok(())
    }

    #[test]
    fn a_guard_reads_only_steps_before_the_one_it_gates() -> Result<(), String> {
        let policy = policy_value(
            r#"{"guards":[{"step":"approve-spend","value":"$steps.fulfill.data.ok","equals":true}]}"#,
        )?;
        let error = validate_graph_policy(Some(&policy), &["approve-spend", "fulfill"], "policy")
            .err()
            .ok_or("a guard reading a later step must be refused")?;
        assert!(
            error
                .to_string()
                .contains("unknown or later step 'fulfill'"),
            "{error}"
        );
        let stale = policy_value(
            r#"{"guards":[{"step":"fulfill","field":"approve-spend.data.approved","equals":true}]}"#,
        )?;
        let error = validate_graph_policy(Some(&stale), &["approve-spend", "fulfill"], "policy")
            .err()
            .ok_or("a guard's field key is not a reference")?;
        assert!(
            error
                .to_string()
                .contains("field is not a field of a guard"),
            "{error}"
        );
        Ok(())
    }
}
