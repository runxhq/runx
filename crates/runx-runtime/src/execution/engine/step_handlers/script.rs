//! Script steps. A script is deterministic code in the JavaScript worker that
//! drives the calls its step lists, and the step's run record is its history.
//! A package's script step runs a module of its package over its step's
//! `calls`; a turn's script call runs the source it gives as a script step at
//! the call's address, over the turn's own calls. Each drive of the step
//! passes the script from the beginning against the calls the record
//! settled, in the order they settled, and keeps it live from there: every
//! call it makes past the record is admitted, executed and recorded by the
//! kernel through the same ledger an agent turn's calls go through, and its
//! settlement is delivered to the script where it stands. A call's identity
//! is its position in the step. After a crash, the next drive of the step
//! replays the record and goes on from the first call it doesn't hold.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;

use runx_contracts::javascript_worker::{
    ScriptCall, ScriptCallOutcome, ScriptCallTarget, ScriptContext, ScriptEnding, ScriptReference,
    ScriptRejection, ScriptSettlement,
};
use runx_contracts::{ArtifactLabels, CallTarget, JsonObject, JsonValue, TurnCallOutcome};
use runx_parser::ScriptStep;

use super::super::super::run_record::{CallOutcome, RecordedCall};
use super::super::bounded::run_bounded;
use super::super::turn::{
    Admission, CallLedger, SCRIPT_MODULE, TurnScope, past_inline_bound, resolution_request,
};
use super::{
    RegularSkillSeal, RegularSkillStepOutput, StepHandlerCtx, StepReceiptLineage,
    build_ephemeral_step_output, build_step_output_projection, output_error,
    seal_regular_skill_step,
};
use crate::RuntimeError;
use crate::adapter::{InvocationOutput, SkillAdapter};
use crate::adapters::javascript::PreparedScript;
use crate::execution::engine::StepRun;
use crate::execution::output_projection::project_step_claim;
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
        given,
        reused: _,
        script: turn_script,
    } = request;
    let scope = TurnScope {
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
        answers_each_call: host.answers_each_call(),
        given,
    };
    let env = &runtime.options.env;
    // A package's script runs the modules its run admitted; a turn's runs
    // the source its call gives, over the turn's own calls.
    let (prepared, mut ledger) = match turn_script {
        Some(turn_script) => (
            runtime
                .javascript
                .prepare_source(SCRIPT_MODULE, &turn_script.source, env),
            CallLedger::of_turn_script(scope, turn_script),
        ),
        None => {
            let package = runtime.options.package(workflow_dir)?;
            (
                runtime.javascript.prepare_script(script, &package, env)?,
                CallLedger::of_step(scope),
            )
        }
    };
    // The run's max_concurrency, and the host's ceiling, narrow what the
    // step declares.
    let concurrency = usize::try_from(crate::kernel::authority::step_concurrency(
        runtime.options.run_authority.as_ref(),
        runtime.options.max_concurrency,
        script.concurrency,
    ))
    .unwrap_or(1);
    let ended = match drive(
        &mut ledger,
        host,
        &prepared,
        &inputs,
        (script.max_calls, concurrency),
    ) {
        Ok(ended) => ended,
        // A wait holds the step where it stands, and a stop or a fault is
        // the run's, not the script's.
        Err(error)
            if matches!(
                error,
                RuntimeError::ResolutionPending { .. } | RuntimeError::Stopped { .. }
            ) || error.is_fatal_step_fault() =>
        {
            return Err(error);
        }
        // The worker refused the script or failed under it, or it went
        // another way than its record: it goes no further, and its step
        // fails with the reason, sealed with every call it made.
        Err(error) => {
            if let Some((position, requests)) = drop_open_calls(&mut ledger, host)? {
                return Err(waiting_on(&step.id, position, requests));
            }
            Err(error.to_string())
        }
    };
    // A turn's script declares no contract: what it returns, an object, is
    // its call's result.
    let ended = match turn_script {
        Some(_) => ended.and_then(|output| {
            if output.is_object() {
                Ok(output)
            } else {
                Err("a turn's script returns an object, its call's result".to_owned())
            }
        }),
        None => ended,
    };
    let recorded = ledger.recorded()?;
    // What the script made is made from what its calls returned and what it
    // read.
    let read =
        ArtifactLabels::derived_from(recorded.iter().filter_map(|call| match &call.outcome {
            CallOutcome::Completed(run) => Some(&run.labels),
            CallOutcome::Read(artifact) => Some(&artifact.labels),
            _ => None,
        }));
    let calls = recorded
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
    let projection = match turn_script {
        Some(_) if output.succeeded() => {
            project_step_claim(output.value.as_object().cloned().unwrap_or_default(), None)
        }
        _ => build_step_output_projection(
            step,
            &output,
            None,
            None,
            workflow_dir,
            &super::verdict::Evaluating::Step(&|| {
                super::verdict::evaluator(runtime, workflow_dir, step)
            }),
        )?,
    };
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
            ending: None,
            refusal: None,
            declined: None,
        },
        RegularSkillStepOutput {
            output,
            projection,
            ephemeral_contract,
            // The step's receipt names each call the script made, in order.
            receipt_lineage: StepReceiptLineage::of_turn(&calls),
            read,
        },
    )
}

