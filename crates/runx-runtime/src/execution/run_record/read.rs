//! One run as a host reads it (`runx.run_read.v1`): its summary, what it waits
//! on, and its record as events after a cursor.
//!
//! A run and each of its child runs is a stream of its own, and the store
//! orders every commit, so a run's events come in the order its tree
//! committed them. The cursor holds the last commit returned, which is what
//! makes a read after it skip and repeat nothing, including in child runs
//! that began after the earlier read, and what makes a read cost what the
//! tree committed since, never what it holds. The summary and what the run
//! waits on are read after the events, so they are never older than the last
//! event returned.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ArtifactLabels, CallRead, CallReadEntry, CallReadSchema, CallTarget, JsonObject, JsonValue,
    LoadedPackage, ResolutionRequest, ResolutionResponseActor, RunCallOutcome, RunEventFact,
    RunEventRecord, RunHold, RunRead, RunReadSchema, RunReadSummary, WaitedRequest,
};
use serde::{Deserialize, Serialize};

use super::{
    AnswerEvent, CallOutcomeEvent, HoldEvent, RunEvent, RunRecordError, call_input_digest, driven,
    standing, standing_event, validate_run_id, waiting_requests,
};
use crate::execution::engine::StepOutcome;
use crate::kernel::store::{KernelStore, Recorded};
use crate::paths::RunxPaths;

/// The most events one read returns.
const PAGE: usize = 500;

