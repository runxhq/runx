//! The run record: the one durable record of a run.
//!
//! Every run is one `run` stream in the workspace's kernel store, keyed by its
//! id. The engine records each step, skip and suspension as it happens, each
//! step with the answers it consumed; the run's owner records its start and
//! its seal. An answer is recorded only by the step that used it, so an answer
//! the run refuses is never part of its record.
//! Step records, receipts and request and answer values are content-addressed
//! blobs committed with the event that names them, so each is stored once.
//!
//! Every append states the version its writer last read, so exactly one
//! driver advances a run; a second continuation is refused, never raced.
//! Replay rebuilds a run from its events alone. Resume, retries, history and
//! paused-request reads never read a receipt or any other file back as live
//! state.
//!
//! A nested skill step is a child run: its own stream, keyed
//! `<parent>/<step>`, that names its parent step. A harness evaluation is a
//! run of its own that is never resumed or listed.
//!
//! While a run waits on an agent act, each call the agent makes is a `call`
//! event naming the turn it belongs to. Calls never change where the run
//! waits: a run whose latest event is a call is still paused at its turn.

use std::sync::{Arc, Mutex};

use runx_contracts::{
    ClosureDisposition, ContextEntry, JsonObject, JsonValue, Receipt, ResolutionRequest, TurnTarget,
};
use runx_core::state_machine::StepAdmissionWitness;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::runner::{StepOutcome, StepRun};
use crate::kernel::authority::RunAuthority;
use crate::kernel::store::{
    Append, Blob, Expected, Head, KernelStore, KernelStoreError, StreamEvent,
};

/// One event in a run's stream.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum RunEvent {
    Started(Box<Started>),
    ChildStarted(Box<ChildStarted>),
    EvaluationStarted(Box<EvaluationStarted>),
    Step(Box<StepEvent>),
    Skipped(Skipped),
    Suspended(Box<SuspendedEvent>),
    Call(Box<CallEvent>),
    Sealed(Box<SealedEvent>),
}

impl StreamEvent for RunEvent {
    const KIND: &'static str = "run";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Started(_) => "started",
            Self::ChildStarted(_) => "child_started",
            Self::EvaluationStarted(_) => "evaluation_started",
            Self::Step(_) => "step",
            Self::Skipped(_) => "skipped",
            Self::Suspended(_) => "suspended",
            Self::Call(_) => "call",
            Self::Sealed(_) => "sealed",
        }
    }
}

/// What a top-level run is bound to. A pinned run id is a submission
/// identity: a continuation under it must present the same identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Started {
    pub skill: String,
    /// The path the skill was loaded from, for `runx resume`.
    pub skill_ref: String,
    pub runner: String,
    pub package_digest: String,
    pub execution_closure_digest: String,
    /// Blob: the run's inputs.
    pub inputs: String,
    /// Blob: the caller's current and historical context, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_authority: Option<RunAuthority>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChildStarted {
    pub parent: String,
    pub step_id: String,
    pub skill: String,
    /// Blob: the child's materialized inputs.
    pub inputs: String,
    pub created_at: String,
}

/// A harness evaluation: a fixture run against its target, never resumed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvaluationStarted {
    pub fixture: String,
    pub target: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepEvent {
    step_id: String,
    attempt: u32,
    /// Blob: [`RecordedStep`].
    record: String,
    /// Blob: the step receipt.
    receipt: String,
    /// Blobs: the step's nested receipts, descendants before their roots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    nested: Vec<String>,
    /// The answers the step consumed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    answers: Vec<AnswerEvent>,
}

/// A step's record without its receipts, which are blobs of their own.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedStep {
    skill: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runner: Option<String>,
    contract: JsonObject,
    outcome: StepOutcome,
    admission_witness: StepAdmissionWitness,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Skipped {
    step_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuspendedEvent {
    step_id: String,
    attempt: u32,
    hold: HoldEvent,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HoldEvent {
    /// The step asked its host and is waiting for the answer.
    Request {
        request_id: String,
        request: String,
        digest: String,
    },
    /// The step's child run holds the request.
    Child { run_id: String },
    /// The step has not started: it waits for a bounded retry, acknowledged
    /// by this exact request.
    Retry {
        request_id: String,
        request: String,
        digest: String,
    },
}

/// A call an agent made during its turn. A later event with the same index
/// settles a call that was pending.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallEvent {
    step_id: String,
    attempt: u32,
    /// The request the turn answers, so the run's latest event still names
    /// what it waits on.
    turn: TurnRequestEvent,
    index: u32,
    target: TurnTarget,
    /// Blob: the call's input.
    input: String,
    outcome: CallOutcomeEvent,
    /// The answers the call consumed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    answers: Vec<AnswerEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnRequestEvent {
    request_id: String,
    /// Blob: the agent act request.
    request: String,
    digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CallOutcomeEvent {
    /// The call ran as a step: blobs of its record and receipts.
    Completed {
        record: String,
        receipt: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        nested: Vec<String>,
    },
    /// Admission refused the call; nothing ran.
    Refused { reason: String },
    /// The call could not run.
    Failed { reason: String },
    /// The call waits on a request: what it asked, or, while it has not
    /// settled, its own retry.
    Pending {
        request_id: String,
        request: String,
        digest: String,
    },
}

/// An answer a step consumed, bound to the request it answered.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerEvent {
    request_id: String,
    request_digest: String,
    /// Blob: the answer.
    value: String,
    human: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedEvent {
    disposition: ClosureDisposition,
    /// The receipt the run presents: the domain act when there is one.
    receipt_id: String,
    /// Blob: the graph receipt.
    receipt: String,
    /// Blob: the domain act receipt, when the runner declares `act:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    primary: Option<String>,
    /// Blob: the public result of a run that stopped short of completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_result: Option<String>,
}

/// The caller's context, stored as one blob.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunContext {
    pub current: Vec<ContextEntry>,
    pub historical: Vec<ContextEntry>,
}

/// An answer a step consumed: who gave it, and the request it was bound to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Answer {
    pub request_id: String,
    pub request_digest: String,
    pub value: JsonValue,
    pub human: bool,
}