/// Drive the script `prepared` on `inputs` over `ledger`, the calls its
/// record holds, making each call past the record, at most `max_calls` of
/// them and `concurrency` at once: what it returned, or why it failed. A
/// script whose calls wait holds its step on the first of them.
fn drive<A>(
    ledger: &mut CallLedger<'_, A>,
    host: &mut dyn Host,
    prepared: &PreparedScript,
    inputs: &JsonObject,
    (max_calls, concurrency): (u32, usize),
) -> Result<Result<serde_json::Value, String>, RuntimeError>
where
    A: SkillAdapter,
{
    let scope = ledger.scope();
    let (runtime, step_id) = (scope.runtime, scope.step_id);
    // The script passes from the beginning against its record once per drive
    // of the step, and from there stays live: each call's settlement goes on
    // from where the script stands.
    // What a `ctx.now()` past the record reads, and what a wait is due by:
    // the run's clock, as each pass or settlement reaches the script.
    let now = || {
        runtime
            .options
            .clock
            .now_millis()
            .ok_or_else(|| RuntimeError::StepFailed {
                step_id: step_id.to_owned(),
                reason: "the run's clock reads no time for ctx.now()".to_owned(),
            })
    };
    // An input past the inline bound reaches the script by reference, an
    // array's item by item. It's given before any recorded call goes
    // further, since a call may name what it was given.
    let (given, references) = ledger.give(inputs)?;
    // Calls that wait on a person, a child or a time, by position. A waiting
    // call never stops the others: the step holds only once none can go
    // further. A wait this drive reaches at or after its `until` settles.
    let mut waiting = further_open_calls(ledger, host, now()?)?;
    // A pass that executes apart from the driver goes on only in the child
    // run its act is in: the script itself goes no further, and the step
    // stands as its record has it.
    if host.executes() {
        let Some((position, requests)) = waiting.into_iter().next() else {
            return Err(RuntimeError::EngineInvariant {
                context: "executing apart from the driver",
                message: format!("script step {step_id} has no call open"),
            });
        };
        return Err(waiting_on(step_id, position, requests));
    }
    let mut settled = settlements(ledger, ledger.recorded()?)?;
    settled.extend(clock_reads(ledger.clock()?));
    let (mut live, mut passed) =
        runtime
            .javascript
            .start_script(prepared, (&given, references), settled, now()?)?;
    // Every call the script has made, in position order, and how many of
    // them are calls rather than reads of the clock.
    let mut made = Vec::new();
    let mut calls = 0_usize;
    let max = usize::try_from(max_calls).unwrap_or(usize::MAX);
    let ended = loop {
        matches_record(ledger, &passed.calls)?;
        // A clock read past the record is recorded at its position as the
        // script made it, before anything else the pass did.
        let read = passed
            .calls
            .iter()
            .filter_map(|call| match call.target {
                ScriptCallTarget::Now { millis } => Some((call.position, millis)),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (position, millis) in read {
            if !ledger.clock()?.contains_key(&position) {
                ledger.record_clock(position, millis)?;
            }
        }
        // Only a call this report holds can be new: every earlier one was made
        // before the script was given its settlement.
        let new = {
            let recorded = ledger.recorded()?;
            passed
                .calls
                .iter()
                .filter(|call| {
                    !matches!(call.target, ScriptCallTarget::Now { .. })
                        && at(recorded, call.position).is_none()
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        // A wait is the ledger's own: a request the run waits on until a
        // later drive, at or after its `until`, settles it.
        let (waits, new): (Vec<_>, Vec<_>) = new
            .into_iter()
            .partition(|call| matches!(call.target, ScriptCallTarget::Wait { .. }));
        for call in &waits {
            if let ScriptCallTarget::Wait { until } = call.target {
                waiting.push((call.position, vec![ledger.wait(call.position, until)?]));
            }
        }
        // A read of a reference is the ledger's own too, settled at once.
        let (reads, new): (Vec<_>, Vec<_>) = new
            .into_iter()
            .partition(|call| matches!(call.target, ScriptCallTarget::Read { .. }));
        for call in &reads {
            if let ScriptCallTarget::Read { digest } = &call.target {
                ledger.read(call.position, digest)?;
            }
        }
        calls += passed
            .calls
            .iter()
            .filter(|call| !matches!(call.target, ScriptCallTarget::Now { .. }))
            .count();
        made.extend(passed.calls);
        match passed.ending {
            ScriptEnding::Diverged { position } => {
                return Err(diverged(
                    step_id,
                    position,
                    "the record settles a call the script didn't make",
                ));
            }
            ScriptEnding::Threw { message } => break Err(message),
            // The adapter reads an output past what an answer carries in its
            // parts, and gives it whole.
            ScriptEnding::ReturnedInParts { .. } => {
                return Err(RuntimeError::EngineInvariant {
                    context: "a script's output",
                    message: format!(
                        "script step {step_id} was given its output's size, not the output"
                    ),
                });
            }
            ScriptEnding::Returned { output } => {
                // The output counts only once every call the script started
                // has settled.
                if let Some(position) = outstanding(&made, ledger.recorded()?) {
                    break Err(format!(
                        "the script returned with call {position} outstanding; a script awaits every call it starts"
                    ));
                }
                // A reference it returns is its field's value, which no copy
                // through the script carries.
                break referenced(ledger, output)?;
            }
            ScriptEnding::Waiting => {
                if new.is_empty() && waits.is_empty() && reads.is_empty() {
                    if let Some((position, requests)) = waiting.into_iter().next() {
                        return Err(waiting_on(step_id, position, requests));
                    }
                    break Err(
                        "the script waits on something that is not one of its calls".to_owned()
                    );
                }
                if calls > max {
                    break Err(format!("the script reached its max_calls of {max_calls}"));
                }
                let positions = reads
                    .iter()
                    .chain(&new)
                    .map(|call| call.position)
                    .collect::<Vec<_>>();
                match make_calls(ledger, host, new, concurrency)? {
                    Err(refusal) => break Err(refusal),
                    Ok(still) => waiting.extend(still),
                }
                let fresh = {
                    let recorded = ledger.recorded()?;
                    settlements(
                        ledger,
                        positions
                            .iter()
                            .filter_map(|position| at(recorded, *position)),
                    )?
                };
                if fresh.is_empty() {
                    // Every new call waits, so nothing can go further.
                    if let Some((position, requests)) = waiting.into_iter().next() {
                        return Err(waiting_on(step_id, position, requests));
                    }
                    break Err("the script's calls settled to nothing".to_owned());
                }
                passed = live.settle(fresh, now()?)?;
            }
        }
    };
    drop(live);
    // A script that ended without its output, by a throw, a rejection it
    // never handled, an early return or a limit, leaves nothing it started
    // running: what it left open is dropped, and an effect whose end isn't
    // known holds the step until it settles. Only then does the step fail,
    // with the script's own reason, which a replay reaches again.
    if ended.is_err()
        && let Some((position, requests)) = drop_open_calls(ledger, host)?
    {
        return Err(waiting_on(step_id, position, requests));
    }
    Ok(ended)
}

/// A call that waits, at its position, with the requests it waits on.
type Waiting = (u32, Vec<runx_contracts::ResolutionRequest>);

/// Make each new call: admitted in position order under the driver, then run
/// at most `concurrency` at once, apart from the driver where its host can
/// detach a host for each, and recorded under the driver in the order they
/// finish. Return those that wait on a person or a child; a waiting call
/// doesn't stop the next.
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
    // Every call the pass made names something the ledger can make before
    // any is admitted, so one it can't leaves none of them claimed.
    let mut made = Vec::with_capacity(new.len());
    for call in new {
        match ledger_call(ledger, &call.target) {
            Ok(Some((target, input, context))) => made.push((call, target, input, context)),
            Ok(None) => {
                return Err(RuntimeError::EngineInvariant {
                    context: "making a script's calls",
                    message: "a clock read is recorded, never made".to_owned(),
                });
            }
            Err(refusal) => return Ok(Err(refusal)),
        }
    }
    let mut admitted = Vec::new();
    for (call, target, input, context) in made {
        match ledger.admit(
            host,
            call.position,
            target,
            input,
            context,
            reuse(&call.target),
        )? {
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
            // A call on its own thread still stops as the work it's part of
            // stops.
            let working = crate::interrupt::working();
            let working = &working;
            // The calls dispatched at once are in flight before any of them
            // runs, so a cancel finds each where the scheduler put it; a
            // later one is dispatched as a place frees for it.
            for call in admitted.iter().take(concurrency) {
                call.dispatch();
            }
            std::thread::scope(|threads| {
                run_bounded(
                    admitted.into_iter().zip(hosts),
                    concurrency,
                    |(call, mut detached), done| {
                        call.dispatch();
                        threads.spawn(move || {
                            let executed = std::panic::catch_unwind(AssertUnwindSafe(|| {
                                crate::interrupt::while_working_under(working, || {
                                    scope.execute_call(&mut detached, call)
                                })
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

/// Drop each call the script left open, and return the first that still
/// waits on its effect's end, with what it waits on.
fn drop_open_calls<A>(
    ledger: &mut CallLedger<'_, A>,
    host: &mut dyn Host,
) -> Result<Option<Waiting>, RuntimeError>
where
    A: SkillAdapter,
{
    let open = ledger
        .recorded()?
        .iter()
        .filter(|call| !settled(&call.outcome))
        .map(|call| call.index)
        .collect::<Vec<_>>();
    let mut held = None;
    for index in open {
        let waiting = ledger.drop_open(host, index)?;
        if held.is_none() && !waiting.is_empty() {
            held = Some((index, waiting));
        }
    }
    Ok(held)
}

/// Take each recorded call that hasn't settled further, and return those that
/// still wait: a run paused on one goes on from it.
fn further_open_calls<A>(
    ledger: &mut CallLedger<'_, A>,
    host: &mut dyn Host,
    now: i64,
) -> Result<Vec<Waiting>, RuntimeError>
where
    A: SkillAdapter,
{
    let open = ledger
        .recorded()?
        .iter()
        .filter(|call| !settled(&call.outcome))
        .map(|call| (call.index, call.target.clone(), call.outcome.clone()))
        .collect::<Vec<_>>();
    let mut waiting = Vec::new();
    let step = ledger.step_address();
    for (index, target, outcome) in open {
        // A call executing apart from this pass is its own.
        if host.executing(&step, index) {
            continue;
        }
        // A pass that executes apart from the driver takes further only the
        // call its act is under, and reports each as its record has it.
        if host.executes() {
            let standing = ledger.standing(index)?;
            if host.leads_to(&ledger.address_of(index)) {
                ledger.further(host, index)?;
            }
            if let TurnCallOutcome::Pending { waiting: requests } = standing.outcome {
                waiting.push((index, requests));
            }
            continue;
        }
        match (target, outcome) {
            // A wait settles once the run is driven at or after its until,
            // at the time this drive reads; before then it waits as it did.
            (CallTarget::Wait { until }, _) if now >= until => {
                ledger.settle_wait(index, until, now)?;
            }
            (CallTarget::Wait { .. }, CallOutcome::Pending(request)) => {
                waiting.push((index, vec![resolution_request(&request)?]));
            }
            _ => {
                if let TurnCallOutcome::Pending { waiting: requests } =
                    ledger.further(host, index)?.outcome
                {
                    waiting.push((index, requests));
                }
            }
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

/// The script's output, each reference it holds, at any depth, taking the
/// content of the artifact it names: one a call of the step returned, or one
/// of the step's inputs its script was given. A reference that names neither
/// fails the step, naming where the output holds it.
fn referenced<A>(
    ledger: &CallLedger<'_, A>,
    mut output: serde_json::Value,
) -> Result<Result<serde_json::Value, String>, RuntimeError>
where
    A: SkillAdapter,
{
    Ok(resolve_references(ledger, &mut output, "output")?.map(|()| output))
}

/// Replace each reference `value` holds, `at` naming where, with its
/// content, or say which names no reference of the step.
fn resolve_references<A>(
    ledger: &CallLedger<'_, A>,
    value: &mut serde_json::Value,
    at: &str,
) -> Result<Result<(), String>, RuntimeError>
where
    A: SkillAdapter,
{
    let reference = value
        .as_object()
        .filter(|held| {
            held.get("schema").and_then(serde_json::Value::as_str)
                == Some(runx_contracts::ARTIFACT_SCHEMA)
        })
        .and_then(|held| held.get("digest"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    if let Some(digest) = reference {
        return match ledger.returned(&digest)? {
            Some((_, content)) => {
                *value = serde_json::to_value(content)
                    .map_err(|source| RuntimeError::json("reading a returned reference", source))?;
                Ok(Ok(()))
            }
            None => Ok(Err(format!(
                "{at}: {digest} names no artifact a call of the step returned"
            ))),
        };
    }
    match value {
        serde_json::Value::Object(fields) => {
            for (field, held) in fields {
                let at = match at {
                    "output" => format!("output {field}"),
                    _ => format!("{at}.{field}"),
                };
                if let Err(refused) = resolve_references(ledger, held, &at)? {
                    return Ok(Err(refused));
                }
            }
        }
        serde_json::Value::Array(items) => {
            for (index, held) in items.iter_mut().enumerate() {
                if let Err(refused) = resolve_references(ledger, held, &format!("{at}[{index}]"))? {
                    return Ok(Err(refused));
                }
            }
        }
        _ => {}
    }
    Ok(Ok(()))
}

/// What the script read of the clock, by position, as its replay takes it.
fn clock_reads(clock: &BTreeMap<u32, i64>) -> impl Iterator<Item = ScriptSettlement> + '_ {
    clock.iter().map(|(position, millis)| ScriptSettlement {
        position: *position,
        outcome: ScriptCallOutcome::Now { millis: *millis },
    })
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
        CallOutcome::Completed(_)
            | CallOutcome::Refused(_)
            | CallOutcome::Declined(_)
            | CallOutcome::Failed(_)
            | CallOutcome::RetryDeclined
            | CallOutcome::Cancelled
            | CallOutcome::Waited(_)
            | CallOutcome::Read(_)
            | CallOutcome::TooLarge { .. }
    )
}

/// What `call` settled to, as the script's promise takes it. A result the
/// ledger withholds, as it withholds it from a turn, is refused, and the
/// script never sees what it read. A packet output reaches the script as its
/// data, as a `$steps` reference reads it in a workflow; its envelope is how
/// the record keeps it. A field of its result past the bound its record holds
/// reaches the script as a reference to the artifact the call recorded it
/// in, which holds what the field would have held inline.
fn settlement<A>(
    ledger: &CallLedger<'_, A>,
    call: &RecordedCall,
) -> Result<ScriptCallOutcome, RuntimeError>
where
    A: SkillAdapter,
{
    let delivering = |source| RuntimeError::json("delivering a call's result", source);
    Ok(match &call.outcome {
        outcome if ledger.withholds(outcome) => rejected(
            ScriptRejection::Refused,
            format!(
                "call {} reads what's labelled local_only, and this step's host can't say its model runs on this machine",
                call.index
            ),
        ),
        CallOutcome::Completed(run) if run.outcome.succeeded() => {
            let mut value = JsonObject::new();
            let mut references = Vec::new();
            for field in run.contract.keys() {
                let held = run
                    .output(field)
                    .ok_or_else(|| RuntimeError::EngineInvariant {
                        context: "delivering a call's result",
                        message: format!("packet output {field} holds no data"),
                    })?;
                if !past_inline_bound(held, call.inline_bound)? {
                    value.insert(field.clone(), held.clone());
                    continue;
                }
                let artifact = call
                    .outputs
                    .iter()
                    .find(|part| part.output.as_str() == field)
                    .map(|part| part.artifact.clone())
                    .ok_or_else(|| RuntimeError::EngineInvariant {
                        context: "delivering a call's result",
                        message: format!(
                            "field {field} is past the inline bound and has no artifact"
                        ),
                    })?;
                references.push(ScriptReference {
                    field: field.clone(),
                    artifact,
                });
            }
            ScriptCallOutcome::Resolved {
                value: serde_json::to_value(&value).map_err(delivering)?,
                references,
            }
        }
        CallOutcome::Read(artifact) => ScriptCallOutcome::Resolved {
            value: serde_json::to_value(ledger.content(artifact)?).map_err(delivering)?,
            references: Vec::new(),
        },
        CallOutcome::TooLarge { size_bytes, bound } => rejected(
            ScriptRejection::TooLarge,
            format!(
                "the artifact's size_bytes is {size_bytes}, past the worker's input bound of {bound}; give it to a call or return it by reference"
            ),
        ),
        // A child run a person declined is declined, as any call is.
        CallOutcome::Completed(run)
            if run.receipt.seal.outcome == runx_contracts::RunOutcome::Declined =>
        {
            rejected(
                ScriptRejection::Declined,
                run.declined.clone().unwrap_or_else(|| output_error(run)),
            )
        }
        CallOutcome::Completed(run) => rejected(ScriptRejection::Failed, output_error(run)),
        CallOutcome::Failed(reason) => rejected(ScriptRejection::Failed, reason.clone()),
        // A person left it failed: the script catches it by its code.
        CallOutcome::RetryDeclined => ScriptCallOutcome::Rejected {
            reason: ScriptRejection::Failed,
            code: crate::execution::run_record::RETRY_DECLINED.to_owned(),
            message: "a person answered its retry {\"retry\": false}, so it wasn't run again"
                .to_owned(),
        },
        CallOutcome::Refused(reason) => rejected(ScriptRejection::Refused, reason.clone()),
        CallOutcome::Declined(reason) => rejected(ScriptRejection::Declined, reason.clone()),
        CallOutcome::Cancelled => rejected(
            ScriptRejection::Cancelled,
            "its host cancelled the call before it settled".to_owned(),
        ),
        // A wait returns the time it settled.
        CallOutcome::Waited(at) => ScriptCallOutcome::Resolved {
            value: serde_json::Value::from(*at),
            references: Vec::new(),
        },
        CallOutcome::Pending(_)
        | CallOutcome::Claimed(_)
        | CallOutcome::Executing
        | CallOutcome::Child(_) => {
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
        ScriptRejection::Cancelled => "cancelled",
        ScriptRejection::TooLarge => "too_large",
    };
    ScriptCallOutcome::Rejected {
        reason,
        code: code.to_owned(),
        message,
    }
}

/// Every call the pass made at a recorded position must be the call the
/// record holds there: the same target and the same input, or a clock read
/// of the same time. A mismatch is a failed recovery, and nothing acts on it.
fn matches_record<A>(ledger: &CallLedger<'_, A>, calls: &[ScriptCall]) -> Result<(), RuntimeError>
where
    A: SkillAdapter,
{
    let step_id = ledger.scope().step_id;
    let (recorded, clock) = (ledger.recorded()?, ledger.clock()?);
    let digest = crate::execution::run_record::call_input_digest;
    for call in calls {
        let read = clock.get(&call.position);
        match (&call.target, read) {
            (ScriptCallTarget::Now { millis }, Some(known)) if millis == known => continue,
            (ScriptCallTarget::Now { .. }, Some(_)) => {
                return Err(diverged(
                    step_id,
                    call.position,
                    "the script read another time there than its record holds",
                ));
            }
            (_, Some(_)) => {
                return Err(diverged(
                    step_id,
                    call.position,
                    "the script made a call where its record holds a clock read",
                ));
            }
            (_, None) => {}
        }
        let Some(known) = at(recorded, call.position) else {
            continue;
        };
        let same = match ledger_call(ledger, &call.target) {
            Ok(Some((target, input, context))) => {
                known.target == target
                    && digest(&known.input, &known.context)? == digest(&input, &context)?
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

/// What a skill call asks of reuse. Its ledger answers it as it admits the
/// call, in place of this answer of no candidate.
fn reuse(target: &ScriptCallTarget) -> Option<runx_contracts::RunCallReuse> {
    let ScriptCallTarget::Skill {
        reuse: Some(ask), ..
    } = target
    else {
        return None;
    };
    Some(runx_contracts::RunCallReuse {
        max_age_seconds: ask.max_age_seconds,
        model: ask.model.as_deref().map(Into::into),
        decision: runx_contracts::ReuseDecision::Recomputed {
            reason: runx_contracts::RecomputeReason::NoCandidate,
        },
    })
}

fn diverged(step_id: &str, position: u32, why: &str) -> RuntimeError {
    RuntimeError::ContinuationRejected {
        reason: format!(
            "script step {step_id} can't be recovered at call {position}: {why}, so nothing acts on it"
        ),
    }
}

/// A script's call as the ledger records it: what it names, its input, and
/// the context a skill call gives its child. `None` for `ctx.now()`, which
/// isn't a call the ledger makes; an error for a call the ledger can't name.
fn ledger_call<A>(
    ledger: &CallLedger<'_, A>,
    target: &ScriptCallTarget,
) -> Result<Option<(CallTarget, JsonObject, Vec<ScriptContext>)>, String>
where
    A: SkillAdapter,
{
    let object = |inputs: &serde_json::Value| {
        serde_json::from_value::<JsonObject>(inputs.clone())
            .map_err(|error| format!("a call's inputs are a JSON object: {error}"))
    };
    Ok(Some(match target {
        ScriptCallTarget::Skill {
            reference,
            inputs,
            context,
            ..
        } => (
            // A method a turn's load bound is called as the load bound it.
            ledger
                .as_offered(CallTarget::Skill {
                    skill: reference.as_str().into(),
                    method: None,
                    bound: None,
                })
                .map_err(|error| error.to_string())?,
            object(inputs)?,
            context.clone(),
        ),
        ScriptCallTarget::Tool { name, inputs } => (
            CallTarget::Tool {
                tool: name.as_str().into(),
            },
            object(inputs)?,
            Vec::new(),
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
                CallTarget::Operation {
                    requirement: requirement.into(),
                    operation: operation.into(),
                },
                object(&serde_json::to_value(input).map_err(|error| error.to_string())?)?,
                Vec::new(),
            )
        }
        ScriptCallTarget::Wait { until } => (
            CallTarget::Wait { until: *until },
            JsonObject::new(),
            Vec::new(),
        ),
        ScriptCallTarget::Read { digest } => (
            CallTarget::Read {
                digest: digest.as_str().into(),
            },
            JsonObject::new(),
            Vec::new(),
        ),
        ScriptCallTarget::Now { .. } => return Ok(None),
    }))
}