/// Read run `run_id` of the workspace at `cwd`, from the events after
/// `after`, a cursor an earlier read returned.
pub fn read_run(
    env: &BTreeMap<String, String>,
    cwd: &std::path::Path,
    run_id: &str,
    after: Option<&str>,
) -> Result<RunRead, RunRecordError> {
    let paths = RunxPaths::resolve(env, cwd);
    let store = KernelStore::for_project(&paths)?;
    let run_id = validate_run_id(run_id)?;
    let unknown = || RunRecordError::UnknownRun {
        run_id: run_id.to_owned(),
    };
    let started = match store.first::<RunEvent>(run_id)?.map(|first| first.event) {
        Some(RunEvent::Started(started)) => started,
        _ => return Err(unknown()),
    };
    let after = match after {
        Some(cursor) => Cursor::decode(cursor, run_id)?.after,
        None => 0,
    };
    // A cursor names a commit this tree has reached.
    if after > store.tree_sequence::<RunEvent>(run_id)? {
        return Err(RunRecordError::InvalidCursor);
    }
    let mut page = store.tree_after::<RunEvent>(run_id, after, PAGE + 1)?;
    let more = page.len() > PAGE;
    page.truncate(PAGE);
    let reached = page.last().map_or(after, |event| event.sequence);
    let events = page
        .into_iter()
        .filter_map(|event| project(&store, &paths, &event.key, event.recorded).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    // An admitted answer updates the run but leaves it where it waits.
    let updated_at = store
        .latest::<RunEvent>(run_id)?
        .ok_or_else(unknown)?
        .committed_at;
    let latest = standing_event(&store, run_id)?.ok_or_else(unknown)?;
    let (state, outcome, receipt_id) = standing(&latest.event);
    let waiting = waiting_requests(&store, run_id)?
        .into_iter()
        .map(|pending| {
            let request: ResolutionRequest = serde_json::to_value(&pending.value)
                .and_then(serde_json::from_value)
                .map_err(|error| invalid(run_id, format!("a waited request: {error}")))?;
            Ok(WaitedRequest {
                address: pending.id.into(),
                request_digest: pending.digest.into(),
                request,
            })
        })
        .collect::<Result<Vec<_>, RunRecordError>>()?;
    Ok(RunRead {
        schema: RunReadSchema::V1,
        run: RunReadSummary {
            run_id: run_id.into(),
            skill: started.skill.into(),
            method: started.method.into(),
            state,
            outcome,
            package_digest: started.package_digest.into(),
            closure_digest: started.closure_digest.into(),
            driver: started.driver,
            receipt_id: receipt_id.map(Into::into),
            why: why(&store, run_id)?,
            driven: driven(&paths, run_id)?,
            created_at: started.created_at.into(),
            updated_at: updated_at.into(),
        },
        waiting,
        events,
        cursor: Cursor {
            run: run_id.to_owned(),
            after: reached,
        }
        .encode(run_id)?
        .into(),
        more,
    })
}

/// What one page of a script's calls holds at most, as `call.read` writes
/// them: a script's calls, however many, are read a page at a time, so no
/// read is past what `artifact.open` opens.
const CALL_READ_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// Call `index` of step `step_id` in run `run_id` of the workspace at `cwd`,
/// read whole: as the step's latest attempt holding it last recorded it,
/// with the request of the turn that made it and, for a turn's script call,
/// each call its script made, at its position, and the labels of everything
/// it holds.
pub fn read_call(
    env: &BTreeMap<String, String>,
    cwd: &std::path::Path,
    run_id: &str,
    step_id: &str,
    index: u32,
    from: u32,
) -> Result<(CallRead, ArtifactLabels), RunRecordError> {
    let paths = RunxPaths::resolve(env, cwd);
    let store = KernelStore::for_project(&paths)?;
    let run_id = validate_run_id(run_id)?;
    let address = format!("{step_id}/{index}");
    let unknown = || RunRecordError::UnknownCall {
        run_id: run_id.to_owned(),
        call: address.clone(),
    };
    if !matches!(
        store.first::<RunEvent>(run_id)?.map(|first| first.event),
        Some(RunEvent::Started(_))
    ) {
        return Err(RunRecordError::UnknownRun {
            run_id: run_id.to_owned(),
        });
    }
    // The step's calls are on its attempts' streams, and a script call's
    // under it; a call's latest event is how it stands. Only those streams
    // are read, whatever else the run holds.
    let mut calls = calls_on(&store, &format!("{run_id}/{step_id}@"))?;
    let made = calls_on(&store, &format!("{run_id}/{address}@"))?;
    let attempt = calls
        .keys()
        .filter(|(_, at)| *at == index)
        .map(|(attempt, _)| *attempt)
        .max()
        .ok_or_else(unknown)?;
    let call = calls.remove(&(attempt, index)).ok_or_else(unknown)?;
    let turn = call
        .turn
        .as_ref()
        .map(|turn| store.blob_json::<ResolutionRequest>(&turn.request))
        .transpose()?;
    let mut labels = Vec::new();
    let mut entry =
        |address: String, event: super::CallEvent| -> Result<CallReadEntry, RunRecordError> {
            labels.push(call_labels(&store, &event.outcome)?);
            labels.extend(event.given);
            Ok(CallReadEntry {
                address: address.into(),
                input: store.blob_json(&event.input)?,
                target: event.target,
                outcome: call_outcome(&store, event.outcome)?,
            })
        };
    // A page of the calls under it, from `from`, within a bound however
    // many its script made; `next` is where the rest are read from.
    let mut page = Vec::new();
    let mut bytes = 0_usize;
    let mut next = None;
    for ((_, position), event) in made
        .into_iter()
        .filter(|((at, position), _)| *at == attempt && *position >= from)
    {
        let made = entry(format!("{address}/{position}"), event)?;
        let size = serde_json::to_vec(&made).map_or(usize::MAX, |encoded| encoded.len());
        if !page.is_empty() && bytes.saturating_add(size) > CALL_READ_PAGE_BYTES {
            next = Some(position);
            break;
        }
        bytes = bytes.saturating_add(size);
        page.push(made);
    }
    let call = entry(address.clone(), call)?;
    // Its input is what its maker wrote from all it had seen.
    labels.extend(seen_before(&store, run_id, step_id, index, attempt)?);
    Ok((
        CallRead {
            schema: CallReadSchema::V1,
            run_id: run_id.into(),
            attempt,
            call,
            turn,
            calls: page,
            next,
        },
        ArtifactLabels::derived_from(labels.iter()),
    ))
}

/// The calls on the attempts' streams `prefix` names, `<run>/<step>@`, by
/// attempt and index: each as its latest event has it. Only those streams
/// are read, whatever else the run holds.
fn calls_on(
    store: &KernelStore,
    prefix: &str,
) -> Result<BTreeMap<(u32, u32), super::CallEvent>, RunRecordError> {
    let mut calls = BTreeMap::new();
    for head in store.heads_from::<RunEvent>(prefix)? {
        if head
            .key
            .strip_prefix(prefix)
            .and_then(|attempt| attempt.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        for recorded in store.read::<RunEvent>(&head.key)? {
            if let RunEvent::Call(call) = recorded.event {
                calls.insert((call.attempt, call.index), *call);
            }
        }
    }
    Ok(calls)
}

/// The labels of all call `index` of step `step_id`'s maker had seen when it
/// made it: each result returned before it on its stream, what its turn was
/// given, and, for a call a script made, all the script's own maker had seen
/// when it made the script call, up to the turn.
fn seen_before(
    store: &KernelStore,
    run_id: &str,
    step_id: &str,
    index: u32,
    attempt: u32,
) -> Result<Vec<ArtifactLabels>, RunRecordError> {
    let calls = calls_on(store, &format!("{run_id}/{step_id}@"))?;
    let mut labels = Vec::new();
    for ((at, position), earlier) in &calls {
        if *at == attempt && *position < index {
            labels.push(call_labels(store, &earlier.outcome)?);
        }
    }
    // What the turn was given is labelled as its context entries are.
    if let Some(turn) = calls
        .get(&(attempt, index))
        .and_then(|call| call.turn.as_ref())
        && let ResolutionRequest::Turn { invocation, .. } =
            store.blob_json::<ResolutionRequest>(&turn.request)?
    {
        let envelope = &invocation.envelope;
        labels.extend(
            envelope
                .current_context
                .iter()
                .chain(&envelope.historical_context)
                .map(|entry| ArtifactLabels {
                    sharing: entry.labels.sharing,
                }),
        );
    }
    // A script's call: the script was given what its maker had seen.
    if let Some((script_step, script_index)) = step_id
        .rsplit_once('/')
        .and_then(|(step, index)| Some((step, index.parse::<u32>().ok()?)))
        .filter(|(step, _)| !step.is_empty())
    {
        labels.extend(seen_before(
            store,
            run_id,
            script_step,
            script_index,
            attempt,
        )?);
    }
    Ok(labels)
}

/// Where what a recorded call returned may be shared: its record's labels
/// and its outputs' artifacts'.
fn call_labels(
    store: &KernelStore,
    outcome: &CallOutcomeEvent,
) -> Result<ArtifactLabels, RunRecordError> {
    #[derive(Deserialize)]
    struct Labelled {
        labels: Option<ArtifactLabels>,
    }
    Ok(match outcome {
        CallOutcomeEvent::Completed {
            record, outputs, ..
        } => {
            let Labelled { labels } = store.blob_json(record)?;
            ArtifactLabels::derived_from(
                labels
                    .iter()
                    .chain(outputs.iter().map(|part| &part.artifact.labels)),
            )
        }
        CallOutcomeEvent::Read { artifact } => artifact.labels,
        _ => ArtifactLabels::CONFIGURED_MODEL,
    })
}

/// The read position: the run, and the last commit of its tree returned.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    run: String,
    after: u64,
}

impl Cursor {
    fn encode(&self, run_id: &str) -> Result<String, RunRecordError> {
        serde_json::to_vec(self)
            .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
            .map_err(|error| invalid(run_id, format!("encoding its cursor: {error}")))
    }

    fn decode(cursor: &str, run_id: &str) -> Result<Self, RunRecordError> {
        let decoded: Self = URL_SAFE_NO_PAD
            .decode(cursor.trim())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or(RunRecordError::InvalidCursor)?;
        if decoded.run != run_id {
            return Err(RunRecordError::CursorRunMismatch {
                run_id: run_id.to_owned(),
                cursor_run: decoded.run,
            });
        }
        Ok(decoded)
    }
}

/// The part of a step record a read reports.
#[derive(Deserialize)]
struct StepStatus {
    outcome: StepOutcome,
}

/// The part of a step's or a call's record a read reports beside its
/// outcome: its output, every field the record holds.
#[derive(Deserialize)]
struct CallOutput {
    contract: JsonObject,
}

/// The part of a step's record a read reports beside its outcome: its
/// output, and what its turn used, whether it answered or failed.
#[derive(Deserialize)]
struct StepOutput {
    contract: JsonObject,
    #[serde(default)]
    usage: Option<runx_contracts::TurnUsage>,
}

/// The part of a receipt a read reports.
#[derive(Deserialize)]
struct ReceiptId {
    id: String,
}

/// How a host reads one event of a run's tree; none for the answers a call
/// used, which the run records beside the call that the read shows.
fn project(
    store: &KernelStore,
    paths: &RunxPaths,
    stream: &str,
    recorded: Recorded<RunEvent>,
) -> Result<Option<RunEventRecord>, RunRecordError> {
    let event = match recorded.event {
        RunEvent::Started(started) => RunEventFact::Started {
            skill: started.skill.into(),
            method: started.method.into(),
            implementation: started.implementation,
            ran_on: started.ran_on,
        },
        RunEvent::ChildStarted(started) => RunEventFact::ChildStarted {
            parent: started.parent.into(),
            step_id: started.step_id.into(),
            skill: started.skill.into(),
            implementation: started.implementation,
        },
        RunEvent::EvaluationStarted(_) => {
            return Err(invalid(stream, "an evaluation inside a run".to_owned()));
        }
        RunEvent::Step(event) => {
            let (outcome, receipt_id) = step_status(store, &event.record, &event.receipt)?;
            let StepOutput { contract, usage } = store.blob_json(&event.record)?;
            let TurnStatements { model, host } = turn_statements(store, stream, &event.answers)?;
            let reason = (!outcome.succeeded()).then(|| failed_at(&outcome, &event.step_id));
            RunEventFact::Step {
                model,
                usage,
                host,
                step_id: event.step_id.into(),
                attempt: event.attempt,
                succeeded: outcome.succeeded(),
                receipt_id,
                output: contract,
                reason,
            }
        }
        RunEvent::Skipped(skipped) => RunEventFact::Skipped {
            step_ids: skipped.step_ids.into_iter().map(Into::into).collect(),
        },
        RunEvent::Suspended(suspended) => RunEventFact::Suspended {
            step_id: suspended.step_id.into(),
            attempt: suspended.attempt,
            hold: match suspended.hold {
                HoldEvent::Request {
                    request_id, digest, ..
                } => RunHold::Request {
                    address: request_id.into(),
                    request_digest: digest.into(),
                },
                HoldEvent::Retry {
                    request_id, digest, ..
                } => RunHold::Retry {
                    address: request_id.into(),
                    request_digest: digest.into(),
                },
                HoldEvent::Claimed {
                    request_id, digest, ..
                } => RunHold::Claimed {
                    address: request_id.into(),
                    request_digest: digest.into(),
                },
                HoldEvent::Child { run_id } => RunHold::Child {
                    run_id: run_id.into(),
                },
            },
        },
        RunEvent::Call(call) => {
            let input: JsonObject = store.blob_json(&call.input)?;
            let context: Vec<runx_contracts::javascript_worker::ScriptContext> = call
                .context
                .as_deref()
                .map(|blob| store.blob_json(blob))
                .transpose()?
                .unwrap_or_default();
            let loaded = match call.target {
                CallTarget::Load => loaded(store, paths, stream, &call.outcome)?,
                _ => None,
            };
            let outcome = call_outcome(store, call.outcome)?;
            RunEventFact::Call {
                step_id: call.step_id.into(),
                attempt: call.attempt,
                turn: call.turn.map(|turn| turn.request_id.into()),
                index: call.index,
                target: call.target,
                input_digest: call_input_digest(&input, &context)?.into(),
                call_id: call.call_id.map(Into::into),
                standing: call.standing.map(Box::new),
                outcome: Box::new(outcome),
                cancelled: call.cancelled,
                reuse: call.reuse,
                loaded: loaded.map(Box::new),
            }
        }
        RunEvent::Clock(read) => RunEventFact::Clock {
            step_id: read.step_id.into(),
            attempt: read.attempt,
            index: read.index,
            millis: read.millis,
        },
        RunEvent::Sealed(sealed) => RunEventFact::Sealed {
            outcome: sealed.outcome,
            receipt_id: sealed.receipt_id.into(),
        },
        RunEvent::Standing(standing) => RunEventFact::Standing {
            approvals: standing.approvals,
        },
        RunEvent::Consumed(_) => return Ok(None),
        RunEvent::Answered(answered) => RunEventFact::Answered {
            address: answered.request_id.into(),
            request_digest: answered.request_digest.into(),
            answer_digest: answered.value.into(),
            actor: if answered.human {
                ResolutionResponseActor::Human
            } else {
                ResolutionResponseActor::Agent
            },
        },
    };
    Ok(Some(RunEventRecord {
        stream: stream.into(),
        version: recorded.version,
        committed_at: recorded.committed_at.into(),
        event,
    }))
}

/// How a recorded call stands, as a host reads it.
fn call_outcome(
    store: &KernelStore,
    outcome: CallOutcomeEvent,
) -> Result<RunCallOutcome, RunRecordError> {
    Ok(match outcome {
        CallOutcomeEvent::Completed {
            record,
            receipt,
            outputs,
            ..
        } => {
            let (outcome, receipt_id) = step_status(store, &record, &receipt)?;
            let CallOutput { mut contract } = store.blob_json(&record)?;
            // A field an artifact holds is the artifact's alone.
            for part in &outputs {
                contract.remove(part.output.as_str());
            }
            RunCallOutcome::Completed {
                succeeded: outcome.succeeded(),
                receipt_id,
                output: contract,
                outputs,
                reason: (!outcome.succeeded())
                    .then(|| reason(outcome.failure_message().unwrap_or_default())),
            }
        }
        CallOutcomeEvent::Refused { reason: why } => RunCallOutcome::Refused {
            reason: reason(why),
        },
        CallOutcomeEvent::Declined { reason: why } => RunCallOutcome::Declined {
            reason: reason(why),
        },
        CallOutcomeEvent::Failed { failure } => RunCallOutcome::Failed {
            reason: reason(failure.to_string()),
        },
        CallOutcomeEvent::RetryDeclined => RunCallOutcome::Failed {
            reason: crate::execution::run_record::RETRY_DECLINED.into(),
        },
        CallOutcomeEvent::Pending {
            request_id, digest, ..
        } => RunCallOutcome::Pending {
            address: request_id.into(),
            request_digest: digest.into(),
        },
        CallOutcomeEvent::Claimed {
            request_id, digest, ..
        } => RunCallOutcome::Claimed {
            address: request_id.into(),
            request_digest: digest.into(),
        },
        CallOutcomeEvent::Executing => RunCallOutcome::Executing,
        CallOutcomeEvent::Child { run_id } => RunCallOutcome::Child {
            run_id: run_id.into(),
        },
        CallOutcomeEvent::Cancelled => RunCallOutcome::Cancelled,
        CallOutcomeEvent::Waited { at } => RunCallOutcome::Waited { at },
        CallOutcomeEvent::Read { artifact } => RunCallOutcome::Read { artifact },
        CallOutcomeEvent::TooLarge { size_bytes, bound } => {
            RunCallOutcome::TooLarge { size_bytes, bound }
        }
    })
}

/// How the step a record names ended, and its receipt's id.
fn step_status(
    store: &KernelStore,
    record: &str,
    receipt: &str,
) -> Result<(StepOutcome, NonEmptyString), RunRecordError> {
    let status: StepStatus = store.blob_json(record)?;
    let receipt: ReceiptId = store.blob_json(receipt)?;
    Ok((status.outcome, receipt.id.into()))
}

/// Why run `run_id` stopped, when its last step on its own stream failed:
/// that step's failure. Its children's steps are theirs.
fn why(store: &KernelStore, run_id: &str) -> Result<Option<NonEmptyString>, RunRecordError> {
    let Some(last) = store
        .read::<RunEvent>(run_id)?
        .into_iter()
        .rev()
        .find_map(|recorded| match recorded.event {
            RunEvent::Step(step) => Some(step),
            _ => None,
        })
    else {
        return Ok(None);
    };
    let (outcome, _) = step_status(store, &last.record, &last.receipt)?;
    Ok((!outcome.succeeded()).then(|| failed_at(&outcome, &last.step_id)))
}

/// A failed step's failure as a reader of its record is told it: placed at
/// its step, as the run's own error names it, so the record says where.
fn failed_at(outcome: &StepOutcome, step_id: &str) -> NonEmptyString {
    reason(
        outcome
            .failure("its process")
            .map(|failure| failure.at_step(step_id).to_string())
            .unwrap_or_default(),
    )
}

/// A recorded reason, which is never empty on the wire.
fn reason(text: String) -> NonEmptyString {
    NonEmptyString::new(text).unwrap_or_else(|| "the call gave no reason".into())
}

fn invalid(run_id: &str, reason: String) -> RunRecordError {
    RunRecordError::Invalid {
        run_id: run_id.to_owned(),
        reason,
    }
}

/// What an agent step's turn stated in the answer it consumed: the model
/// that answered, and the host that gave the answer. What it used is its
/// step's record's, which holds it whether the turn answered or failed.
#[derive(Default)]
struct TurnStatements {
    model: Option<NonEmptyString>,
    host: Option<runx_contracts::TurnHost>,
}

/// What a turn stated, from the answer it consumed: the turn's answer is the
/// one that carries the turn's input digest.
fn turn_statements(
    store: &KernelStore,
    stream: &str,
    answers: &[AnswerEvent],
) -> Result<TurnStatements, RunRecordError> {
    let Some(answer) = answers.iter().find(|answer| answer.input_digest.is_some()) else {
        return Ok(TurnStatements::default());
    };
    let value: JsonValue = store.blob_json(&answer.value)?;
    let refused = |reason: String| invalid(stream, format!("a turn's answer: {reason}"));
    Ok(TurnStatements {
        model: crate::execution::outcome::stated_model(&value)
            .map_err(refused)?
            .map(Into::into),
        host: crate::execution::outcome::stated_host(&value).map_err(refused)?,
    })
}

/// The package a load bound, from what its record holds: a completed load's
/// result, or, while it waits on the person's trust, what its request asks
/// them to trust. A load refused before it bound one names none.
fn loaded(
    store: &KernelStore,
    paths: &RunxPaths,
    stream: &str,
    outcome: &CallOutcomeEvent,
) -> Result<Option<LoadedPackage>, RunRecordError> {
    #[derive(Deserialize)]
    struct Bound {
        package: LoadedPackage,
    }
    /// A load's record: how it ended, and what it reported when it bound.
    #[derive(Deserialize)]
    struct LoadRecord {
        outcome: StepOutcome,
        contract: JsonObject,
    }
    match outcome {
        CallOutcomeEvent::Completed { record, .. } => {
            let record: LoadRecord = store.blob_json(record)?;
            if !record.outcome.succeeded() {
                return Ok(None);
            }
            let bound: Bound = serde_json::to_value(&record.contract)
                .and_then(serde_json::from_value)
                .map_err(|error| invalid(stream, format!("a load's result: {error}")))?;
            Ok(Some(bound.package))
        }
        CallOutcomeEvent::Pending { request, .. } => {
            let ResolutionRequest::Approval { gate, .. } = store.blob_json(request)? else {
                return Ok(None);
            };
            let payload = match (&gate.payload, &gate.payload_artifact) {
                (Some(payload), _) => payload.clone(),
                (None, Some(artifact)) => {
                    crate::services::read_json_artifact(&paths.artifacts(), artifact)
                        .map_err(|reason| invalid(stream, format!("a load's request: {reason}")))?
                }
                (None, None) => return Ok(None),
            };
            let bound: Bound = serde_json::to_value(&payload)
                .and_then(serde_json::from_value)
                .map_err(|error| invalid(stream, format!("a load's request: {error}")))?;
            Ok(Some(bound.package))
        }
        _ => Ok(None),
    }
}
