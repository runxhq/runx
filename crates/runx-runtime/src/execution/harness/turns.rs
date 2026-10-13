//! A harness caller works an agent turn from its script: the calls in order,
//! each through the turn's own call path and checked against what it must
//! return, then the output the turn ends with. Cases record what the turn
//! did, not only its answer.

use std::collections::BTreeMap;

use runx_contracts::{JsonValue, TurnCallOutcome};
use runx_parser::harness_fixture::HarnessTurnScript;

use super::json_assertions::assert_json_expectation;
use crate::Host;
use crate::RuntimeError;
use crate::execution::run_record::local_request_id;
use crate::execution::runner::turn::{Turn, call_tool};

/// Make the calls of `turn`'s script it has not made yet. A script keyed by
/// the turn's address is its own; one keyed by the act's id in its own run
/// works every turn of that id that no address names, in a script or an
/// answer (`answered` says whether an answer names this one). Returns the
/// turn's output once every call has settled, or `None` when the turn has
/// no script or a call waits on something the caller did not answer.
pub(super) fn work_scripted_turn(
    turn: &mut dyn Turn,
    host: &mut dyn Host,
    scripts: &BTreeMap<String, HarnessTurnScript>,
    answered: bool,
) -> Result<Option<JsonValue>, RuntimeError> {
    let address = turn.invocation().id.as_str().to_owned();
    let id = if scripts.contains_key(&address) {
        address.as_str()
    } else if answered {
        return Ok(None);
    } else {
        local_request_id(&address)
    };
    let Some(script) = scripts.get(id) else {
        return Ok(None);
    };
    let made = turn.calls()?.len();
    for (index, call) in script.calls.iter().enumerate().skip(made) {
        let result = call_tool(turn, host, &call.tool, call.input.clone())?;
        if let Some(expect) = &call.expect {
            let actual = JsonValue::from_serialize(&result).map_err(|error| {
                RuntimeError::InvalidRunStep {
                    step_id: turn.step_id().to_owned(),
                    reason: format!("call {index} result does not serialize: {error}"),
                }
            })?;
            assert_json_expectation(
                expect,
                &actual,
                &format!("caller.turns.{id}.calls[{index}].expect"),
            )
            .map_err(|error| RuntimeError::InvalidRunStep {
                step_id: turn.step_id().to_owned(),
                reason: error.to_string(),
            })?;
        }
        if matches!(result.outcome, TurnCallOutcome::Pending { .. }) {
            return Ok(None);
        }
    }
    if turn
        .calls()?
        .iter()
        .any(|call| matches!(call.outcome, TurnCallOutcome::Pending { .. }))
    {
        return Ok(None);
    }
    Ok(Some(JsonValue::Object(script.output.clone())))
}
