//! Script steps. A script is deterministic code in the JavaScript worker that
//! drives the calls its step lists, and the step's run record is its history.
//! Each drive of the step passes the script from the beginning against the
//! calls the record settled, in the order they settled, and keeps it live
//! from there: every call it makes past the record is admitted, executed and
//! recorded by the kernel through the same ledger an agent turn's calls go
//! through, and its settlement is delivered to the script where it stands. A
//! call's identity is its position in the step. After a crash, the next drive
//! of the step replays the record and goes on from the first call it doesn't
//! hold.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;

use runx_contracts::javascript_worker::{
    ScriptCall, ScriptCallOutcome, ScriptCallTarget, ScriptEnding, ScriptRejection,
    ScriptSettlement,
};
use runx_contracts::{JsonObject, JsonValue, TurnCallOutcome, TurnTarget};
use runx_parser::ScriptStep;

use super::super::super::run_record::{CallOutcome, RecordedCall};
use super::super::bounded::run_bounded;
use super::super::turn::{Admission, CallLedger, TurnScope};
use super::{
    RegularSkillSeal, RegularSkillStepOutput, StepHandlerCtx, StepReceiptLineage,
    build_ephemeral_step_output, build_step_output_projection, output_error,
    seal_regular_skill_step,
};
use crate::RuntimeError;
use crate::adapter::{InvocationOutput, SkillAdapter};
use crate::execution::runner::StepRun;
use crate::host::Host;
use crate::output_contract::method_contract_checks;