/// Where a suspended run waits.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Hold {
    Request(PendingRequest),
    Child(String),
    Retry(PendingRequest),
}

/// A request a run is waiting on, with the digest an answer must bind to.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PendingRequest {
    pub id: String,
    pub value: JsonValue,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Suspension {
    pub step_id: String,
    pub attempt: u32,
    pub hold: Hold,
}

/// A call an agent made during its turn, as its latest event settled it.
#[derive(Clone, Debug)]
pub(crate) struct RecordedCall {
    pub step_id: String,
    pub attempt: u32,
    pub index: u32,
    pub target: TurnTarget,
    pub input: JsonObject,
    pub outcome: CallOutcome,
}

#[derive(Clone, Debug)]
pub(crate) enum CallOutcome {
    Completed(Box<StepRun>),
    Refused(String),
    Failed(String),
    Pending(PendingRequest),
}

/// What a call records: its turn, its place in it, and how it ended.
pub(crate) struct CallRecord<'a> {
    pub step_id: &'a str,
    pub attempt: u32,
    pub turn: &'a PendingRequest,
    pub index: u32,
    pub target: &'a TurnTarget,
    pub input: &'a JsonObject,
    pub outcome: &'a CallOutcome,
    pub answers: &'a [Answer],
}

/// How a run began.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Origin {
    Run(Box<Started>),
    Child(Box<ChildStarted>),
    Evaluation(Box<EvaluationStarted>),
}

/// One replayed step-level fact, in stream order.
#[derive(Clone, Debug)]
pub(crate) enum Replayed {
    Step(Box<StepRun>),
    Skipped(Vec<String>),
}

#[derive(Clone, Debug)]
pub(crate) struct Sealed {
    pub disposition: ClosureDisposition,
    pub receipt: Receipt,
    pub primary: Option<Receipt>,
    pub terminal_result: Option<JsonValue>,
}

/// A run rebuilt from its stream.
#[derive(Clone, Debug)]
pub(crate) struct RunRecord {
    pub origin: Origin,
    pub created_at: String,
    pub inputs: JsonObject,
    pub context: RunContext,
    pub facts: Vec<Replayed>,
    /// Where the run waits now, when its last step-level event is a
    /// suspension no later step has settled.
    pub suspension: Option<Suspension>,
    /// Every answer the run's steps and calls consumed, in order.
    pub answers: Vec<Answer>,
    /// Every call agents made during the run's turns, each as its latest
    /// event settled it, in the order they were first made.
    pub calls: Vec<RecordedCall>,
    /// How many times the run has waited for a bounded retry.
    pub retries: u32,
    pub sealed: Option<Sealed>,
}

/// Where a run stands, from its latest event alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Paused,
    Sealed,
}

impl RunState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Sealed => "sealed",
        }
    }
}

#[derive(Debug, Error)]
pub enum RunRecordError {
    #[error(transparent)]
    Store(#[from] KernelStoreError),
    #[error("run {run_id} was advanced by another driver; this continuation is refused")]
    Contended { run_id: String },
    #[error("run {run_id} already exists with a different {field}")]
    IdentityMismatch { run_id: String, field: &'static str },
    #[error("run {run_id} has an invalid record: {reason}")]
    Invalid { run_id: String, reason: String },
    #[error("run id {run_id:?} is invalid: use letters, digits and _ - . : (at most 200 bytes)")]
    InvalidId { run_id: String },
    #[error("the system random source is unavailable for a run id")]
    Entropy,
    #[error("invalid {field} filter '{value}': expected an RFC 3339 timestamp")]
    InvalidFilter { field: &'static str, value: String },
}

/// The writer of one run's stream.
#[derive(Clone)]
pub struct RunLog(Arc<RunLogInner>);

struct RunLogInner {
    store: Arc<KernelStore>,
    run_id: String,
    version: Mutex<u64>,
}

impl std::fmt::Debug for RunLog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunLog")
            .field("run_id", &self.0.run_id)
            .finish_non_exhaustive()
    }
}

impl RunLog {
    /// The log of `run_id`, positioned at the stream's current version.
    pub fn open(
        store: Arc<KernelStore>,
        run_id: impl Into<String>,
    ) -> Result<Self, RunRecordError> {
        let run_id = run_id.into();
        let version = store.version::<RunEvent>(&run_id)?;
        Ok(Self(Arc::new(RunLogInner {
            store,
            run_id,
            version: Mutex::new(version),
        })))
    }

    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.0.run_id
    }

    #[must_use]
    pub(crate) fn store(&self) -> &Arc<KernelStore> {
        &self.0.store
    }

    /// The log of the child run that a nested skill step runs as.
    pub(crate) fn child(&self, step_id: &str) -> Result<Self, RunRecordError> {
        Self::open(
            Arc::clone(&self.0.store),
            format!("{}/{step_id}", self.0.run_id),
        )
    }

    /// Rebuild the run from its stream; `None` when it has not started.
    pub(crate) fn load(&self) -> Result<Option<RunRecord>, RunRecordError> {
        let events = self.0.store.read::<RunEvent>(&self.0.run_id)?;
        let version = events.last().map_or(0, |event| event.version);
        *self.version() = version;
        replay(
            &self.0.store,
            &self.0.run_id,
            events.into_iter().map(|event| event.event),
        )
    }

