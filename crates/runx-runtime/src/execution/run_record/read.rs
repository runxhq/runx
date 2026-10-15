//! One run as a host reads it (`runx.run_read.v1`): its summary, what it waits
//! on, and its record as events after a cursor.
//!
//! A run and each of its child runs is a stream of its own, and the store
//! keeps no order across streams, so events come stream by stream, each in
//! version order. The cursor holds the last version returned for every stream
//! of the tree, which is what makes a read after it skip and repeat nothing,
//! including in child runs that began after the earlier read. The summary and
//! what the run waits on are read after the events, so they are never older
//! than the last event returned.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    JsonObject, ResolutionRequest, ResolutionResponseActor, RunCallOutcome, RunEventFact,
    RunEventRecord, RunHold, RunRead, RunReadSchema, RunReadSummary, WaitedRequest,
};
use serde::{Deserialize, Serialize};

use super::{
    CallOutcomeEvent, HoldEvent, RunEvent, RunRecordError, driven, input_digest, standing,
    standing_event, validate_run_id, waiting_requests,
};
use crate::execution::runner::StepOutcome;
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
    let heads = store.tree_heads::<RunEvent>(run_id)?;
    let mut positions = match after {
        Some(cursor) => Cursor::decode(cursor, run_id)?.at,
        None => BTreeMap::new(),
    };
    // A cursor names only streams of this tree, at versions they hold.
    for (stream, version) in &positions {
        let key = stream_key(run_id, stream);
        if !heads
            .iter()
            .any(|head| head.key == key && *version <= head.version)
        {
            return Err(RunRecordError::InvalidCursor);
        }
    }
    let mut events = Vec::new();
    let mut more = false;
    for head in &heads {
        let relative = relative_stream(run_id, &head.key);
        let at = positions.get(relative).copied().unwrap_or(0);
        if head.version <= at {
            continue;
        }
        let room = PAGE - events.len();
        if room == 0 {
            more = true;
            break;
        }
        let page = store.read_after::<RunEvent>(&head.key, at, room)?;
        let Some(last) = page.last().map(|recorded| recorded.version) else {
            continue;
        };
        for recorded in page {
            events.push(project(&store, &head.key, recorded)?);
        }
        positions.insert(relative.to_owned(), last);
        more |= last < head.version;
    }
    // An admitted answer updates the run but leaves it where it waits.
    let updated_at = store
        .latest::<RunEvent>(run_id)?
        .ok_or_else(unknown)?
        .committed_at;
    let latest = standing_event(&store, run_id)?.ok_or_else(unknown)?;
    let (state, disposition, receipt_id) = standing(&latest.event);
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
            runner: started.runner.into(),
            state,
            disposition,
            package_digest: started.package_digest.into(),
            execution_closure_digest: started.execution_closure_digest.into(),
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
            at: positions,
        }
        .encode(run_id)?
        .into(),
        more,
    })
}

/// The read position: the run, and the last version returned for each
/// stream of its tree, by its path below the run (`""` for the run itself).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    run: String,
    at: BTreeMap<String, u64>,
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

/// A stream's path below `run_id`: `""` for the run, `step/0` for a child.
fn relative_stream<'a>(run_id: &str, key: &'a str) -> &'a str {
    key.strip_prefix(run_id)
        .map_or(key, |rest| rest.strip_prefix('/').unwrap_or(rest))
}

fn stream_key(run_id: &str, relative: &str) -> String {
    if relative.is_empty() {
        run_id.to_owned()
    } else {
        format!("{run_id}/{relative}")
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

fn project(
    store: &KernelStore,
    stream: &str,
    recorded: Recorded<RunEvent>,
) -> Result<RunEventRecord, RunRecordError> {
    let step =
        |record: &str, receipt: &str| -> Result<(StepOutcome, NonEmptyString), RunRecordError> {
            let status: StepStatus = store.blob_json(record)?;
            let receipt: ReceiptId = store.blob_json(receipt)?;
            Ok((status.outcome, receipt.id.into()))
        };
    let event = match recorded.event {
        RunEvent::Started(started) => RunEventFact::Started {
            skill: started.skill.into(),
            runner: started.runner.into(),
        },
        RunEvent::ChildStarted(started) => RunEventFact::ChildStarted {
            parent: started.parent.into(),
            step_id: started.step_id.into(),
            skill: started.skill.into(),
        },
        RunEvent::EvaluationStarted(_) => {
            return Err(invalid(stream, "an evaluation inside a run".to_owned()));
        }
        RunEvent::Step(event) => {
            let (outcome, receipt_id) = step(&event.record, &event.receipt)?;
            RunEventFact::Step {
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
                CallOutcomeEvent::Child { run_id } => RunCallOutcome::Child {
                    run_id: run_id.into(),
                },
            };
            RunEventFact::Call {
                step_id: call.step_id.into(),
                attempt: call.attempt,
                turn: call.turn.request_id.into(),
                index: call.index,
                target: call.target,
                input_digest: input_digest(&input)?.into(),
                call_id: call.call_id.map(Into::into),
                standing: call.standing,
                outcome: Box::new(outcome),
            }
        }
        RunEvent::Sealed(sealed) => RunEventFact::Sealed {
            disposition: sealed.disposition,
            receipt_id: sealed.receipt_id.into(),
        },
        RunEvent::Standing(standing) => RunEventFact::Standing {
            approvals: standing.approvals,
        },
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
    Ok(RunEventRecord {
        stream: stream.into(),
        version: recorded.version,
        committed_at: recorded.committed_at.into(),
        event,
    })
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