pub(super) fn run_script_step<A>(
    request: StepHandlerCtx<'_, A>,
    script: &ScriptStep,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        inputs,
        child_act: _,
        provenance: _,
        admission_refs,
        host,
        authority,
        loaded_skill: _,
        // A script reads only its inputs and its calls' results.
        context_entries: _,
        ask_reason: _,
    } = request;
    // A script runs the modules its run admitted.
    let package = runtime.options.package(workflow_dir)?;
    let prepared = runtime
        .javascript
        .prepare_script(script, &package, &runtime.options.env)?;
    let mut ledger = CallLedger::of_step(TurnScope {
        runtime,
        workflow_dir,
        workflow_name,
        step_id: &step.id,
        attempt,
        allowance: &script.allowance,
        connectors: &script.connectors,
        effects: step.effects,
        reach: step.reach,
        reads_local_only: host.reads_local_only(),
    });
    // The script passes from the beginning against its record once per drive
    // of the step, and from there stays live: each call's settlement goes on
    // from where the script stands.
    // Calls that wait on a person or a child, by position. A waiting call
    // never stops the others: the step holds only once none can go further.
    let mut waiting = further_open_calls(&mut ledger, host)?;
    let (mut live, mut passed) = runtime.javascript.start_script(
        &prepared,
        &inputs,
        settlements(&ledger, ledger.recorded()?)?,
        0,
    )?;
    // Every call the script has made, in position order, and how many of
    // them are calls rather than reads of the clock.
    let mut made = Vec::new();
    let mut calls = 0_usize;
    let max_calls = usize::try_from(script.max_calls).unwrap_or(usize::MAX);
    let concurrency = usize::try_from(script.concurrency).unwrap_or(1);
    let ended = loop {
        let recorded = ledger.recorded()?;
        matches_record(&step.id, &passed.calls, recorded)?;
        // Only a call this report holds can be new: every earlier one was made
        // before the script was given its settlement.
        let new = passed
            .calls
            .iter()
            .filter(|call| at(recorded, call.position).is_none())
            .cloned()
            .collect::<Vec<_>>();
        calls += passed
            .calls
            .iter()
            .filter(|call| !matches!(call.target, ScriptCallTarget::Now { .. }))
            .count();
        made.extend(passed.calls);
        match passed.ending {
            ScriptEnding::Diverged { position } => {
                return Err(diverged(
                    &step.id,
                    position,
                    "the record settles a call the script didn't make",
                ));
            }
            ScriptEnding::Threw { message } => break Err(message),
            ScriptEnding::Returned { output } => {
                // The output counts only once every call the script started
                // has settled.
                if let Some(position) = outstanding(&made, recorded) {
                    break Err(format!(
                        "the script returned with call {position} outstanding; a script awaits every call it starts"
                    ));
                }
                break Ok(output);
            }
            ScriptEnding::Waiting => {
                if new.is_empty() {
                    if let Some((position, requests)) = waiting.into_iter().next() {
                        return Err(waiting_on(&step.id, position, requests));
                    }
                    break Err(
                        "the script waits on something that is not one of its calls".to_owned()
                    );
                }
                if calls > max_calls {
                    break Err(format!(
                        "the script reached its max_calls of {}",
                        script.max_calls
                    ));
                }
                let positions = new.iter().map(|call| call.position).collect::<Vec<_>>();
                match make_calls(&mut ledger, host, new, concurrency)? {
                    Err(refusal) => break Err(refusal),
                    Ok(still) => waiting.extend(still),
                }
                let recorded = ledger.recorded()?;
                let fresh = settlements(
                    &ledger,
                    positions
                        .iter()
                        .filter_map(|position| at(recorded, *position)),
                )?;
                if fresh.is_empty() {
                    // Every new call waits, so nothing can go further.
                    if let Some((position, requests)) = waiting.into_iter().next() {
                        return Err(waiting_on(&step.id, position, requests));
                    }
                    break Err("the script's calls settled to nothing".to_owned());
                }
                passed = live.settle(fresh, 0)?;
            }
        }
    };
    drop(live);
    let calls = ledger
        .recorded()?
        .iter()
        .filter_map(|call| match &call.outcome {
            CallOutcome::Completed(run) => Some(run.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut output = match ended {
        Ok(output) => InvocationOutput::runtime_success(
            serde_json::from_value(output)
                .map_err(|source| RuntimeError::json("reading a script's output", source))?,
            0,
        ),
        Err(message) => InvocationOutput::runtime_failure(JsonValue::Null, message, 0),
    };
    let projection = build_step_output_projection(step, &output, None, None)?;
    let ephemeral_contract = build_ephemeral_step_output(step, &output, None, None);
    if output.succeeded() {
        output.observations.contract = method_contract_checks(
            &step.id,
            &output.value,
            step.outputs.as_ref(),
            step.artifacts.as_ref(),
            workflow_dir,
        )?;
    }
    seal_regular_skill_step(
        RegularSkillSeal {
            runtime,
            workflow_dir,
            workflow_name,
            step,
            attempt,
            skill_name: step.id.clone(),
            authority: authority.as_ref(),
            admission_refs,
            closure: None,
            refusal: None,
        },
        RegularSkillStepOutput {
            output,
            projection,
            ephemeral_contract,
            // The step's receipt names each call the script made, in order.
            receipt_lineage: StepReceiptLineage::of_turn(&calls),
        },
    )
}

/// A call that waits, at its position, with the requests it waits on.
type Waiting = (u32, Vec<runx_contracts::ResolutionRequest>);

/// Make each new call: admitted in position order under the driver, then run
/// at most `concurrency` at once, apart from the driver where its host can
/// detach a host for each, and recorded under the driver in the order they
/// finish. Return those that wait on a person or a child; a waiting call
/// doesn't stop the next. A pass that reads `ctx.now()` past its record is
/// refused, since this runtime doesn't record the clock yet.
fn make_calls<A>(
    ledger: &mut CallLedger<'_, A>,
    host: &mut dyn Host,
    new: Vec<ScriptCall>,
    concurrency: usize,
) -> Result<Result<Vec<Waiting>, String>, RuntimeError>
where
    A: SkillAdapter,
{
    let mut waiting = Vec::new();
    let mut admitted = Vec::new();
    for call in new {
        let (target, input) = match ledger_call(&call.target) {
            Ok(Some(call)) => call,
            Ok(None) => {
                return Ok(Err(
                    "ctx.now() is not recorded by this runtime yet, so a script can't read it"
                        .to_owned(),
                ));
            }
            Err(refusal) => return Ok(Err(refusal)),
        };
        match ledger.admit(host, call.position, target, input)? {
            Admission::Settled(result) => {
                if let TurnCallOutcome::Pending { waiting: requests } = result.outcome {
                    waiting.push((call.position, requests));
                }
            }
            Admission::Admitted(call) => admitted.push(*call),
        }
    }
    let detached = (concurrency > 1 && admitted.len() > 1)
        .then(|| {
            admitted
                .iter()
                .map(|_| host.detached())
                .collect::<Option<Vec<_>>>()
        })
        .flatten();
    let mut concluded = |ledger: &mut CallLedger<'_, A>, executed| {
        let result = ledger.conclude(executed)?;
        if let TurnCallOutcome::Pending { waiting: requests } = result.outcome {
            waiting.push((result.index, requests));
        }
        Ok::<(), RuntimeError>(())
    };
    match detached {
        // Each call runs on a thread of its own with its own host.
        Some(hosts) => {
            let scope = ledger.scope();
            std::thread::scope(|threads| {
                run_bounded(
                    admitted.into_iter().zip(hosts),
                    concurrency,
                    |(call, mut detached), done| {
                        threads.spawn(move || {
                            let executed = std::panic::catch_unwind(AssertUnwindSafe(|| {
                                scope.execute_call(&mut detached, call)
                            }))
                            .unwrap_or_else(|_| {
                                Err(RuntimeError::EngineInvariant {
                                    context: "running a script's call",
                                    message: "the call's execution panicked".to_owned(),
                                })
                            });
                            let _ = done.send((executed, detached));
                        });
                    },
                    |(executed, detached)| {
                        detached.hand_back(host)?;
                        concluded(ledger, executed?)
                    },
                )
            })?;
        }
        None => {
            for call in admitted {
                let executed = ledger.scope().execute_call(host, call)?;
                concluded(ledger, executed)?;
            }
        }
    }
    Ok(Ok(waiting))
}

/// Take each recorded call that hasn't settled further, and return those that
/// still wait: a run paused on one goes on from it.
fn further_open_calls<A>(
    ledger: &mut CallLedger<'_, A>,
    host: &mut dyn Host,
) -> Result<Vec<Waiting>, RuntimeError>
where
    A: SkillAdapter,
{
    let open = ledger
        .recorded()?
        .iter()
        .filter(|call| !settled(&call.outcome))
        .map(|call| call.index)
        .collect::<Vec<_>>();
    let mut waiting = Vec::new();
    for index in open {
        if let TurnCallOutcome::Pending { waiting: requests } = ledger.further(host, index)?.outcome
        {
            waiting.push((index, requests));
        }
    }
    Ok(waiting)
}

fn waiting_on(
    step_id: &str,
    position: u32,
    waiting: Vec<runx_contracts::ResolutionRequest>,
) -> RuntimeError {
    match waiting.into_iter().next() {
        Some(request) => RuntimeError::ResolutionPending {
            step_id: step_id.to_owned(),
            reason: format!("call {position} of script step {step_id} waits on its request"),
            request: Box::new(request),
            child: None,
        },
        None => RuntimeError::EngineInvariant {
            context: "running a script call",
            message: format!("call {position} of {step_id} waits on nothing"),
        },
    }
}

/// The recorded call at `position`. A step's calls are recorded in position
/// order.
fn at(recorded: &[RecordedCall], position: u32) -> Option<&RecordedCall> {
    recorded
        .binary_search_by_key(&position, |call| call.index)
        .ok()
        .and_then(|found| recorded.get(found))
}

/// What `calls` settled to, in the order they settled, as the script's
/// promises take it.
fn settlements<'r, A>(
    ledger: &CallLedger<'_, A>,
    calls: impl IntoIterator<Item = &'r RecordedCall>,
) -> Result<Vec<ScriptSettlement>, RuntimeError>
where
    A: SkillAdapter,
{
    let mut settled = calls
        .into_iter()
        .filter(|call| settled(&call.outcome))
        .collect::<Vec<_>>();
    settled.sort_by_key(|call| call.order);
    settled
        .into_iter()
        .map(|call| {
            Ok(ScriptSettlement {
                position: call.index,
                outcome: settlement(ledger, call)?,
            })
        })
        .collect()
}