    /// The calls of one turn, in order, as this driver last saw the run. A
    /// run another driver has advanced since is contended.
    pub(crate) fn calls(
        &self,
        step_id: &str,
        attempt: u32,
    ) -> Result<Vec<RecordedCall>, RunRecordError> {
        let events = self.0.store.read::<RunEvent>(&self.0.run_id)?;
        if events.last().map_or(0, |event| event.version) != *self.version() {
            return Err(RunRecordError::Contended {
                run_id: self.0.run_id.clone(),
            });
        }
        turn_calls(
            &self.0.store,
            events.into_iter().map(|event| event.event),
            step_id,
            attempt,
        )
    }

    /// Start a top-level run. The stream must not exist yet: a run that does
    /// is continued from its record, never started again.
    pub(crate) fn begin(&self, started: StartRequest<'_>) -> Result<(), RunRecordError> {
        let inputs = Blob::json(started.inputs)?;
        let context = (!started.context.current.is_empty()
            || !started.context.historical.is_empty())
        .then(|| Blob::json(started.context))
        .transpose()?;
        let event = Started {
            skill: started.skill.to_owned(),
            skill_ref: started.skill_ref.to_owned(),
            runner: started.runner.to_owned(),
            package_digest: started.package_digest.to_owned(),
            execution_closure_digest: started.execution_closure_digest.to_owned(),
            inputs: inputs.digest().to_owned(),
            context: context.as_ref().map(|blob| blob.digest().to_owned()),
            run_authority: started.run_authority.cloned(),
            created_at: started.created_at.to_owned(),
        };
        let mut blobs = vec![inputs];
        blobs.extend(context);
        self.append_first(RunEvent::Started(Box::new(event)), blobs)
    }

    /// Start a harness evaluation: the run a fixture replays against its
    /// target. It is recorded like any run, and never resumed or listed.
    pub fn evaluation(
        store: Arc<KernelStore>,
        run_id: impl Into<String>,
        fixture: &str,
        target: &str,
        created_at: &str,
    ) -> Result<Self, RunRecordError> {
        let log = Self::open(store, run_id)?;
        log.append_first(
            RunEvent::EvaluationStarted(Box::new(EvaluationStarted {
                fixture: fixture.to_owned(),
                target: target.to_owned(),
                created_at: created_at.to_owned(),
            })),
            Vec::new(),
        )?;
        Ok(log)
    }

    /// Start, or continue, the child run a nested skill step runs as.
    pub(crate) fn start_child(
        &self,
        parent: &str,
        step_id: &str,
        skill: &str,
        inputs: &JsonObject,
        created_at: &str,
    ) -> Result<Option<RunRecord>, RunRecordError> {
        let inputs = Blob::json(inputs)?;
        if let Some(existing) = self.load()? {
            let Origin::Child(recorded) = &existing.origin else {
                return Err(self.mismatch("origin"));
            };
            if recorded.parent != parent || recorded.step_id != step_id || recorded.skill != skill {
                return Err(self.mismatch("parent step"));
            }
            if recorded.inputs != inputs.digest() {
                return Err(self.mismatch("inputs"));
            }
            return Ok(Some(existing));
        }
        let event = ChildStarted {
            parent: parent.to_owned(),
            step_id: step_id.to_owned(),
            skill: skill.to_owned(),
            inputs: inputs.digest().to_owned(),
            created_at: created_at.to_owned(),
        };
        self.append_first(RunEvent::ChildStarted(Box::new(event)), vec![inputs])?;
        Ok(None)
    }

    pub(crate) fn record_step(
        &self,
        step: &StepRun,
        answers: &[Answer],
    ) -> Result<(), RunRecordError> {
        let record = Blob::json(&RecordedStep {
            skill: step.skill.clone(),
            runner: step.runner.clone(),
            contract: step.contract.clone(),
            outcome: step.outcome.clone(),
            admission_witness: step.admission_witness.clone(),
        })?;
        let receipt = Blob::json(&step.receipt)?;
        let nested = step
            .nested_receipts
            .iter()
            .map(Blob::json)
            .collect::<Result<Vec<_>, _>>()?;
        let values = answers
            .iter()
            .map(|answer| Blob::json(&answer.value))
            .collect::<Result<Vec<_>, _>>()?;
        let event = StepEvent {
            step_id: step.step_id.clone(),
            attempt: step.attempt,
            record: record.digest().to_owned(),
            receipt: receipt.digest().to_owned(),
            nested: nested.iter().map(|blob| blob.digest().to_owned()).collect(),
            answers: answers
                .iter()
                .zip(&values)
                .map(|(answer, value)| AnswerEvent {
                    request_id: answer.request_id.clone(),
                    request_digest: answer.request_digest.clone(),
                    value: value.digest().to_owned(),
                    human: answer.human,
                })
                .collect(),
        };
        let mut blobs = vec![record, receipt];
        blobs.extend(nested);
        blobs.extend(values);
        self.append(RunEvent::Step(Box::new(event)), blobs)
    }

    pub(crate) fn record_skipped(&self, step_ids: Vec<String>) -> Result<(), RunRecordError> {
        if step_ids.is_empty() {
            return Ok(());
        }
        self.append(RunEvent::Skipped(Skipped { step_ids }), Vec::new())
    }

