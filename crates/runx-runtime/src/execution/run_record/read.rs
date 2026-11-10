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
    CallTarget, JsonObject, JsonValue, LoadedPackage, ResolutionRequest, ResolutionResponseActor,
    RunCallOutcome, RunEventFact, RunEventRecord, RunHold, RunRead, RunReadSchema, RunReadSummary,
    WaitedRequest,
};
use serde::{Deserialize, Serialize};

use super::{
    AnswerEvent, CallOutcomeEvent, HoldEvent, RunEvent, RunRecordError, driven, input_digest,
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
    let step =
        |record: &str, receipt: &str| -> Result<(StepOutcome, NonEmptyString), RunRecordError> {
            let status: StepStatus = store.blob_json(record)?;
            let receipt: ReceiptId = store.blob_json(receipt)?;
            Ok((status.outcome, receipt.id.into()))
        };
    let event = match recorded.event {
        RunEvent::Started(started) => RunEventFact::Started {
            skill: started.skill.into(),
            method: started.method.into(),
            implementation: started.implementation,
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
            let (outcome, receipt_id) = step(&event.record, &event.receipt)?;
            let (model, host) = turn_statements(store, stream, &event.answers)?;
            RunEventFact::Step {
                model,
                host,
                step_id: event.step_id.into(),
                attempt: event.attempt,
                succeeded: outcome.succeeded(),
                receipt_id,
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
            let loaded = match call.target {
                CallTarget::Load => loaded(store, paths, stream, &call.outcome)?,
                _ => None,
            };
            let outcome = match call.outcome {
                CallOutcomeEvent::Completed {
                    record,
                    receipt,
                    outputs,
                    ..
                } => {
                    let (outcome, receipt_id) = step(&record, &receipt)?;
                    RunCallOutcome::Completed {
                        succeeded: outcome.succeeded(),
                        receipt_id,
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
                CallOutcomeEvent::Failed { reason: why } => RunCallOutcome::Failed {
                    reason: reason(why),
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
            };
            RunEventFact::Call {
                step_id: call.step_id.into(),
                attempt: call.attempt,
                turn: call.turn.map(|turn| turn.request_id.into()),
                index: call.index,
                target: call.target,
                input_digest: input_digest(&input)?.into(),
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

/// The model an agent step's turn stated and the host that answered it,
/// from the answer it consumed: the turn's answer is the one that carries the
/// turn's input digest.
fn turn_statements(
    store: &KernelStore,
    stream: &str,
    answers: &[AnswerEvent],
) -> Result<(Option<NonEmptyString>, Option<runx_contracts::TurnHost>), RunRecordError> {
    let Some(answer) = answers.iter().find(|answer| answer.input_digest.is_some()) else {
        return Ok((None, None));
    };
    let value: JsonValue = store.blob_json(&answer.value)?;
    let model = crate::execution::outcome::stated_model(&value)
        .map_err(|reason| invalid(stream, format!("a turn's answer: {reason}")))?;
    let host = crate::execution::outcome::stated_host(&value)
        .map_err(|reason| invalid(stream, format!("a turn's answer: {reason}")))?;
    Ok((model.map(Into::into), host))
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