fn settled(outcome: &CallOutcome) -> bool {
    matches!(
        outcome,
        CallOutcome::Completed(_) | CallOutcome::Refused(_) | CallOutcome::Failed(_)
    )
}

/// How a settled call reaches the script: a result the ledger withholds, as
/// it withholds it from a turn, is refused, and the script never sees what
/// it read.
fn settlement<A>(
    ledger: &CallLedger<'_, A>,
    call: &RecordedCall,
) -> Result<ScriptCallOutcome, RuntimeError>
where
    A: SkillAdapter,
{
    Ok(match &call.outcome {
        outcome if ledger.withholds(outcome, &call.outputs) => rejected(
            ScriptRejection::Refused,
            format!(
                "call {} reads what's labelled local_only, and this step's host can't say its model runs on this machine",
                call.index
            ),
        ),
        CallOutcome::Completed(run) if run.outcome.succeeded() => ScriptCallOutcome::Resolved {
            value: serde_json::to_value(&run.contract)
                .map_err(|source| RuntimeError::json("delivering a call's result", source))?,
        },
        CallOutcome::Completed(run) => rejected(ScriptRejection::Failed, output_error(run)),
        CallOutcome::Failed(reason) => rejected(ScriptRejection::Failed, reason.clone()),
        CallOutcome::Refused(reason) => rejected(ScriptRejection::Refused, reason.clone()),
        CallOutcome::Pending(_) | CallOutcome::Claimed(_) | CallOutcome::Child(_) => {
            return Err(RuntimeError::EngineInvariant {
                context: "replaying a script",
                message: "an open call has no settlement to deliver".to_owned(),
            });
        }
    })
}