    /// The step waits on its own host request.
    pub(crate) fn record_request(
        &self,
        step_id: &str,
        attempt: u32,
        request: &ResolutionRequest,
    ) -> Result<(), RunRecordError> {
        let (request_id, blob, digest) = request_blob(request)?;
        self.append(
            RunEvent::Suspended(Box::new(SuspendedEvent {
                step_id: step_id.to_owned(),
                attempt,
                hold: HoldEvent::Request {
                    request_id,
                    request: blob.digest().to_owned(),
                    digest,
                },
            })),
            vec![blob],
        )
    }

    /// The step waits on the request its child run holds.
    pub(crate) fn record_child_hold(
        &self,
        step_id: &str,
        attempt: u32,
        child: &str,
    ) -> Result<(), RunRecordError> {
        self.append(
            RunEvent::Suspended(Box::new(SuspendedEvent {
                step_id: step_id.to_owned(),
                attempt,
                hold: HoldEvent::Child {
                    run_id: child.to_owned(),
                },
            })),
            Vec::new(),
        )
    }

    /// The step has not started; it waits for a bounded retry.
    pub(crate) fn record_retry(
        &self,
        step_id: &str,
        request: &ResolutionRequest,
    ) -> Result<(), RunRecordError> {
        let (request_id, blob, digest) = request_blob(request)?;
        self.append(
            RunEvent::Suspended(Box::new(SuspendedEvent {
                step_id: step_id.to_owned(),
                attempt: 0,
                hold: HoldEvent::Retry {
                    request_id,
                    request: blob.digest().to_owned(),
                    digest,
                },
            })),
            vec![blob],
        )
    }

    /// A call an agent made during its turn, with the answers it consumed.
    pub(crate) fn record_call(&self, call: &CallRecord<'_>) -> Result<(), RunRecordError> {
        let turn = Blob::json(&call.turn.value)?;
        let input = Blob::json(call.input)?;
        let mut blobs = vec![turn, input];
        let outcome = match call.outcome {
            CallOutcome::Completed(step) => {
                let record = Blob::json(&RecordedStep {
                    skill: step.skill.clone(),
                    runner: step.runner.clone(),
                    contract: step.contract.clone(),
                    outcome: step.outcome.clone(),
                    admission_witness: step.admission_witness.clone(),
                })?;
                let receipt = Blob::json(&step.receipt)?;
                let nested = step
                    .nested_receipts
                    .iter()
                    .map(Blob::json)
                    .collect::<Result<Vec<_>, _>>()?;
                let outcome = CallOutcomeEvent::Completed {
                    record: record.digest().to_owned(),
                    receipt: receipt.digest().to_owned(),
                    nested: nested.iter().map(|blob| blob.digest().to_owned()).collect(),
                };
                blobs.extend([record, receipt]);
                blobs.extend(nested);
                outcome
            }
            CallOutcome::Refused(reason) => CallOutcomeEvent::Refused {
                reason: reason.clone(),
            },
            CallOutcome::Failed(reason) => CallOutcomeEvent::Failed {
                reason: reason.clone(),
            },
            CallOutcome::Pending(pending) => {
                let request = Blob::json(&pending.value)?;
                let outcome = CallOutcomeEvent::Pending {
                    request_id: pending.id.clone(),
                    request: request.digest().to_owned(),
                    digest: pending.digest.clone(),
                };
                blobs.push(request);
                outcome
            }
        };
        let values = call
            .answers
            .iter()
            .map(|answer| Blob::json(&answer.value))
            .collect::<Result<Vec<_>, _>>()?;
        let event = CallEvent {
            step_id: call.step_id.to_owned(),
            attempt: call.attempt,
            turn: TurnRequestEvent {
                request_id: call.turn.id.clone(),
                request: blobs[0].digest().to_owned(),
                digest: call.turn.digest.clone(),
            },
            index: call.index,
            target: call.target.clone(),
            input: blobs[1].digest().to_owned(),
            outcome,
            answers: call
                .answers
                .iter()
                .zip(&values)
                .map(|(answer, value)| AnswerEvent {
                    request_id: answer.request_id.clone(),
                    request_digest: answer.request_digest.clone(),
                    value: value.digest().to_owned(),
                    human: answer.human,
                })
                .collect(),
        };
        blobs.extend(values);
        self.append(RunEvent::Call(Box::new(event)), blobs)
    }

    pub(crate) fn record_sealed(
        &self,
        receipt: &Receipt,
        primary: Option<&Receipt>,
        terminal_result: Option<&JsonValue>,
    ) -> Result<(), RunRecordError> {
        let receipt_blob = Blob::json(receipt)?;
        let primary_id = primary
            .map_or(&receipt.id, |primary| &primary.id)
            .to_string();
        let primary = primary.map(Blob::json).transpose()?;
        let terminal_result = terminal_result.map(Blob::json).transpose()?;
        let event = SealedEvent {
            disposition: receipt.seal.disposition.clone(),
            receipt_id: primary_id,
            receipt: receipt_blob.digest().to_owned(),
            primary: primary.as_ref().map(|blob| blob.digest().to_owned()),
            terminal_result: terminal_result
                .as_ref()
                .map(|blob| blob.digest().to_owned()),
        };
        let mut blobs = vec![receipt_blob];
        blobs.extend(primary);
        blobs.extend(terminal_result);
        self.append(RunEvent::Sealed(Box::new(event)), blobs)
    }

    /// A run's first event: the stream must not exist yet.
    fn append_first(&self, event: RunEvent, blobs: Vec<Blob>) -> Result<(), RunRecordError> {
        self.append_expected(Expected::Absent, event, blobs)
    }

    fn append(&self, event: RunEvent, blobs: Vec<Blob>) -> Result<(), RunRecordError> {
        let expected = match *self.version() {
            0 => {
                return Err(RunRecordError::Invalid {
                    run_id: self.0.run_id.clone(),
                    reason: "the run was not started".to_owned(),
                });
            }
            version => Expected::Version(version),
        };
        self.append_expected(expected, event, blobs)
    }

    fn append_expected(
        &self,
        expected: Expected,
        event: RunEvent,
        blobs: Vec<Blob>,
    ) -> Result<(), RunRecordError> {
        let mut version = self.version();
        let append = Append::kernel(self.0.run_id.clone(), expected, &event)?.with_blobs(blobs);
        match self.0.store.append(vec![append]) {
            Ok(versions) => {
                *version = versions.first().copied().unwrap_or(*version + 1);
                Ok(())
            }
            Err(KernelStoreError::Conflict { .. }) => Err(RunRecordError::Contended {
                run_id: self.0.run_id.clone(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    fn version(&self) -> std::sync::MutexGuard<'_, u64> {
        self.0
            .version
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn mismatch(&self, field: &'static str) -> RunRecordError {
        RunRecordError::IdentityMismatch {
            run_id: self.0.run_id.clone(),
            field,
        }
    }
}

/// What a top-level run is started with.
pub(crate) struct StartRequest<'a> {
    pub skill: &'a str,
    pub skill_ref: &'a str,
    pub runner: &'a str,
    pub package_digest: &'a str,
    pub execution_closure_digest: &'a str,
    pub inputs: &'a JsonObject,
    pub context: &'a RunContext,
    pub run_authority: Option<&'a RunAuthority>,
    pub created_at: &'a str,
}

/// The longest run id a caller may pin.
const MAX_RUN_ID_BYTES: usize = 200;

/// A caller-pinned run id: letters, digits and `_ - . :`, so it can never
/// name a child run or escape a key space.
pub(crate) fn validate_run_id(run_id: &str) -> Result<&str, RunRecordError> {
    let valid = !run_id.is_empty()
        && run_id.len() <= MAX_RUN_ID_BYTES
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'));
    if valid {
        Ok(run_id)
    } else {
        Err(RunRecordError::InvalidId {
            run_id: run_id.to_owned(),
        })
    }
}

/// A new run id: `run_<skill>_<time><random>`, unique per invocation and
/// ordered by when it was minted.
pub(crate) fn mint_run_id(skill: &str) -> Result<String, RunRecordError> {
    use ring::rand::SecureRandom as _;
    let mut random = [0_u8; 6];
    ring::rand::SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| RunRecordError::Entropy)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let segment = skill
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let segment = segment.trim_matches('-');
    let segment = segment.get(..segment.len().min(64)).unwrap_or(segment);
    Ok(format!(
        "run_{segment}_{millis:012x}{}",
        runx_contracts::hex_lower(&random)
    ))
}

/// The digest an answer to `request` binds to.
pub(crate) fn request_digest(request: &ResolutionRequest) -> Result<String, RunRecordError> {
    request_blob(request).map(|(_, _, digest)| digest)
}

fn request_blob(request: &ResolutionRequest) -> Result<(String, Blob, String), RunRecordError> {
    let codec = |error: serde_json::Error| KernelStoreError::Codec(error.to_string());
    let value = serde_json::to_value(request).map_err(codec)?;
    let bytes = serde_json::to_vec(&value).map_err(codec)?;
    let digest = runx_contracts::sha256_prefixed(&bytes);
    let id = match request {
        ResolutionRequest::Input { id, .. }
        | ResolutionRequest::Approval { id, .. }
        | ResolutionRequest::AgentAct { id, .. }
        | ResolutionRequest::Retry { id, .. } => id.to_string(),
    };
    Ok((id, Blob::new(bytes)?, digest))
}

// Function rationale: replay folds one stream in order; its invariants (one
// origin, steps after the origin, nothing after the seal) stay together.
fn replay(
    store: &KernelStore,
    run_id: &str,
    events: impl IntoIterator<Item = RunEvent>,
) -> Result<Option<RunRecord>, RunRecordError> {
    let invalid = |reason: &str| RunRecordError::Invalid {
        run_id: run_id.to_owned(),
        reason: reason.to_owned(),
    };
    let mut events = events.into_iter();
    let Some(first) = events.next() else {
        return Ok(None);
    };
    let (origin, inputs_digest, context, created_at) = match first {
        RunEvent::Started(started) => {
            let digest = Some(started.inputs.clone());
            let context = started
                .context
                .as_ref()
                .map(|digest| store.blob_json(digest))
                .transpose()?
                .unwrap_or_default();
            let created_at = started.created_at.clone();
            (Origin::Run(started), digest, context, created_at)
        }
        RunEvent::ChildStarted(started) => {
            let digest = Some(started.inputs.clone());
            let created_at = started.created_at.clone();
            (
                Origin::Child(started),
                digest,
                RunContext::default(),
                created_at,
            )
        }
        RunEvent::EvaluationStarted(started) => {
            let created_at = started.created_at.clone();
            (
                Origin::Evaluation(started),
                None,
                RunContext::default(),
                created_at,
            )
        }
        _ => return Err(invalid("the stream does not begin with its start")),
    };
    let mut record = RunRecord {
        origin,
        created_at,
        inputs: inputs_digest
            .map(|digest| store.blob_json(&digest))
            .transpose()?
            .unwrap_or_default(),
        context,
        facts: Vec::new(),
        suspension: None,
        answers: Vec::new(),
        calls: Vec::new(),
        retries: 0,
        sealed: None,
    };
    for event in events {
        if record.sealed.is_some() {
            return Err(invalid("an event follows the seal"));
        }
        match event {
            RunEvent::Started(_) | RunEvent::ChildStarted(_) | RunEvent::EvaluationStarted(_) => {
                return Err(invalid("the run starts twice"));
            }
            RunEvent::Step(step) => {
                record.suspension = None;
                for answer in &step.answers {
                    record.answers.push(Answer {
                        request_id: answer.request_id.clone(),
                        request_digest: answer.request_digest.clone(),
                        value: store.blob_json(&answer.value)?,
                        human: answer.human,
                    });
                }
                record
                    .facts
                    .push(Replayed::Step(Box::new(step_run(store, *step)?)));
            }
            RunEvent::Skipped(skipped) => {
                record.suspension = None;
                record.facts.push(Replayed::Skipped(skipped.step_ids));
            }
            RunEvent::Suspended(suspended) => {
                let hold = match suspended.hold {
                    HoldEvent::Request {
                        request_id,
                        request,
                        digest,
                    } => Hold::Request(PendingRequest {
                        id: request_id,
                        value: store.blob_json(&request)?,
                        digest,
                    }),
                    HoldEvent::Child { run_id } => Hold::Child(run_id),
                    HoldEvent::Retry {
                        request_id,
                        request,
                        digest,
                    } => {
                        record.retries = record.retries.saturating_add(1);
                        Hold::Retry(PendingRequest {
                            id: request_id,
                            value: store.blob_json(&request)?,
                            digest,
                        })
                    }
                };
                record.suspension = Some(Suspension {
                    step_id: suspended.step_id,
                    attempt: suspended.attempt,
                    hold,
                });
            }
            RunEvent::Call(call) => {
                let call = *call;
                for answer in &call.answers {
                    record.answers.push(Answer {
                        request_id: answer.request_id.clone(),
                        request_digest: answer.request_digest.clone(),
                        value: store.blob_json(&answer.value)?,
                        human: answer.human,
                    });
                }
                settle_call(&mut record.calls, recorded_call(store, call)?);
            }
            RunEvent::Sealed(sealed) => {
                record.suspension = None;
                record.sealed = Some(Sealed {
                    disposition: sealed.disposition,
                    receipt: store.blob_json(&sealed.receipt)?,
                    primary: sealed
                        .primary
                        .map(|digest| store.blob_json(&digest))
                        .transpose()?,
                    terminal_result: sealed
                        .terminal_result
                        .map(|digest| store.blob_json(&digest))
                        .transpose()?,
                });
            }
        }
    }
    Ok(Some(record))
}

/// One turn's calls, each as its latest event settled it, in order.
fn turn_calls(
    store: &KernelStore,
    events: impl IntoIterator<Item = RunEvent>,
    step_id: &str,
    attempt: u32,
) -> Result<Vec<RecordedCall>, RunRecordError> {
    let mut calls = Vec::new();
    for event in events {
        if let RunEvent::Call(call) = event
            && call.step_id == step_id
            && call.attempt == attempt
        {
            settle_call(&mut calls, recorded_call(store, *call)?);
        }
    }
    calls.sort_by_key(|call| call.index);
    Ok(calls)
}

/// A later event for a call's index settles the call.
fn settle_call(calls: &mut Vec<RecordedCall>, recorded: RecordedCall) {
    match calls.iter_mut().find(|known| {
        known.step_id == recorded.step_id
            && known.attempt == recorded.attempt
            && known.index == recorded.index
    }) {
        Some(known) => *known = recorded,
        None => calls.push(recorded),
    }
}

fn recorded_call(store: &KernelStore, call: CallEvent) -> Result<RecordedCall, RunRecordError> {
    Ok(RecordedCall {
        outcome: call_outcome(store, &call)?,
        input: store.blob_json(&call.input)?,
        step_id: call.step_id,
        attempt: call.attempt,
        index: call.index,
        target: call.target,
    })
}

fn call_outcome(store: &KernelStore, call: &CallEvent) -> Result<CallOutcome, RunRecordError> {
    Ok(match &call.outcome {
        CallOutcomeEvent::Completed {
            record,
            receipt,
            nested,
        } => CallOutcome::Completed(Box::new(step_run(
            store,
            StepEvent {
                step_id: call_step_id(&call.step_id, call.index),
                attempt: 1,
                record: record.clone(),
                receipt: receipt.clone(),
                nested: nested.clone(),
                answers: Vec::new(),
            },
        )?)),
        CallOutcomeEvent::Refused { reason } => CallOutcome::Refused(reason.clone()),
        CallOutcomeEvent::Failed { reason } => CallOutcome::Failed(reason.clone()),
        CallOutcomeEvent::Pending {
            request_id,
            request,
            digest,
        } => CallOutcome::Pending(PendingRequest {
            id: request_id.clone(),
            value: store.blob_json(request)?,
            digest: digest.clone(),
        }),
    })
}

/// The step a call runs as: `<turn step>/<index>`, so a child run it starts
/// is `<run>/<turn step>/<index>`.
pub(crate) fn call_step_id(turn_step: &str, index: u32) -> String {
    format!("{turn_step}/{index}")
}

fn step_run(store: &KernelStore, event: StepEvent) -> Result<StepRun, RunRecordError> {
    let record: RecordedStep = store.blob_json(&event.record)?;
    Ok(StepRun {
        step_id: event.step_id,
        attempt: event.attempt,
        skill: record.skill,
        runner: record.runner,
        contract: record.contract,
        ephemeral_contract: Default::default(),
        outcome: record.outcome,
        receipt: store.blob_json(&event.receipt)?,
        nested_receipts: event
            .nested
            .iter()
            .map(|digest| store.blob_json(digest))
            .collect::<Result<_, _>>()?,
        admission_witness: record.admission_witness,
    })
}

/// The request a run waits on, following child runs down to the step that
/// asked. `None` unless the run's latest event is a suspension: once a host
/// has answered, the run waits on its own continuation, not on the host.
pub(crate) fn pending_request(
    store: &KernelStore,
    run_id: &str,
) -> Result<Option<PendingRequest>, RunRecordError> {
    Ok(holder(store, run_id)?.map(|holder| holder.request))
}

/// Every request a paused run waits on: the one its step asked, then those
/// its open turn's calls wait on.
pub(crate) fn waiting_requests(
    store: &KernelStore,
    run_id: &str,
) -> Result<Vec<PendingRequest>, RunRecordError> {
    let Some(holder) = holder(store, run_id)? else {
        return Ok(Vec::new());
    };
    // Only an agent turn makes calls of its own.
    if holder
        .request
        .value
        .as_object()
        .and_then(|value| value.get("kind"))
        != Some(&JsonValue::String("agent_act".to_owned()))
    {
        return Ok(vec![holder.request]);
    }
    let calls = turn_calls(
        store,
        store
            .read::<RunEvent>(&holder.run_id)?
            .into_iter()
            .map(|event| event.event),
        &holder.step_id,
        holder.attempt,
    )?;
    let mut waiting = vec![holder.request];
    waiting.extend(calls.into_iter().filter_map(|call| match call.outcome {
        CallOutcome::Pending(request) => Some(request),
        _ => None,
    }));
    Ok(waiting)
}

/// Where a run's wait is held: the run and step that asked, and the request.
struct Holder {
    run_id: String,
    step_id: String,
    attempt: u32,
    request: PendingRequest,
}

/// What `run_id` waits on, following child runs down to the step that asked.
fn holder(store: &KernelStore, run_id: &str) -> Result<Option<Holder>, RunRecordError> {
    let mut current = run_id.to_owned();
    loop {
        let Some(latest) = store.latest::<RunEvent>(&current)? else {
            return Ok(None);
        };
        let suspended = match latest.event {
            RunEvent::Suspended(suspended) => suspended,
            // A call leaves the run waiting on the turn it was made in.
            RunEvent::Call(call) => {
                return Ok(Some(Holder {
                    run_id: current,
                    request: PendingRequest {
                        id: call.turn.request_id,
                        value: store.blob_json(&call.turn.request)?,
                        digest: call.turn.digest,
                    },
                    step_id: call.step_id,
                    attempt: call.attempt,
                }));
            }
            _ => return Ok(None),
        };
        match suspended.hold {
            HoldEvent::Request {
                request_id,
                request,
                digest,
            }
            | HoldEvent::Retry {
                request_id,
                request,
                digest,
            } => {
                return Ok(Some(Holder {
                    run_id: current,
                    request: PendingRequest {
                        id: request_id,
                        value: store.blob_json(&request)?,
                        digest,
                    },
                    step_id: suspended.step_id,
                    attempt: suspended.attempt,
                }));
            }
            HoldEvent::Child { run_id } => current = run_id,
        }
    }
}

/// One row of the run listing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub skill: String,
    pub runner: String,
    pub state: RunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disposition: Option<ClosureDisposition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Which runs to list. Text filters match case-insensitively.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunFilter {
    /// A substring of the run id or the skill.
    pub query: Option<String>,
    /// A substring of the skill.
    pub skill: Option<String>,
    /// `running`, `paused` or `sealed`, or a sealed run's disposition.
    pub state: Option<String>,
    /// Runs started at or after this RFC 3339 instant.
    pub since: Option<String>,
    /// Runs started at or before this RFC 3339 instant.
    pub until: Option<String>,
    pub limit: Option<usize>,
}

pub const RUN_LISTING_PROJECTOR_ID: &str = "runx-runtime.run-record.v1";
const DEFAULT_RUN_LIMIT: usize = 50;
const RUN_PAGE: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunListing {
    pub projector_id: String,
    pub runs: Vec<RunSummary>,
}

/// The workspace's top-level runs, most recently advanced first. A page reads
/// only each run's first and latest events, and stops at the first run that
/// last moved before `since`.
pub fn list_runs(
    env: &std::collections::BTreeMap<String, String>,
    cwd: &std::path::Path,
    filter: &RunFilter,
) -> Result<RunListing, RunRecordError> {
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd))?;
    let lower = |value: &Option<String>| {
        value
            .as_ref()
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty())
    };
    let instant = |field: &'static str, value: &Option<String>| {
        value
            .as_ref()
            .map(|value| {
                crate::receipts::archive::Timestamp::parse(value.trim()).ok_or_else(|| {
                    RunRecordError::InvalidFilter {
                        field,
                        value: value.clone(),
                    }
                })
            })
            .transpose()
    };
    let query = lower(&filter.query);
    let skill = lower(&filter.skill);
    let state = lower(&filter.state);
    let since = instant("since", &filter.since)?;
    let until = instant("until", &filter.until)?;
    let limit = filter.limit.unwrap_or(DEFAULT_RUN_LIMIT);
    let mut runs = Vec::new();
    let mut cursor: Option<Head> = None;
    while runs.len() < limit {
        let heads = store.recent_heads::<RunEvent>(cursor.as_ref(), RUN_PAGE)?;
        let exhausted = heads.len() < RUN_PAGE;
        for head in heads {
            cursor = Some(head.clone());
            if since.is_some_and(|since| {
                crate::receipts::archive::Timestamp::parse(&head.updated_at)
                    .is_some_and(|updated| updated < since)
            }) {
                return Ok(listing(runs));
            }
            // Child runs share the kind; they are listed through their parent.
            if head.key.contains('/') {
                continue;
            }
            let Some(summary) = summarize(&store, &head)? else {
                continue;
            };
            let started = crate::receipts::archive::Timestamp::parse(&summary.created_at);
            let matches = query.as_ref().is_none_or(|query| {
                summary.run_id.to_lowercase().contains(query)
                    || summary.skill.to_lowercase().contains(query)
            }) && skill
                .as_ref()
                .is_none_or(|skill| summary.skill.to_lowercase().contains(skill))
                && state.as_ref().is_none_or(|state| {
                    summary.state.label() == state
                        || summary
                            .disposition
                            .as_ref()
                            .is_some_and(|disposition| disposition.label() == state)
                })
                && since.is_none_or(|since| started.is_some_and(|started| started >= since))
                && until.is_none_or(|until| started.is_some_and(|started| started <= until));
            if matches {
                runs.push(summary);
                if runs.len() == limit {
                    break;
                }
            }
        }
        if exhausted {
            break;
        }
    }
    Ok(listing(runs))
}