fn rejected(reason: ScriptRejection, message: String) -> ScriptCallOutcome {
    let code = match reason {
        ScriptRejection::Refused => "refused",
        ScriptRejection::Failed => "failed",
        ScriptRejection::Declined => "declined",
    };
    ScriptCallOutcome::Rejected {
        reason,
        code: code.to_owned(),
        message,
    }
}

/// Every call the pass made at a recorded position must be the call the
/// record holds there: the same target and the same input. A mismatch is a
/// failed recovery, and nothing acts on it.
fn matches_record(
    step_id: &str,
    calls: &[ScriptCall],
    recorded: &[RecordedCall],
) -> Result<(), RuntimeError> {
    let digest = crate::execution::run_record::input_digest;
    for call in calls {
        let Some(known) = at(recorded, call.position) else {
            continue;
        };
        let same = match ledger_call(&call.target) {
            Ok(Some((target, input))) => {
                known.target == target && digest(&known.input)? == digest(&input)?
            }
            Ok(None) | Err(_) => false,
        };
        if !same {
            return Err(diverged(
                step_id,
                call.position,
                "the script made another call there than its record holds",
            ));
        }
    }
    Ok(())
}

/// The first call the script made that the record doesn't show settled.
fn outstanding(calls: &[ScriptCall], recorded: &[RecordedCall]) -> Option<u32> {
    let settled = recorded
        .iter()
        .filter(|known| settled(&known.outcome))
        .map(|known| known.index)
        .collect::<BTreeSet<_>>();
    calls
        .iter()
        .filter(|call| !matches!(call.target, ScriptCallTarget::Now { .. }))
        .find(|call| !settled.contains(&call.position))
        .map(|call| call.position)
}

fn diverged(step_id: &str, position: u32, why: &str) -> RuntimeError {
    RuntimeError::ContinuationRejected {
        reason: format!(
            "script step {step_id} can't be recovered at call {position}: {why}, so nothing acts on it"
        ),
    }
}

/// A script's call as the ledger records it: what it names and its input.
/// `None` for `ctx.now()`, which isn't a call the ledger makes; an error for
/// a call the ledger can't name.
fn ledger_call(target: &ScriptCallTarget) -> Result<Option<(TurnTarget, JsonObject)>, String> {
    let object = |inputs: &serde_json::Value| {
        serde_json::from_value::<JsonObject>(inputs.clone())
            .map_err(|error| format!("a call's inputs are a JSON object: {error}"))
    };
    Ok(Some(match target {
        ScriptCallTarget::Skill { reference, inputs } => (
            TurnTarget::Skill {
                skill: reference.as_str().into(),
                method: None,
                bound: None,
            },
            object(inputs)?,
        ),
        ScriptCallTarget::Tool { name, inputs } => (
            TurnTarget::Tool {
                tool: name.as_str().into(),
            },
            object(inputs)?,
        ),
        ScriptCallTarget::Operation {
            operation,
            inputs,
            idempotency_key,
        } => {
            let (requirement, operation) = operation.rsplit_once('/').ok_or_else(|| {
                format!("ctx.operation names <connector>/<operation>, not {operation:?}")
            })?;
            let mut input = BTreeMap::from([("input".to_owned(), inputs.clone())]);
            if let Some(key) = idempotency_key {
                input.insert(
                    "idempotency_key".to_owned(),
                    serde_json::Value::String(key.clone()),
                );
            }
            (
                TurnTarget::Operation {
                    requirement: requirement.into(),
                    operation: operation.into(),
                },
                object(&serde_json::to_value(input).map_err(|error| error.to_string())?)?,
            )
        }
        ScriptCallTarget::Now { .. } => return Ok(None),
    }))
}