fn listing(runs: Vec<RunSummary>) -> RunListing {
    RunListing {
        projector_id: RUN_LISTING_PROJECTOR_ID.to_owned(),
        runs,
    }
}

/// The request a paused run in this workspace waits on.
pub fn paused_request(
    env: &std::collections::BTreeMap<String, String>,
    cwd: &std::path::Path,
    run_id: &str,
) -> Result<Option<PendingRequest>, RunRecordError> {
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd))?;
    pending_request(&store, validate_run_id(run_id)?)
}

/// A paused run: what `runx resume` reloads, and the request it waits on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PausedRun {
    pub run_id: String,
    pub skill: String,
    pub skill_ref: String,
    pub runner: String,
    pub package_digest: String,
    pub execution_closure_digest: String,
    pub request: PendingRequest,
}

pub fn paused_run(
    env: &std::collections::BTreeMap<String, String>,
    cwd: &std::path::Path,
    run_id: &str,
) -> Result<Option<PausedRun>, RunRecordError> {
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd))?;
    let run_id = validate_run_id(run_id)?;
    let Some(request) = pending_request(&store, run_id)? else {
        return Ok(None);
    };
    let Some(first) = store.first::<RunEvent>(run_id)? else {
        return Ok(None);
    };
    let RunEvent::Started(started) = first.event else {
        return Ok(None);
    };
    Ok(Some(PausedRun {
        run_id: run_id.to_owned(),
        skill: started.skill,
        skill_ref: started.skill_ref,
        runner: started.runner,
        package_digest: started.package_digest,
        execution_closure_digest: started.execution_closure_digest,
        request,
    }))
}

/// A sealed run in this workspace: what ran, under which inputs, and the
/// graph receipt it sealed.
#[derive(Clone, Debug, PartialEq)]
pub struct SealedRun {
    pub runner: String,
    pub package_digest: String,
    pub inputs: JsonObject,
    pub receipt: Receipt,
}

pub fn sealed_run(
    env: &std::collections::BTreeMap<String, String>,
    cwd: &std::path::Path,
    run_id: &str,
) -> Result<Option<SealedRun>, RunRecordError> {
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd))?;
    let Some(record) = RunLog::open(store, validate_run_id(run_id)?)?.load()? else {
        return Ok(None);
    };
    let (Origin::Run(started), Some(sealed)) = (record.origin, record.sealed) else {
        return Ok(None);
    };
    Ok(Some(SealedRun {
        runner: started.runner,
        package_digest: started.package_digest,
        inputs: record.inputs,
        receipt: sealed.receipt,
    }))
}

// Function rationale: a listing row reads only the run's first and latest
// events, so a page costs the same however long its runs are.
fn summarize(store: &KernelStore, head: &Head) -> Result<Option<RunSummary>, RunRecordError> {
    let Some(first) = store.first::<RunEvent>(&head.key)? else {
        return Ok(None);
    };
    let RunEvent::Started(started) = first.event else {
        return Ok(None);
    };
    let Some(latest) = store.latest::<RunEvent>(&head.key)? else {
        return Ok(None);
    };
    let (state, disposition, receipt_id, pending_request_id) = match latest.event {
        RunEvent::Sealed(sealed) => (
            RunState::Sealed,
            Some(sealed.disposition),
            Some(sealed.receipt_id),
            None,
        ),
        RunEvent::Suspended(_) => (
            RunState::Paused,
            None,
            None,
            pending_request(store, &head.key)?.map(|request| request.id),
        ),
        RunEvent::Call(call) => (RunState::Paused, None, None, Some(call.turn.request_id)),
        _ => (RunState::Running, None, None, None),
    };
    Ok(Some(RunSummary {
        run_id: head.key.clone(),
        skill: started.skill,
        runner: started.runner,
        state,
        disposition,
        pending_request_id,
        receipt_id,
        created_at: started.created_at,
        updated_at: head.updated_at.clone(),
    }))
}

/// A started evaluation run in a store of its own, for unit tests.
#[cfg(test)]
pub(crate) fn scratch_log(run_id: &str) -> (tempfile::TempDir, RunLog) {
    let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
    let log = KernelStore::open(root.path())
        .map_err(RunRecordError::from)
        .and_then(|store| {
            RunLog::evaluation(
                store,
                run_id,
                "unit-test",
                "unit-test",
                "2026-07-24T00:00:00Z",
            )
        })
        .unwrap_or_else(|error| unreachable!("scratch run log: {error}"));
    (root, log)
}

#[cfg(test)]
mod tests;
