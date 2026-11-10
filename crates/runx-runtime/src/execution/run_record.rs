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
//! `<parent>/<step>`, that names its parent step. A test evaluation is a
//! run of its own that is never resumed or listed.
//!
//! While a run waits on a turn, each call the agent makes is a `call`
//! event naming the turn it belongs to. Calls never change where the run
//! waits: a run whose latest event is a call is still paused at its turn. A
//! call that runs a child skill links that child run; the child's own stream
//! records its progress.
//!
//! Everything a run waits on has an address: its request id at the root, and
//! `<path>#<id>` below it, the path being the child run's place under the
//! root. A paused run waits on every open request in its tree.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

pub use runx_contracts::RunState;
use runx_contracts::{
    CallAct, CallTarget, ContextEntry, JsonObject, JsonValue, Receipt, ResolutionRequest,
    RunCallOutput, RunDriver, RunListEntry, RunOutcome, StandingApproval, UnreadableRun,
};
use runx_core::state_machine::StepAdmissionWitness;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::engine::{StepOutcome, StepRun};
use crate::kernel::authority::RunAuthority;
use crate::kernel::custody::Settlement;
use crate::kernel::store::{
    Append, Blob, Expected, Head, KernelStore, KernelStoreError, Recorded, StreamEvent,
};

mod claim;
mod driver;
mod read;

pub(crate) use claim::retained_claim;
pub use driver::{DriverLock, driven};
pub use read::read_run;

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
    Clock(Box<ClockEvent>),
    Consumed(Box<ConsumedEvent>),
    Sealed(Box<SealedEvent>),
    Answered(Box<AnsweredEvent>),
    Standing(Box<StandingEvent>),
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
            Self::Clock(_) => "clock",
            Self::Consumed(_) => "consumed",
            Self::Sealed(_) => "sealed",
            Self::Answered(_) => ANSWERED,
            Self::Standing(_) => "standing",
        }
    }
}

/// The event type of an answer a resume admitted.
const ANSWERED: &str = "answered";

/// The person's standing approvals, replacing the run's set for every later
/// step.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StandingEvent {
    approvals: Vec<StandingApproval>,
}

/// What a top-level run is bound to. A pinned run id is a submission
/// identity: a continuation under it must present the same identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Started {
    pub skill: String,
    /// The path the skill was loaded from, for `runx resume`.
    pub skill_path: String,
    pub method: String,
    pub package_digest: String,
    pub closure_digest: String,
    /// Blob: the run's inputs.
    pub inputs: String,
    /// Blob: the caller's current and historical context, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_authority: Option<RunAuthority>,
    /// The act its invoker stated, which the run's steps read as `$run.act`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<CallAct>,
    /// Who works the run's turns, fixed when it starts.
    pub driver: RunDriver,
    /// The operator context the run was prepared with, held for every pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<crate::execution::prepared_skill::PreparedContext>,
    /// The registry reference the run buys, when its method is one the
    /// registry sells: the kernel buys it rather than run it (S10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase: Option<String>,
    /// What started it: the source-tree digest each of its binaries reports.
    /// A resume under another is refused.
    pub implementation: runx_contracts::RunImplementation,
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
    /// The act the parent's step gave the child, read as its `$run.act`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<CallAct>,
    /// What started it, decided from its own method.
    pub implementation: runx_contracts::RunImplementation,
    pub created_at: String,
}

/// A test evaluation: a fixture run against its target, never resumed.
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
    method: Option<String>,
    contract: JsonObject,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    artifact_outputs: BTreeSet<String>,
    outcome: StepOutcome,
    admission_witness: StepAdmissionWitness,
    /// Where what the step made may be shared, as it ran: a resumed run reads
    /// it here and never derives it again from what holds now.
    labels: runx_contracts::ArtifactLabels,
    /// What the step was given, as one digest, when it could be made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_digest: Option<String>,
    /// The credential the step's operation call presented, when it could be
    /// recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential_fingerprint: Option<String>,
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
    /// The answers a claim consumed: the acknowledgement that runs a claimed
    /// step again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    answers: Vec<AnswerEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HoldEvent {
    /// The step asked its host and is waiting for the answer.
    Request {
        request_id: String,
        request: String,
        digest: String,
        /// At a step that makes calls, an agent's turn or a script, its calls
        /// that still wait, so what the step waits on is read from its hold
        /// alone.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        waiting: Vec<WaitingCallEvent>,
    },
    /// The step's child run holds the request.
    Child { run_id: String },
    /// The step has not started: it waits for a bounded retry, which the
    /// driver that reaches it after `due_at` runs.
    Retry {
        request_id: String,
        request: String,
        digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        due_at: Option<String>,
    },
    /// The step reaches the world and was claimed before it ran. Until its
    /// step event follows it may have run, so the run waits on this retry
    /// rather than running it again unseen.
    Claimed {
        request_id: String,
        request: String,
        digest: String,
    },
}

/// A call a step made: an agent during its turn, or a script. A later event
/// with the same index settles a call that was pending.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallEvent {
    step_id: String,
    attempt: u32,
    /// The request the turn answers, so the run's latest event still names
    /// what it waits on. A script's calls belong to no turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn: Option<TurnRequestEvent>,
    index: u32,
    /// The id its host pinned on the call, unique within its turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
    target: CallTarget,
    /// Blob: the call's input.
    input: String,
    outcome: CallOutcomeEvent,
    /// The standing approval that admitted it on the approval floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    standing: Option<StandingApproval>,
    /// The answers the call consumed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    answers: Vec<AnswerEvent>,
    /// Its host cancelled it; its outcome is how it settled.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    cancelled: bool,
    /// The most a field of its result held inline when it reached a script:
    /// each field past it reached the script by reference to its artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inline_bound: Option<u64>,
    /// What it asked of reuse and what it was answered, recorded once so
    /// recovery reads the same choice and never decides again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reuse: Option<runx_contracts::RunCallReuse>,
    /// The reuse identity of a call that asked, on each of its events, so the
    /// child it runs is indexed under it when it settles, after a resume too.
    /// Never in the run's read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reuse_identity: Option<String>,
    /// A turn's call leaves its run paused at the turn: the turn's calls that
    /// still wait once it is recorded, itself among them while it waits, in
    /// order, so what the turn waits on is read from its latest call alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    waiting: Vec<WaitingCallEvent>,
}

/// A call of a step that still waits, and on what, as its latest event left
/// it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case", deny_unknown_fields)]
enum WaitingCallEvent {
    /// Its own ask, approval or wait, or the retry of a claim that never
    /// settled.
    Request {
        index: u32,
        request_id: String,
        /// Blob: the request.
        request: String,
        digest: String,
    },
    /// What the child run it started waits on.
    Child { index: u32, run_id: String },
}

impl WaitingCallEvent {
    /// What call `index`, settled by `outcome`, waits on; nothing once it
    /// has ended.
    fn of(index: u32, outcome: &CallOutcomeEvent) -> Option<Self> {
        match outcome {
            CallOutcomeEvent::Pending {
                request_id,
                request,
                digest,
            }
            | CallOutcomeEvent::Claimed {
                request_id,
                request,
                digest,
            } => Some(Self::Request {
                index,
                request_id: request_id.clone(),
                request: request.clone(),
                digest: digest.clone(),
            }),
            CallOutcomeEvent::Child { run_id } => Some(Self::Child {
                index,
                run_id: run_id.clone(),
            }),
            // A safe read in flight waits on nothing: it reruns by itself.
            CallOutcomeEvent::Executing
            | CallOutcomeEvent::Completed { .. }
            | CallOutcomeEvent::Refused { .. }
            | CallOutcomeEvent::Declined { .. }
            | CallOutcomeEvent::Failed { .. }
            | CallOutcomeEvent::Cancelled
            | CallOutcomeEvent::Waited { .. }
            | CallOutcomeEvent::Read { .. }
            | CallOutcomeEvent::TooLarge { .. } => None,
        }
    }
}

/// The answers call `index` of `step_id`'s `attempt` used, on its run's own
/// stream, where the call itself is on its step's: what the run consumed,
/// read without its calls.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsumedEvent {
    step_id: String,
    attempt: u32,
    index: u32,
    answers: Vec<AnswerEvent>,
}

/// A script read the clock: what `ctx.now()` returned at its position in its
/// step, so replay returns the same.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockEvent {
    step_id: String,
    attempt: u32,
    index: u32,
    /// Milliseconds since the epoch.
    millis: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnRequestEvent {
    request_id: String,
    /// Blob: the turn's request.
    request: String,
    digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CallOutcomeEvent {
    /// The call ran as a step: blobs of its record and receipts, and its
    /// output as the project artifacts a host reads it from.
    Completed {
        record: String,
        receipt: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        nested: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        outputs: Vec<RunCallOutput>,
    },
    /// Admission refused the call; nothing ran.
    Refused { reason: String },
    /// A person declined its approval; nothing ran.
    Declined { reason: String },
    /// The call could not run.
    Failed { reason: String },
    /// The call waits on its own ask or approval.
    Pending {
        request_id: String,
        request: String,
        digest: String,
    },
    /// The call was claimed and has not settled, so its effect may have
    /// happened: it waits on its retry.
    Claimed {
        request_id: String,
        request: String,
        digest: String,
    },
    /// A safe read was claimed and has not settled: a crash that leaves it
    /// so reruns it with nobody asked (D33).
    Executing,
    /// The call runs a child skill as `run_id`, and waits on what that run
    /// waits on.
    Child { run_id: String },
    /// Its host cancelled it before it started, or while it only read:
    /// nothing it did is in effect.
    Cancelled,
    /// A script's wait settled at `at`, in milliseconds since the epoch.
    Waited { at: i64 },
    /// A script's read settled with the content of `artifact`.
    Read {
        artifact: Box<runx_contracts::Artifact>,
    },
    /// A script's read of a value of `size_bytes`, past the worker's input
    /// `bound`, which it refused.
    TooLarge { size_bytes: u64, bound: u64 },
}

/// An answer a step consumed, bound to the request it answered.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerEvent {
    request_id: String,
    request_digest: String,
    /// Blob: the answer.
    value: String,
    human: bool,
    /// The turn's `input_digest`, when the answer is an agent turn's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_digest: Option<String>,
    /// The type of the approval gate the answer decided, when it answered
    /// an approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gate: Option<String>,
}

/// A person's decision a resume admitted, in the stream of the run that
/// raised its request, before any step used it. The first later step, call or claim of
/// that stream that consumes the same request uses it; a later admission to
/// the same request replaces it until then.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnsweredEvent {
    /// The address of the request answered.
    request_id: String,
    request_digest: String,
    /// Blob: the answer.
    value: String,
    human: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedEvent {
    outcome: RunOutcome,
    /// The run's receipt.
    receipt_id: String,
    /// Blob: the run's receipt.
    receipt: String,
    /// Blob: the public result of a run that stopped short of completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_result: Option<String>,
}

/// The caller's context, stored as one blob.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallerContext {
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
    /// An agent turn's `input_digest`: what the turn asked, beside the
    /// address and run its request names.
    pub input_digest: Option<String>,
    /// The type of the approval gate the answer decided, when it answered
    /// an approval, so a person's approval to spend on a read is told from
    /// any other answer.
    pub gate: Option<String>,
}

/// Where a suspended run waits.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Hold {
    Request(PendingRequest),
    Child(String),
    /// A bounded retry: runx runs the step again itself once it is due.
    Retry {
        request: PendingRequest,
        due_at: Option<String>,
    },
    /// A claimed step that never settled: it runs again only on the person's
    /// acknowledgement of its retry.
    Claimed(PendingRequest),
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
    pub call_id: Option<String>,
    pub target: CallTarget,
    pub input: JsonObject,
    pub standing: Option<StandingApproval>,
    pub outcome: CallOutcome,
    /// A completed call's output as project artifacts.
    pub outputs: Vec<RunCallOutput>,
    /// The place of the call's latest event among its step's call events, so
    /// its calls read back in the order they settled.
    pub order: u64,
    /// Its host cancelled it; its outcome is how it settled.
    pub cancelled: bool,
    /// The most a field of its result held inline when it reached a script.
    pub inline_bound: Option<u64>,
    /// What it asked of reuse and what it was answered, from its first event.
    pub reuse: Option<runx_contracts::RunCallReuse>,
    /// The reuse identity its child is indexed under, when it asked.
    pub reuse_identity: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum CallOutcome {
    Completed(Box<StepRun>),
    Refused(String),
    /// A person declined its approval; nothing ran.
    Declined(String),
    Failed(String),
    /// Waits on its own ask or approval.
    Pending(PendingRequest),
    /// Claimed and not settled: waits on its retry.
    Claimed(PendingRequest),
    /// A safe read claimed and not settled, which waits on nothing: a crash
    /// that leaves it so reruns it with nobody asked (D33).
    Executing,
    /// Runs a child skill as this run, and waits on what it waits on.
    Child(String),
    /// Its host cancelled it before it started, or while it only read.
    Cancelled,
    /// A script's wait settled at this time, in milliseconds since the epoch.
    Waited(i64),
    /// A script's read settled with this artifact's content.
    Read(Box<runx_contracts::Artifact>),
    /// A script's read of a value of `size_bytes`, past the worker's input
    /// `bound`, which it refused.
    TooLarge {
        size_bytes: u64,
        bound: u64,
    },
}

/// What a call records: its turn, if a turn made it, its place in its step,
/// and how it ended.
pub(crate) struct CallRecord<'a> {
    pub step_id: &'a str,
    pub attempt: u32,
    pub turn: Option<&'a PendingRequest>,
    pub index: u32,
    pub call_id: Option<&'a str>,
    pub target: &'a CallTarget,
    pub input: &'a JsonObject,
    pub outcome: &'a CallOutcome,
    /// A completed call's output, persisted as project artifacts.
    pub outputs: &'a [RunCallOutput],
    /// The standing approval that admitted it on the approval floor.
    pub standing: Option<&'a StandingApproval>,
    pub answers: &'a [Answer],
    /// Its host cancelled it; `outcome` is how it settled.
    pub cancelled: bool,
    /// The most a field of its result holds inline as it reaches a script.
    pub inline_bound: Option<u64>,
    /// What it asked of reuse and what it was answered.
    pub reuse: Option<&'a runx_contracts::RunCallReuse>,
    /// The reuse identity its child is indexed under, when it asked.
    pub reuse_identity: Option<&'a str>,
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
    pub outcome: RunOutcome,
    pub receipt: Receipt,
    pub terminal_result: Option<JsonValue>,
}

/// A run rebuilt from its stream.
#[derive(Clone, Debug)]
pub(crate) struct RunRecord {
    pub origin: Origin,
    pub created_at: String,
    pub inputs: JsonObject,
    /// The act the run's invoker stated, read by its steps as `$run.act`.
    pub act: Option<CallAct>,
    pub context: CallerContext,
    pub facts: Vec<Replayed>,
    /// Where the run waits now, when its last step-level event is a
    /// suspension no later step has settled.
    pub suspension: Option<Suspension>,
    /// Every answer the run's steps and calls consumed, in order.
    pub answers: Vec<Answer>,
    /// The answers a resume admitted that no step, call or claim has used
    /// yet, in the order they were admitted.
    pub admitted: Vec<Answer>,
    /// The standing approvals the person last set, which replace the set the
    /// run started with.
    pub standing: Option<Vec<StandingApproval>>,
    /// How many times the run has waited for a bounded retry.
    pub retries: u32,
    pub sealed: Option<Sealed>,
}

impl RunRecord {
    /// Record the answers a step, call or claim consumed; each uses the
    /// earliest admitted answer to its request, if a resume admitted one.
    fn consume(
        &mut self,
        store: &KernelStore,
        answers: &[AnswerEvent],
    ) -> Result<(), RunRecordError> {
        for answer in answers {
            if let Some(position) = self.admitted.iter().position(|admitted| {
                admitted.request_id == answer.request_id
                    && admitted.request_digest == answer.request_digest
            }) {
                self.admitted.remove(position);
            }
            self.answers.push(Answer {
                request_id: answer.request_id.clone(),
                request_digest: answer.request_digest.clone(),
                value: store.blob_json(&answer.value)?,
                human: answer.human,
                input_digest: answer.input_digest.clone(),
                gate: answer.gate.clone(),
            });
        }
        Ok(())
    }
}

/// The events and blobs of the answers a step, call or claim consumed.
fn answer_events(answers: &[Answer]) -> Result<(Vec<AnswerEvent>, Vec<Blob>), RunRecordError> {
    let values = answers
        .iter()
        .map(|answer| Blob::json(&answer.value))
        .collect::<Result<Vec<_>, _>>()?;
    let events = answers
        .iter()
        .zip(&values)
        .map(|(answer, value)| answer_event(answer, value))
        .collect();
    Ok((events, values))
}

/// How `answer` is recorded, its value held in `value`.
fn answer_event(answer: &Answer, value: &Blob) -> AnswerEvent {
    AnswerEvent {
        request_id: answer.request_id.clone(),
        request_digest: answer.request_digest.clone(),
        value: value.digest().to_owned(),
        human: answer.human,
        input_digest: answer.input_digest.clone(),
        gate: answer.gate.clone(),
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
    #[error("run {run_id} cannot replace its original implementation: {reason}")]
    Implementation { run_id: String, reason: String },
    #[error("run {run_id} has an invalid record: {reason}")]
    Invalid { run_id: String, reason: String },
    #[error(
        "run id {run_id:?} is invalid: use letters, digits and _ - . : (at most 200 bytes), not beginning sha256:"
    )]
    InvalidId { run_id: String },
    #[error("the system random source is unavailable for a run id")]
    Entropy,
    #[error("invalid {field} filter '{value}': expected an RFC 3339 timestamp")]
    InvalidFilter { field: &'static str, value: String },
    #[error("run {run_id} is driven by another driver; this driver is refused")]
    Driven { run_id: String },
    #[error("run {run_id}'s driver lock is unavailable: {source}")]
    DriverLock {
        run_id: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the cursor belongs to run {cursor_run}, not {run_id}")]
    CursorRunMismatch { run_id: String, cursor_run: String },
    #[error("the cursor is damaged or names a version its run does not hold")]
    InvalidCursor,
    #[error("run {run_id} has no record in this workspace")]
    UnknownRun { run_id: String },
}

/// The writer of one run's stream.
#[derive(Clone)]
pub struct RunLog(Arc<RunLogInner>);

struct RunLogInner {
    store: Arc<KernelStore>,
    run_id: String,
    version: Mutex<u64>,
    /// Whether a run another writer advanced since this log last read it is
    /// contended; a log that only reads takes the run as it finds it.
    strict: bool,
    /// The act the run was started with, held once this log reads or records
    /// the run's start. A run's act never changes after it starts.
    act: OnceLock<Option<CallAct>>,
    /// What this log read of its run, so it reads again only what was
    /// committed since.
    kept: Mutex<Kept>,
}

/// The run as a log last took it, and each step's calls as it last read
/// them, each through the version it read to.
#[derive(Clone, Default)]
struct Kept {
    record: Option<(u64, RunRecord)>,
    steps: BTreeMap<(String, u32), KeptCalls>,
    /// The logs of the child runs this log opened, so each is read once.
    children: BTreeMap<String, RunLog>,
}

/// A step's calls, each as its latest event settled it, the calls that
/// still wait, and its script's clock reads, by position, as its own stream
/// holds them through `through`.
#[derive(Clone, Default)]
struct KeptCalls {
    through: u64,
    /// The stream's version as this log last wrote it, which a read takes up
    /// from `through`.
    written: u64,
    calls: Vec<RecordedCall>,
    waiting: BTreeMap<u32, WaitingCallEvent>,
    clock: BTreeMap<u32, i64>,
    /// The place the step's next call event takes among its call events.
    order: u64,
}

impl KeptCalls {
    /// The step's calls after `events`, its stream's next, which holds
    /// nothing but its calls and its script's clock reads.
    fn apply(
        &mut self,
        store: &KernelStore,
        stream: &str,
        events: Vec<Recorded<RunEvent>>,
    ) -> Result<(), RunRecordError> {
        for recorded in events {
            match recorded.event {
                RunEvent::Call(call) => {
                    match WaitingCallEvent::of(call.index, &call.outcome) {
                        Some(waits) => self.waiting.insert(call.index, waits),
                        None => self.waiting.remove(&call.index),
                    };
                    settle_call(&mut self.calls, recorded_call(store, *call, self.order)?);
                    self.order += 1;
                }
                RunEvent::Clock(read) => {
                    self.clock.insert(read.index, read.millis);
                }
                _ => {
                    return Err(RunRecordError::Invalid {
                        run_id: stream.to_owned(),
                        reason: "a step's calls stream holds something other than its calls"
                            .to_owned(),
                    });
                }
            }
            self.through = recorded.version;
        }
        Ok(())
    }

    /// The calls by index, and the clock reads.
    fn read(&self) -> (Vec<RecordedCall>, BTreeMap<u32, i64>) {
        let mut calls = self.calls.clone();
        calls.sort_by_key(|call| call.index);
        (calls, self.clock.clone())
    }
}

/// The key of the stream that holds the calls of step `step_id`'s
/// `attempt` in run `run_id`, beside the run's own stream and in its tree.
/// No run id or step id holds an `@`, so it names no run.
pub(crate) fn calls_key(run_id: &str, step_id: &str, attempt: u32) -> String {
    format!("{run_id}/{step_id}@{attempt}")
}

/// Whether `stream` holds the calls of one of run `run_id`'s steps.
#[must_use]
pub fn is_calls_stream_of(stream: &str, run_id: &str) -> bool {
    stream
        .strip_prefix(run_id)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|step| !step.contains('/') && step.contains('@'))
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
            strict: true,
            act: OnceLock::new(),
            kept: Mutex::default(),
        })))
    }

    /// This run's log for a pass that writes nothing of the run's own: it
    /// reads the run as it finds it, while the run's driver records. Its
    /// writes are still refused against a version it did not read.
    #[must_use]
    pub(crate) fn reading(&self) -> Self {
        // What it read is taken first, as `load` takes it before the version,
        // and each guard is released before the next is taken.
        let kept = self.kept().clone();
        let version = *self.version();
        Self(Arc::new(RunLogInner {
            store: Arc::clone(&self.0.store),
            run_id: self.0.run_id.clone(),
            version: Mutex::new(version),
            strict: false,
            act: self.0.act.clone(),
            kept: Mutex::new(kept),
        }))
    }

    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.0.run_id
    }

    /// The address of `request_id`, a request this run raises.
    pub(crate) fn address(&self, request_id: &str) -> String {
        request_address(&self.0.run_id, request_id)
    }

    #[must_use]
    pub(crate) fn store(&self) -> &Arc<KernelStore> {
        &self.0.store
    }

    /// How many child runs deep this run is below its root: one per step
    /// on its path, a call's index belonging to the step that made it.
    pub(crate) fn depth(&self) -> u32 {
        let depth = self
            .0
            .run_id
            .split('/')
            .skip(1)
            .filter(|segment| segment.starts_with(|first: char| first.is_ascii_alphabetic()))
            .count();
        u32::try_from(depth).unwrap_or(u32::MAX)
    }

    /// The id of the child run a nested skill step runs as.
    pub(crate) fn child_id(&self, step_id: &str) -> String {
        format!("{}/{step_id}", self.0.run_id)
    }

    /// The log of the child run that a nested skill step runs as: the one
    /// this log opened before, so what it read of the child it reads again
    /// only from there.
    pub(crate) fn child(&self, step_id: &str) -> Result<Self, RunRecordError> {
        let child_id = self.child_id(step_id);
        if let Some(child) = self.kept().children.get(&child_id) {
            return Ok(child.clone());
        }
        let child = Self::open(Arc::clone(&self.0.store), child_id.clone())?;
        Ok(self
            .kept()
            .children
            .entry(child_id)
            .or_insert(child)
            .clone())
    }

    /// The log of `run_id`, this run or one below it, from this log's child
    /// logs, so what a driver read of that run it reads again only from
    /// where it read it to.
    pub(crate) fn log_of(&self, run_id: &str) -> Result<Self, RunRecordError> {
        if run_id == self.0.run_id {
            return Ok(self.clone());
        }
        let below = run_id
            .strip_prefix(self.0.run_id.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
            .ok_or_else(|| RunRecordError::Invalid {
                run_id: self.0.run_id.clone(),
                reason: format!("{run_id} is no run below it"),
            })?;
        let holding = self
            .kept()
            .children
            .iter()
            .find(|(child, _)| {
                run_id == child.as_str()
                    || run_id
                        .strip_prefix(child.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|(_, log)| log.clone());
        match holding {
            Some(child) => child.log_of(run_id),
            None => self.child(below),
        }
    }

    /// The run as a continuation finds it while it waits: what started it
    /// and what it waits on, from the record this log took; `None` when it
    /// waits on nothing or is a child run, which continues with its parent.
    pub(crate) fn paused(&self) -> Result<Option<PausedRun>, RunRecordError> {
        let Some(record) = self.load()? else {
            return Ok(None);
        };
        if record.sealed.is_some() || record.suspension.is_none() {
            return Ok(None);
        }
        let Origin::Run(started) = record.origin else {
            return Ok(None);
        };
        Ok(Some(PausedRun {
            run_id: self.0.run_id.clone(),
            skill: started.skill,
            skill_path: started.skill_path,
            method: started.method,
            package_digest: started.package_digest,
            closure_digest: started.closure_digest,
            purchase: started.purchase,
            waiting: self.waiting()?,
        }))
    }

    /// Everything the run waits on, at any depth, each request once: its
    /// hold, the calls of its held step that still wait, and what each child
    /// run they wait on waits on, from what this log and its child logs read,
    /// so a driver reads each stream of its run once.
    pub(crate) fn waiting(&self) -> Result<Vec<PendingRequest>, RunRecordError> {
        let mut waiting = Vec::new();
        self.collect_waiting(&mut waiting)?;
        Ok(waiting)
    }

    fn collect_waiting(&self, waiting: &mut Vec<PendingRequest>) -> Result<(), RunRecordError> {
        let Some(record) = self.load()? else {
            return Ok(());
        };
        let Some(suspension) = record.suspension.filter(|_| record.sealed.is_none()) else {
            return Ok(());
        };
        let request = match suspension.hold {
            Hold::Child(run_id) => return self.log_of(&run_id)?.collect_waiting(waiting),
            // A step waiting on a retry or a claim has made no call that waits.
            Hold::Retry { request, .. } | Hold::Claimed(request) => {
                push_waiting(waiting, request);
                return Ok(());
            }
            Hold::Request(request) => request,
        };
        push_waiting(waiting, request);
        for call in self
            .waiting_calls(&suspension.step_id, suspension.attempt)?
            .into_values()
        {
            match call {
                WaitingCallEvent::Request {
                    request_id,
                    request,
                    digest,
                    ..
                } => push_waiting(
                    waiting,
                    PendingRequest {
                        id: request_id,
                        value: self.0.store.blob_json(&request)?,
                        digest,
                    },
                ),
                WaitingCallEvent::Child { run_id, .. } => {
                    self.log_of(&run_id)?.collect_waiting(waiting)?;
                }
            }
        }
        Ok(())
    }

    /// When the run was admitted, as the event that opened its stream records
    /// it; none before the run is recorded. What a run's steps derive from
    /// its admission is dated by it, so a step driven again asks the same.
    pub(crate) fn admitted_at(&self) -> Result<Option<String>, RunRecordError> {
        // A log that took its run holds it already.
        if let Some((_, record)) = &self.kept().record {
            return Ok(Some(record.created_at.clone()));
        }
        Ok(self
            .0
            .store
            .first::<RunEvent>(&self.0.run_id)?
            .and_then(|first| match first.event {
                RunEvent::Started(started) => Some(started.created_at),
                RunEvent::ChildStarted(started) => Some(started.created_at),
                RunEvent::EvaluationStarted(started) => Some(started.created_at),
                _ => None,
            }))
    }

    /// Rebuild the run from its stream; `None` when it has not started. A
    /// log that took its run once reads only what was committed since.
    pub(crate) fn load(&self) -> Result<Option<RunRecord>, RunRecordError> {
        let mut kept = self.kept();
        if let Some((through, record)) = kept.record.as_mut() {
            let newer = self
                .0
                .store
                .read_all_after::<RunEvent>(&self.0.run_id, *through)?;
            let mut held = self.version();
            for recorded in newer {
                record.apply(&self.0.store, &self.0.run_id, recorded.event)?;
                *through = recorded.version;
            }
            *held = *through;
            return Ok(Some(record.clone()));
        }
        crate::work_counts::count(crate::work_counts::Work::RunMaterialization);
        let events = self.0.store.read::<RunEvent>(&self.0.run_id)?;
        let version = events.last().map_or(0, |event| event.version);
        *self.version() = version;
        let record = replay(
            &self.0.store,
            &self.0.run_id,
            events.into_iter().map(|event| event.event),
        )?;
        if let Some(record) = &record {
            self.keep_act(record.act.as_ref());
            kept.record = Some((version, record.clone()));
        }
        Ok(record)
    }

    /// The act the run was started with, as its steps read it: held from
    /// the run's start, or read once from the event that opened its stream.
    pub(crate) fn act(&self) -> Result<Option<CallAct>, RunRecordError> {
        if let Some(act) = self.0.act.get() {
            return Ok(act.clone());
        }
        let act = self
            .0
            .store
            .first::<RunEvent>(&self.0.run_id)?
            .and_then(|first| match first.event {
                RunEvent::Started(started) => started.act,
                RunEvent::ChildStarted(started) => started.act,
                _ => None,
            });
        self.keep_act(act.as_ref());
        Ok(act)
    }

    fn keep_act(&self, act: Option<&CallAct>) {
        let _first_kept = self.0.act.set(act.cloned());
    }

    fn kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.0
            .kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The calls of one turn, in order, as this driver last saw the run. A
    /// run another driver has advanced since is contended.
    pub(crate) fn calls(
        &self,
        step_id: &str,
        attempt: u32,
    ) -> Result<Vec<RecordedCall>, RunRecordError> {
        Ok(self.step_calls(step_id, attempt)?.0)
    }

    /// A step's calls, in order, and the clock reads its script recorded, by
    /// position, as this driver last saw the run. A run another driver has
    /// advanced since is contended.
    pub(crate) fn step_calls(
        &self,
        step_id: &str,
        attempt: u32,
    ) -> Result<(Vec<RecordedCall>, BTreeMap<u32, i64>), RunRecordError> {
        if self.0.strict {
            let contended = || RunRecordError::Contended {
                run_id: self.0.run_id.clone(),
            };
            if self.0.store.version::<RunEvent>(&self.0.run_id)? != *self.version() {
                return Err(contended());
            }
            // Calls another writer made since this log read the step's.
            let known = self
                .kept()
                .steps
                .get(&(step_id.to_owned(), attempt))
                .map(|calls| calls.through.max(calls.written));
            if let Some(known) = known
                && self
                    .0
                    .store
                    .version::<RunEvent>(&calls_key(&self.0.run_id, step_id, attempt))?
                    != known
            {
                return Err(contended());
            }
        }
        self.kept_calls(step_id, attempt, KeptCalls::read)
    }

    /// The calls of `step_id`'s `attempt` as its own stream holds them, given
    /// to `read`: read whole the first time this log reads them, and then only
    /// from where it read them to, so a step's calls are read once and the
    /// run's own stream holds none of them.
    fn kept_calls<R>(
        &self,
        step_id: &str,
        attempt: u32,
        read: impl FnOnce(&KeptCalls) -> R,
    ) -> Result<R, RunRecordError> {
        let key = calls_key(&self.0.run_id, step_id, attempt);
        let mut kept = self.kept();
        let calls = kept.steps.entry((step_id.to_owned(), attempt)).or_default();
        let newer = self
            .0
            .store
            .read_all_after::<RunEvent>(&key, calls.through)?;
        calls.apply(&self.0.store, &key, newer)?;
        Ok(read(calls))
    }

    /// Record what a script's `ctx.now()` read at position `index` of its
    /// step: `millis` since the epoch.
    pub(crate) fn record_clock(
        &self,
        step_id: &str,
        attempt: u32,
        index: u32,
        millis: i64,
    ) -> Result<(), RunRecordError> {
        self.append_call(
            (step_id, attempt),
            RunEvent::Clock(Box::new(ClockEvent {
                step_id: step_id.to_owned(),
                attempt,
                index,
                millis,
            })),
            Vec::new(),
            None,
            None,
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
            skill_path: started.skill_path.to_owned(),
            method: started.method.to_owned(),
            package_digest: started.package_digest.to_owned(),
            closure_digest: started.closure_digest.to_owned(),
            inputs: inputs.digest().to_owned(),
            context: context.as_ref().map(|blob| blob.digest().to_owned()),
            run_authority: started.run_authority.cloned(),
            act: started.act.cloned(),
            driver: started.driver,
            prepared: started.prepared.cloned(),
            purchase: started.purchase.map(str::to_owned),
            implementation: started.implementation.clone(),
            created_at: started.created_at.to_owned(),
        };
        let mut blobs = vec![inputs];
        blobs.extend(context);
        self.append_first(RunEvent::Started(Box::new(event)), blobs)?;
        self.keep_act(started.act);
        Ok(())
    }

    /// Start a test evaluation: the run a fixture replays against its
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
        log.keep_act(None);
        Ok(log)
    }

    /// Start, or continue, the child run a nested skill step runs as.
    pub(crate) fn start_child(
        &self,
        started: ChildStart<'_>,
    ) -> Result<Option<RunRecord>, RunRecordError> {
        let ChildStart {
            parent,
            step_id,
            skill,
            inputs,
            act,
            implementation,
            must_match,
            created_at,
        } = started;
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
            if recorded.act.as_ref() != act {
                return Err(self.mismatch("act"));
            }
            if let Some(reason) = must_match
                .then(|| crate::implementation::refusal(&recorded.implementation, implementation))
                .flatten()
            {
                return Err(RunRecordError::Implementation {
                    run_id: self.0.run_id.clone(),
                    reason,
                });
            }
            return Ok(Some(existing));
        }
        let event = ChildStarted {
            parent: parent.to_owned(),
            step_id: step_id.to_owned(),
            skill: skill.to_owned(),
            inputs: inputs.digest().to_owned(),
            act: act.cloned(),
            implementation: implementation.clone(),
            created_at: created_at.to_owned(),
        };
        self.append_first(RunEvent::ChildStarted(Box::new(event)), vec![inputs])?;
        self.keep_act(act);
        Ok(None)
    }

    pub(crate) fn record_step(
        &self,
        step: &StepRun,
        answers: &[Answer],
    ) -> Result<(), RunRecordError> {
        let record = Blob::json(&RecordedStep {
            skill: step.skill.clone(),
            method: step.method.clone(),
            contract: step.contract.clone(),
            artifact_outputs: step.artifact_outputs.clone(),
            outcome: step.outcome.clone(),
            admission_witness: step.admission_witness.clone(),
            labels: step.labels,
            input_digest: step.input_digest.clone(),
            credential_fingerprint: step.credential_fingerprint.clone(),
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
            nested: nested
                .iter()
                .map(|blob| blob.digest().to_owned())
                .chain(step.recorded_nested.iter().cloned())
                .collect(),
            answers: answers
                .iter()
                .zip(&values)
                .map(|(answer, value)| answer_event(answer, value))
                .collect(),
        };
        let mut blobs = vec![record, receipt];
        blobs.extend(nested);
        blobs.extend(values);
        self.append_settling(
            RunEvent::Step(Box::new(event)),
            blobs,
            step.settlement.as_ref(),
        )
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
        let waiting = self
            .waiting_calls(step_id, attempt)?
            .into_values()
            .collect();
        self.append(
            RunEvent::Suspended(Box::new(SuspendedEvent {
                step_id: step_id.to_owned(),
                attempt,
                hold: HoldEvent::Request {
                    request_id,
                    request: blob.digest().to_owned(),
                    digest,
                    waiting,
                },
                answers: Vec::new(),
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
                answers: Vec::new(),
            })),
            Vec::new(),
        )
    }

    /// The step reaches the world: claim it before it runs, under the version
    /// this driver last read, so a second driver is refused before anything
    /// runs and a step that never settles waits on `retry`. A claim that
    /// runs a claimed step again records the acknowledgement it consumed.
    pub(crate) fn record_claim(
        &self,
        step_id: &str,
        attempt: u32,
        retry: &ResolutionRequest,
        answers: &[Answer],
    ) -> Result<(), RunRecordError> {
        let (request_id, blob, digest) = request_blob(retry)?;
        let (answers, values) = answer_events(answers)?;
        self.append(
            RunEvent::Suspended(Box::new(SuspendedEvent {
                step_id: step_id.to_owned(),
                attempt,
                hold: HoldEvent::Claimed {
                    request_id,
                    request: blob.digest().to_owned(),
                    digest,
                },
                answers,
            })),
            std::iter::once(blob).chain(values).collect(),
        )
    }

    /// The step has not started; it waits for a bounded retry, due
    /// `retry_after_seconds` after `now`.
    pub(crate) fn record_retry(
        &self,
        step_id: &str,
        request: &ResolutionRequest,
        now: std::time::SystemTime,
    ) -> Result<(), RunRecordError> {
        let (request_id, blob, digest) = request_blob(request)?;
        let delay = match request {
            ResolutionRequest::Retry {
                retry_after_seconds,
                ..
            } => *retry_after_seconds,
            _ => 0,
        };
        let due = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(delay);
        self.append(
            RunEvent::Suspended(Box::new(SuspendedEvent {
                step_id: step_id.to_owned(),
                attempt: 0,
                hold: HoldEvent::Retry {
                    request_id,
                    request: blob.digest().to_owned(),
                    digest,
                    due_at: Some(crate::time::iso8601_from_unix_seconds(
                        i64::try_from(due).unwrap_or(i64::MAX),
                    )),
                },
                answers: Vec::new(),
            })),
            vec![blob],
        )
    }

    /// An answer a resume admits, recorded before the run goes further, so
    /// the next driver to reach its request uses it even after a crash.
    pub(crate) fn record_answer(&self, answer: &Answer) -> Result<(), RunRecordError> {
        let value = Blob::json(&answer.value)?;
        self.append(
            RunEvent::Answered(Box::new(AnsweredEvent {
                request_id: answer.request_id.clone(),
                request_digest: answer.request_digest.clone(),
                value: value.digest().to_owned(),
                human: answer.human,
            })),
            vec![value],
        )
    }

    /// The person's new standing approvals, recorded before the run goes
    /// further.
    pub(crate) fn record_standing(
        &self,
        approvals: &[StandingApproval],
    ) -> Result<(), RunRecordError> {
        self.append(
            RunEvent::Standing(Box::new(StandingEvent {
                approvals: approvals.to_vec(),
            })),
            Vec::new(),
        )
    }

    /// Every answer a resume admitted in this run's tree that no step, call
    /// or claim has used yet.
    pub(crate) fn admitted(&self) -> Result<Vec<Answer>, RunRecordError> {
        let mut admitted = Vec::new();
        for key in self
            .0
            .store
            .tree_keys_with::<RunEvent>(&self.0.run_id, ANSWERED)?
        {
            if let Some(record) = self.log_of(&key)?.load()? {
                admitted.extend(record.admitted);
            }
        }
        Ok(admitted)
    }

    /// A call an agent made during its turn, with the answers it consumed.
    pub(crate) fn record_call(&self, call: &CallRecord<'_>) -> Result<(), RunRecordError> {
        let input = Blob::json(call.input)?;
        let turn = call.turn.map(|turn| Blob::json(&turn.value)).transpose()?;
        let turn_event = call
            .turn
            .zip(turn.as_ref())
            .map(|(turn, blob)| TurnRequestEvent {
                request_id: turn.id.clone(),
                request: blob.digest().to_owned(),
                digest: turn.digest.clone(),
            });
        let input_digest = input.digest().to_owned();
        let mut blobs = vec![input];
        blobs.extend(turn);
        let settlement = match call.outcome {
            CallOutcome::Completed(step) => step.settlement.as_ref(),
            _ => None,
        };
        let outcome = match call.outcome {
            CallOutcome::Completed(step) => {
                let record = Blob::json(&RecordedStep {
                    skill: step.skill.clone(),
                    method: step.method.clone(),
                    contract: step.contract.clone(),
                    artifact_outputs: step.artifact_outputs.clone(),
                    outcome: step.outcome.clone(),
                    admission_witness: step.admission_witness.clone(),
                    labels: step.labels,
                    input_digest: step.input_digest.clone(),
                    credential_fingerprint: step.credential_fingerprint.clone(),
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
                    nested: nested
                        .iter()
                        .map(|blob| blob.digest().to_owned())
                        .chain(step.recorded_nested.iter().cloned())
                        .collect(),
                    outputs: call.outputs.to_vec(),
                };
                blobs.extend([record, receipt]);
                blobs.extend(nested);
                outcome
            }
            CallOutcome::Refused(reason) => CallOutcomeEvent::Refused {
                reason: reason.clone(),
            },
            CallOutcome::Declined(reason) => CallOutcomeEvent::Declined {
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
            CallOutcome::Claimed(retry) => {
                let request = Blob::json(&retry.value)?;
                let outcome = CallOutcomeEvent::Claimed {
                    request_id: retry.id.clone(),
                    request: request.digest().to_owned(),
                    digest: retry.digest.clone(),
                };
                blobs.push(request);
                outcome
            }
            CallOutcome::Executing => CallOutcomeEvent::Executing,
            CallOutcome::Child(run_id) => CallOutcomeEvent::Child {
                run_id: run_id.clone(),
            },
            CallOutcome::Cancelled => CallOutcomeEvent::Cancelled,
            CallOutcome::Waited(at) => CallOutcomeEvent::Waited { at: *at },
            CallOutcome::Read(artifact) => CallOutcomeEvent::Read {
                artifact: artifact.clone(),
            },
            CallOutcome::TooLarge { size_bytes, bound } => CallOutcomeEvent::TooLarge {
                size_bytes: *size_bytes,
                bound: *bound,
            },
        };
        let values = call
            .answers
            .iter()
            .map(|answer| Blob::json(&answer.value))
            .collect::<Result<Vec<_>, _>>()?;
        // A turn's call is where its run stands, so it names what the turn
        // waits on; a script stands on its hold, which names it there.
        let waiting = match &turn_event {
            Some(_) => {
                let mut waiting = self.waiting_calls(call.step_id, call.attempt)?;
                match WaitingCallEvent::of(call.index, &outcome) {
                    Some(waits) => waiting.insert(call.index, waits),
                    None => waiting.remove(&call.index),
                };
                waiting.into_values().collect()
            }
            None => Vec::new(),
        };
        let event = CallEvent {
            step_id: call.step_id.to_owned(),
            attempt: call.attempt,
            turn: turn_event,
            index: call.index,
            call_id: call.call_id.map(str::to_owned),
            target: call.target.clone(),
            input: input_digest,
            outcome,
            standing: call.standing.cloned(),
            answers: call
                .answers
                .iter()
                .zip(&values)
                .map(|(answer, value)| answer_event(answer, value))
                .collect(),
            cancelled: call.cancelled,
            inline_bound: call.inline_bound,
            reuse: call.reuse.cloned(),
            reuse_identity: call.reuse_identity.map(str::to_owned),
            waiting,
        };
        blobs.extend(values);
        // The answers a call used are the run's to know without reading its
        // calls, so the run records them on its own stream with the call.
        let consumed = (!event.answers.is_empty()).then(|| {
            RunEvent::Consumed(Box::new(ConsumedEvent {
                step_id: event.step_id.clone(),
                attempt: event.attempt,
                index: event.index,
                answers: event.answers.clone(),
            }))
        });
        self.append_call(
            (call.step_id, call.attempt),
            RunEvent::Call(Box::new(event)),
            blobs,
            consumed,
            settlement,
        )
    }

    /// The calls of the step at `step_id`'s `attempt` that still wait, by
    /// index, as its calls stream stands.
    fn waiting_calls(
        &self,
        step_id: &str,
        attempt: u32,
    ) -> Result<BTreeMap<u32, WaitingCallEvent>, RunRecordError> {
        self.kept_calls(step_id, attempt, |calls| calls.waiting.clone())
    }

    pub(crate) fn record_sealed(
        &self,
        receipt: &Receipt,
        terminal_result: Option<&JsonValue>,
    ) -> Result<(), RunRecordError> {
        let receipt_blob = Blob::json(receipt)?;
        let terminal_result = terminal_result.map(Blob::json).transpose()?;
        let event = SealedEvent {
            outcome: receipt.seal.outcome,
            receipt_id: receipt.id.to_string(),
            receipt: receipt_blob.digest().to_owned(),
            terminal_result: terminal_result
                .as_ref()
                .map(|blob| blob.digest().to_owned()),
        };
        let mut blobs = vec![receipt_blob];
        blobs.extend(terminal_result);
        self.append(RunEvent::Sealed(Box::new(event)), blobs)
    }

    /// A run's first event: the stream must not exist yet.
    fn append_first(&self, event: RunEvent, blobs: Vec<Blob>) -> Result<(), RunRecordError> {
        self.append_expected(Expected::Absent, event, blobs)
    }

    fn append(&self, event: RunEvent, blobs: Vec<Blob>) -> Result<(), RunRecordError> {
        self.append_settling(event, blobs, None)
    }

    /// Record `event`, and settle the effect it consumed in the same
    /// transaction.
    fn append_settling(
        &self,
        event: RunEvent,
        blobs: Vec<Blob>,
        settlement: Option<&Settlement>,
    ) -> Result<(), RunRecordError> {
        let expected = match *self.version() {
            0 => {
                return Err(RunRecordError::Invalid {
                    run_id: self.0.run_id.clone(),
                    reason: "the run was not started".to_owned(),
                });
            }
            version => Expected::Version(version),
        };
        let settles = match settlement {
            Some(settlement) => {
                settlement
                    .appends(&self.0.store)
                    .map_err(|error| RunRecordError::Invalid {
                        run_id: self.0.run_id.clone(),
                        reason: format!("settling the step's effect: {error}"),
                    })?
            }
            None => Vec::new(),
        };
        self.commit(expected, event, blobs, settles)
    }

    fn append_expected(
        &self,
        expected: Expected,
        event: RunEvent,
        blobs: Vec<Blob>,
    ) -> Result<(), RunRecordError> {
        self.commit(expected, event, blobs, Vec::new())
    }

    fn commit(
        &self,
        expected: Expected,
        event: RunEvent,
        blobs: Vec<Blob>,
        settles: Vec<Append>,
    ) -> Result<(), RunRecordError> {
        let mut version = self.version();
        let append = Append::kernel(self.0.run_id.clone(), expected, &event)?.with_blobs(blobs);
        let mut batch = vec![append];
        batch.extend(settles);
        match self.0.store.append(batch) {
            Ok(versions) => {
                let before = *version;
                *version = versions.first().copied().unwrap_or(before + 1);
                Ok(())
            }
            Err(KernelStoreError::Conflict { .. }) => Err(RunRecordError::Contended {
                run_id: self.0.run_id.clone(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    /// Record `event`, a call or a clock read of `step_id`'s `attempt`, on the
    /// step's calls stream, with what the run records of it on its own
    /// stream, and the effect it settled, in one transaction.
    fn append_call(
        &self,
        (step_id, attempt): (&str, u32),
        event: RunEvent,
        blobs: Vec<Blob>,
        run_event: Option<RunEvent>,
        settlement: Option<&Settlement>,
    ) -> Result<(), RunRecordError> {
        let key = calls_key(&self.0.run_id, step_id, attempt);
        let step = (step_id.to_owned(), attempt);
        let known = self
            .kept()
            .steps
            .get(&step)
            .map(|calls| calls.through.max(calls.written));
        let stream_version = match known {
            Some(version) => version,
            None => self.0.store.version::<RunEvent>(&key)?,
        };
        let expected = match stream_version {
            0 => Expected::Absent,
            version => Expected::Version(version),
        };
        let mut batch = vec![Append::kernel(key, expected, &event)?.with_blobs(blobs)];
        if let Some(settlement) = settlement {
            batch.extend(settlement.appends(&self.0.store).map_err(|error| {
                RunRecordError::Invalid {
                    run_id: self.0.run_id.clone(),
                    reason: format!("settling the step's effect: {error}"),
                }
            })?);
        }
        let mut version = self.version();
        if *version == 0 {
            return Err(RunRecordError::Invalid {
                run_id: self.0.run_id.clone(),
                reason: "the run was not started".to_owned(),
            });
        }
        let run_position = batch.len();
        if let Some(run_event) = &run_event {
            batch.push(Append::kernel(
                self.0.run_id.clone(),
                Expected::Version(*version),
                run_event,
            )?);
        }
        match self.0.store.append(batch) {
            Ok(versions) => {
                if run_event.is_some() {
                    *version = versions.get(run_position).copied().unwrap_or(*version + 1);
                }
                drop(version);
                let written = versions.first().copied().unwrap_or(stream_version + 1);
                self.kept().steps.entry(step).or_default().written = written;
                Ok(())
            }
            Err(KernelStoreError::Conflict { .. }) => Err(RunRecordError::Contended {
                run_id: self.0.run_id.clone(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    /// Whether the run still stands where a step waits, a hold on a request
    /// or a child, as the record this log took holds it. A claim doesn't.
    pub(crate) fn stands_waiting(&self) -> Result<bool, RunRecordError> {
        Ok(self.load()?.is_some_and(|record| {
            record.sealed.is_none()
                && record.suspension.is_some_and(|suspension| {
                    matches!(suspension.hold, Hold::Request(_) | Hold::Child(_))
                })
        }))
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
    pub skill_path: &'a str,
    pub method: &'a str,
    pub package_digest: &'a str,
    pub closure_digest: &'a str,
    pub inputs: &'a JsonObject,
    pub context: &'a CallerContext,
    pub run_authority: Option<&'a RunAuthority>,
    pub act: Option<&'a CallAct>,
    pub driver: RunDriver,
    pub prepared: Option<&'a crate::execution::prepared_skill::PreparedContext>,
    pub purchase: Option<&'a str>,
    /// The implementation it runs on.
    pub implementation: &'a runx_contracts::RunImplementation,
    pub created_at: &'a str,
}

/// What a child run is started with, by its parent's step.
pub(crate) struct ChildStart<'a> {
    pub parent: &'a str,
    pub step_id: &'a str,
    pub skill: &'a str,
    pub inputs: &'a JsonObject,
    pub act: Option<&'a CallAct>,
    /// The implementation it runs on, decided from its own method, which a
    /// new child records.
    pub implementation: &'a runx_contracts::RunImplementation,
    /// Whether an existing child must still be on the implementation it
    /// recorded: not when it runs nothing now, sealed already or refused as
    /// it starts.
    pub must_match: bool,
    pub created_at: &'a str,
}

/// The longest run id a caller may pin.
const MAX_RUN_ID_BYTES: usize = 200;

/// A caller-pinned run id: letters, digits and `_ - . :`, so it can never
/// name a child run or escape a key space, and never beginning `sha256:`, so
/// `runx history` never reads it as a receipt id.
pub(crate) fn validate_run_id(run_id: &str) -> Result<&str, RunRecordError> {
    let valid = !run_id.is_empty()
        && !run_id.starts_with("sha256:")
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

/// The digest a turn call's input is known by: `sha256:` over its canonical
/// JSON, which a host recomputes from the input it sent.
pub(crate) fn input_digest(input: &JsonObject) -> Result<String, RunRecordError> {
    runx_contracts::canonical_digest(input)
        .map_err(|error| KernelStoreError::Codec(error.to_string()).into())
}

/// Where a run stands from its latest event: its state, and a sealed run's
/// outcome and receipt id.
fn standing(latest: &RunEvent) -> (RunState, Option<RunOutcome>, Option<String>) {
    match latest {
        RunEvent::Sealed(sealed) => (
            RunState::Sealed,
            Some(sealed.outcome),
            Some(sealed.receipt_id.clone()),
        ),
        RunEvent::Suspended(_) | RunEvent::Answered(_) | RunEvent::Standing(_) => {
            (RunState::Paused, None, None)
        }
        // A turn's call leaves its run paused at the turn; a script's call
        // is its step still running.
        _ => (RunState::Running, None, None),
    }
}

/// The latest event that says where a stream stands: its last, stepping
/// back past the answers and standing approvals a person gave while it
/// waited, which never move it.
fn standing_event(
    store: &KernelStore,
    key: &str,
) -> Result<Option<Recorded<RunEvent>>, RunRecordError> {
    let Some(mut latest) = store.latest::<RunEvent>(key)? else {
        return Ok(None);
    };
    while matches!(
        latest.event,
        RunEvent::Answered(_) | RunEvent::Standing(_) | RunEvent::Consumed(_)
    ) {
        let earlier = match latest.version.checked_sub(2) {
            Some(after) => store
                .read_after::<RunEvent>(key, after, 1)?
                .into_iter()
                .next(),
            None => None,
        };
        let Some(earlier) = earlier else {
            break;
        };
        latest = earlier;
    }
    Ok(Some(latest))
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
        | ResolutionRequest::Turn { id, .. }
        | ResolutionRequest::Retry { id, .. }
        | ResolutionRequest::Purchase { id, .. }
        | ResolutionRequest::Wait { id, .. } => id.to_string(),
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
    let mut events = events.into_iter();
    let Some(first) = events.next() else {
        return Ok(None);
    };
    let mut record = RunRecord::begin(store, run_id, first)?;
    for event in events {
        record.apply(store, run_id, event)?;
    }
    Ok(Some(record))
}

impl RunRecord {
    /// The run as the event that opened its stream starts it.
    fn begin(store: &KernelStore, run_id: &str, first: RunEvent) -> Result<Self, RunRecordError> {
        let invalid = |reason: &str| RunRecordError::Invalid {
            run_id: run_id.to_owned(),
            reason: reason.to_owned(),
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
                    CallerContext::default(),
                    created_at,
                )
            }
            RunEvent::EvaluationStarted(started) => {
                let created_at = started.created_at.clone();
                (
                    Origin::Evaluation(started),
                    None,
                    CallerContext::default(),
                    created_at,
                )
            }
            _ => return Err(invalid("the stream does not begin with its start")),
        };
        let act = match &origin {
            Origin::Run(started) => started.act.clone(),
            Origin::Child(started) => started.act.clone(),
            Origin::Evaluation(_) => None,
        };
        Ok(Self {
            origin,
            created_at,
            act,
            inputs: inputs_digest
                .map(|digest| store.blob_json(&digest))
                .transpose()?
                .unwrap_or_default(),
            context,
            facts: Vec::new(),
            suspension: None,
            answers: Vec::new(),
            admitted: Vec::new(),
            standing: None,
            retries: 0,
            sealed: None,
        })
    }

    /// The run after `event`, the next its stream holds. A call's input and
    /// result are its step's to read, so the run reads only the answers it
    /// consumed.
    fn apply(
        &mut self,
        store: &KernelStore,
        run_id: &str,
        event: RunEvent,
    ) -> Result<(), RunRecordError> {
        let invalid = |reason: &str| RunRecordError::Invalid {
            run_id: run_id.to_owned(),
            reason: reason.to_owned(),
        };
        let record = self;
        if record.sealed.is_some() {
            return Err(invalid("an event follows the seal"));
        }
        match event {
            RunEvent::Started(_) | RunEvent::ChildStarted(_) | RunEvent::EvaluationStarted(_) => {
                return Err(invalid("the run starts twice"));
            }
            RunEvent::Step(step) => {
                record.suspension = None;
                record.consume(store, &step.answers)?;
                record.facts.push(Replayed::Step(Box::new(step_run(
                    store,
                    *step,
                    Nested::AtTheSeal,
                )?)));
            }
            RunEvent::Skipped(skipped) => {
                record.suspension = None;
                record.facts.push(Replayed::Skipped(skipped.step_ids));
            }
            RunEvent::Suspended(suspended) => {
                record.consume(store, &suspended.answers)?;
                let hold = match suspended.hold {
                    HoldEvent::Request {
                        request_id,
                        request,
                        digest,
                        ..
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
                        due_at,
                    } => {
                        record.retries = record.retries.saturating_add(1);
                        Hold::Retry {
                            request: PendingRequest {
                                id: request_id,
                                value: store.blob_json(&request)?,
                                digest,
                            },
                            due_at,
                        }
                    }
                    // A claim no step event settled may have run: it waits on
                    // its retry until it settles.
                    HoldEvent::Claimed {
                        request_id,
                        request,
                        digest,
                    } => Hold::Claimed(PendingRequest {
                        id: request_id,
                        value: store.blob_json(&request)?,
                        digest,
                    }),
                };
                record.suspension = Some(Suspension {
                    step_id: suspended.step_id,
                    attempt: suspended.attempt,
                    hold,
                });
            }
            RunEvent::Consumed(consumed) => record.consume(store, &consumed.answers)?,
            RunEvent::Call(_) | RunEvent::Clock(_) => {
                return Err(invalid("a call is recorded on its step's calls stream"));
            }
            // A new standing set leaves the run where it waits.
            RunEvent::Standing(standing) => record.standing = Some(standing.approvals),
            // An admitted answer leaves the run where it waits, and replaces
            // one admitted to the same request that no step used.
            RunEvent::Answered(answered) => {
                record.admitted.retain(|admitted| {
                    admitted.request_id != answered.request_id
                        || admitted.request_digest != answered.request_digest
                });
                record.admitted.push(Answer {
                    request_id: answered.request_id,
                    request_digest: answered.request_digest,
                    value: store.blob_json(&answered.value)?,
                    human: answered.human,
                    input_digest: None,
                    gate: None,
                });
            }
            RunEvent::Sealed(sealed) => {
                record.suspension = None;
                record.sealed = Some(Sealed {
                    outcome: sealed.outcome,
                    receipt: store.blob_json(&sealed.receipt)?,
                    terminal_result: sealed
                        .terminal_result
                        .map(|digest| store.blob_json(&digest))
                        .transpose()?,
                });
            }
        }
        Ok(())
    }
}

/// A later event for a call's index settles the call.
/// Take `recorded` as its call's latest event among `calls`.
pub(crate) fn settle_call(calls: &mut Vec<RecordedCall>, recorded: RecordedCall) {
    match calls.iter_mut().find(|known| {
        known.step_id == recorded.step_id
            && known.attempt == recorded.attempt
            && known.index == recorded.index
    }) {
        // What it asked of reuse is on its first event alone.
        Some(known) => {
            let reuse = known.reuse.take();
            *known = RecordedCall {
                reuse: recorded.reuse.clone().or(reuse),
                ..recorded
            };
        }
        None => calls.push(recorded),
    }
}

fn recorded_call(
    store: &KernelStore,
    call: CallEvent,
    order: u64,
) -> Result<RecordedCall, RunRecordError> {
    Ok(RecordedCall {
        order,
        outcome: call_outcome(store, &call)?,
        input: store.blob_json(&call.input)?,
        step_id: call.step_id,
        attempt: call.attempt,
        index: call.index,
        call_id: call.call_id,
        target: call.target,
        standing: call.standing,
        outputs: match &call.outcome {
            CallOutcomeEvent::Completed { outputs, .. } => outputs.clone(),
            _ => Vec::new(),
        },
        cancelled: call.cancelled,
        inline_bound: call.inline_bound,
        reuse: call.reuse,
        reuse_identity: call.reuse_identity,
    })
}

fn call_outcome(store: &KernelStore, call: &CallEvent) -> Result<CallOutcome, RunRecordError> {
    Ok(match &call.outcome {
        CallOutcomeEvent::Completed {
            record,
            receipt,
            nested,
            ..
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
            Nested::Read,
        )?)),
        CallOutcomeEvent::Refused { reason } => CallOutcome::Refused(reason.clone()),
        CallOutcomeEvent::Declined { reason } => CallOutcome::Declined(reason.clone()),
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
        CallOutcomeEvent::Claimed {
            request_id,
            request,
            digest,
        } => CallOutcome::Claimed(PendingRequest {
            id: request_id.clone(),
            value: store.blob_json(request)?,
            digest: digest.clone(),
        }),
        CallOutcomeEvent::Executing => CallOutcome::Executing,
        CallOutcomeEvent::Child { run_id } => CallOutcome::Child(run_id.clone()),
        CallOutcomeEvent::Cancelled => CallOutcome::Cancelled,
        CallOutcomeEvent::Waited { at } => CallOutcome::Waited(*at),
        CallOutcomeEvent::Read { artifact } => CallOutcome::Read(artifact.clone()),
        CallOutcomeEvent::TooLarge { size_bytes, bound } => CallOutcome::TooLarge {
            size_bytes: *size_bytes,
            bound: *bound,
        },
    })
}

/// The address of `request_id`, a request run `run_id` raises: the id itself
/// at the root; below it, `<path>#<id>`, the path being the run's place under
/// the root. A root run id holds no `/`, and no step id or request id holds
/// `#`, so two open requests share an address only when they are the same
/// request: an operation call made twice with the same intent.
pub(crate) fn request_address(run_id: &str, request_id: &str) -> String {
    match run_id.split_once('/') {
        Some((_, path)) => format!("{path}#{request_id}"),
        None => request_id.to_owned(),
    }
}

/// The logical root shared by a run and all its child runs.
pub(crate) fn root_run_id(run_id: &str) -> &str {
    run_id.split_once('/').map_or(run_id, |(root, _)| root)
}

/// The id `address` has in its own run: what follows its `#`, or the whole
/// address at the root.
pub(crate) fn local_request_id(address: &str) -> &str {
    address
        .split_once('#')
        .map_or(address, |(_, request_id)| request_id)
}

/// The step a call runs as: `<turn step>/<index>`, so a child run it starts
/// is `<run>/<turn step>/<index>`.
pub(crate) fn call_step_id(turn_step: &str, index: u32) -> String {
    format!("{turn_step}/{index}")
}

/// How a step's run reads the receipts below it.
#[derive(Clone, Copy)]
enum Nested {
    /// Each one, as a call's run is read, with its step.
    Read,
    /// None until its run seals, as a replayed step's.
    AtTheSeal,
}

fn step_run(
    store: &KernelStore,
    event: StepEvent,
    nested: Nested,
) -> Result<StepRun, RunRecordError> {
    let record: RecordedStep = store.blob_json(&event.record)?;
    let (nested_receipts, recorded_nested) = match nested {
        Nested::Read => (
            event
                .nested
                .iter()
                .map(|digest| store.blob_json(digest))
                .collect::<Result<_, _>>()?,
            Vec::new(),
        ),
        Nested::AtTheSeal => (Vec::new(), event.nested),
    };
    Ok(StepRun {
        step_id: event.step_id,
        attempt: event.attempt,
        skill: record.skill,
        method: record.method,
        contract: record.contract,
        artifact_outputs: record.artifact_outputs,
        ephemeral_contract: Default::default(),
        outcome: record.outcome,
        receipt: store.blob_json(&event.receipt)?,
        nested_receipts,
        recorded_nested,
        refusal: None,
        admission_witness: record.admission_witness,
        settlement: None,
        labels: record.labels,
        input_digest: record.input_digest,
        credential_fingerprint: record.credential_fingerprint,
    })
}

/// Every request a paused run waits on, each by its address: where its step
/// is held, following the child runs workflow steps hold; and at an agent turn,
/// what each call of the turn waits on, following the child runs skill calls
/// started. Empty unless the run is paused: once a host has answered, the
/// run waits on its own continuation, not on the host.
pub(crate) fn waiting_requests(
    store: &KernelStore,
    run_id: &str,
) -> Result<Vec<PendingRequest>, RunRecordError> {
    let mut waiting = Vec::new();
    collect_waiting(store, run_id, &mut waiting)?;
    Ok(waiting)
}

fn collect_waiting(
    store: &KernelStore,
    run_id: &str,
    waiting: &mut Vec<PendingRequest>,
) -> Result<(), RunRecordError> {
    let Some(held) = held(store, run_id)? else {
        return Ok(());
    };
    push_waiting(waiting, held.request);
    // A step that makes calls, an agent's turn or a script, waits on every
    // call of its that waits, each listed once, as its hold or its turn's
    // latest call names them.
    for call in held.waiting {
        match call {
            WaitingCallEvent::Request {
                request_id,
                request,
                digest,
                ..
            } => push_waiting(
                waiting,
                PendingRequest {
                    id: request_id,
                    value: store.blob_json(&request)?,
                    digest,
                },
            ),
            WaitingCallEvent::Child { run_id, .. } => collect_waiting(store, &run_id, waiting)?,
        }
    }
    Ok(())
}

/// List `request` once: a step held on one of its calls' requests names it
/// twice, as its hold and as its call's.
fn push_waiting(waiting: &mut Vec<PendingRequest>, request: PendingRequest) {
    if !waiting.iter().any(|known| known.id == request.id) {
        waiting.push(request);
    }
}

/// Where a run's step is held: the request it waits on, and at a step that
/// makes calls, those that still wait.
struct Held {
    request: PendingRequest,
    waiting: Vec<WaitingCallEvent>,
}

/// Where `run_id` is held, following the child runs workflow steps hold down to
/// the step that asked.
fn held(store: &KernelStore, run_id: &str) -> Result<Option<Held>, RunRecordError> {
    let mut current = run_id.to_owned();
    loop {
        let Some(latest) = standing_event(store, &current)? else {
            return Ok(None);
        };
        let RunEvent::Suspended(suspended) = latest.event else {
            return Ok(None);
        };
        let suspended = *suspended;
        // A step waiting on a retry or a claim has made no call that waits.
        let (request_id, request, digest, waiting) = match suspended.hold {
            HoldEvent::Request {
                request_id,
                request,
                digest,
                waiting,
            } => {
                // A turn's calls come after its hold, and its latest names
                // what the turn waits on now; a script's name nothing, since
                // its hold is recorded again after them.
                let calls = calls_key(&current, &suspended.step_id, suspended.attempt);
                let waiting = match store.latest::<RunEvent>(&calls)?.map(|latest| latest.event) {
                    Some(RunEvent::Call(call))
                        if call
                            .turn
                            .as_ref()
                            .is_some_and(|turn| turn.request_id == request_id) =>
                    {
                        call.waiting
                    }
                    _ => waiting,
                };
                (request_id, request, digest, waiting)
            }
            HoldEvent::Retry {
                request_id,
                request,
                digest,
                ..
            }
            | HoldEvent::Claimed {
                request_id,
                request,
                digest,
            } => (request_id, request, digest, Vec::new()),
            HoldEvent::Child { run_id } => {
                current = run_id;
                continue;
            }
        };
        return Ok(Some(Held {
            request: PendingRequest {
                id: request_id,
                value: store.blob_json(&request)?,
                digest,
            },
            waiting,
        }));
    }
}

/// Which runs to list. Text filters match case-insensitively.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunFilter {
    /// A substring of the run id or the skill.
    pub query: Option<String>,
    /// A substring of the skill.
    pub skill: Option<String>,
    /// `running`, `paused` or `sealed`, or a sealed run's outcome.
    pub state: Option<String>,
    /// Runs started at or after this RFC 3339 instant.
    pub since: Option<String>,
    /// Runs started at or before this RFC 3339 instant.
    pub until: Option<String>,
    pub limit: Option<usize>,
}

impl RunFilter {
    /// Whether anything beyond the query narrows the listing.
    #[must_use]
    pub fn has_list_filters(&self) -> bool {
        self.skill.is_some()
            || self.state.is_some()
            || self.since.is_some()
            || self.until.is_some()
            || self.limit.is_some()
    }
}

pub const RUN_LISTING_PROJECTOR_ID: &str = "runx-runtime.run-record.v1";
const DEFAULT_RUN_LIMIT: usize = 50;
const RUN_PAGE: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunListing {
    pub projector_id: String,
    pub runs: Vec<RunListEntry>,
    /// The runs whose record this runx can't read, each by its id and why.
    pub unreadable: Vec<UnreadableRun>,
}

/// The workspace's top-level runs, most recently advanced first. A page reads
/// only each run's first and latest events, and stops at the first run that
/// last moved before `since`. A run whose record can't be read, such as one
/// another runx version recorded, is listed as unreadable, and the listing
/// carries on: the skill, state and time filters can't judge it, so only a
/// query on its id selects it, and it counts toward the limit.
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
    let mut unreadable = Vec::new();
    let mut cursor: Option<Head> = None;
    while runs.len() + unreadable.len() < limit {
        let heads = store.recent_heads::<RunEvent>(cursor.as_ref(), RUN_PAGE)?;
        let exhausted = heads.len() < RUN_PAGE;
        for head in heads {
            cursor = Some(head.clone());
            if since.is_some_and(|since| {
                crate::receipts::archive::Timestamp::parse(&head.updated_at)
                    .is_some_and(|updated| updated < since)
            }) {
                return Ok(listing(runs, unreadable));
            }
            // Child runs share the kind; they are listed through their parent.
            if head.key.contains('/') {
                continue;
            }
            let summary = match summarize(&store, &head) {
                Ok(Some(summary)) => summary,
                Ok(None) => continue,
                Err(error) if unreadable_record(&error) => {
                    let selected = skill.is_none()
                        && state.is_none()
                        && since.is_none()
                        && until.is_none()
                        && query
                            .as_ref()
                            .is_none_or(|query| head.key.to_lowercase().contains(query));
                    if selected {
                        unreadable.push(UnreadableRun {
                            run_id: head.key.as_str().into(),
                            updated_at: head.updated_at.as_str().into(),
                            reason: error.to_string().into(),
                        });
                        if runs.len() + unreadable.len() == limit {
                            break;
                        }
                    }
                    continue;
                }
                Err(error) => return Err(error),
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
                            .outcome
                            .as_ref()
                            .is_some_and(|outcome| outcome.label() == state)
                })
                && since.is_none_or(|since| started.is_some_and(|started| started >= since))
                && until.is_none_or(|until| started.is_some_and(|started| started <= until));
            if matches {
                runs.push(summary);
                if runs.len() + unreadable.len() == limit {
                    break;
                }
            }
        }
        if exhausted {
            break;
        }
    }
    Ok(listing(runs, unreadable))
}

fn listing(runs: Vec<RunListEntry>, unreadable: Vec<UnreadableRun>) -> RunListing {
    RunListing {
        projector_id: RUN_LISTING_PROJECTOR_ID.to_owned(),
        runs,
        unreadable,
    }
}

/// Whether `error` is this one run's record, which the listing reports and
/// passes over: an event that doesn't decode, a broken chain or blob, or an
/// invalid record. Anything else is the store's, and stops the listing.
fn unreadable_record(error: &RunRecordError) -> bool {
    matches!(
        error,
        RunRecordError::Invalid { .. }
            | RunRecordError::Store(
                KernelStoreError::Codec(_)
                    | KernelStoreError::Integrity { .. }
                    | KernelStoreError::BlobIntegrity { .. }
            )
    )
}

/// A paused run: what `runx resume` reloads, and what it waits on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PausedRun {
    pub run_id: String,
    pub skill: String,
    pub skill_path: String,
    pub method: String,
    pub package_digest: String,
    pub closure_digest: String,
    /// The registry reference the run buys, when it is a purchase.
    pub purchase: Option<String>,
    pub waiting: Vec<PendingRequest>,
}

pub fn paused_run(
    env: &std::collections::BTreeMap<String, String>,
    cwd: &std::path::Path,
    run_id: &str,
) -> Result<Option<PausedRun>, RunRecordError> {
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd))?;
    let run_id = validate_run_id(run_id)?;
    // A run is paused when its own latest event leaves it waiting, even when
    // nothing it waits on needs a host: a child that sealed before its parent
    // step was recorded is taken further by the next continuation.
    let paused = standing_event(&store, run_id)?
        .is_some_and(|latest| standing(&latest.event).0 == RunState::Paused);
    if !paused {
        return Ok(None);
    }
    let Some(first) = store.first::<RunEvent>(run_id)? else {
        return Ok(None);
    };
    let RunEvent::Started(started) = first.event else {
        return Ok(None);
    };
    Ok(Some(PausedRun {
        run_id: run_id.to_owned(),
        skill: started.skill,
        skill_path: started.skill_path,
        method: started.method,
        package_digest: started.package_digest,
        closure_digest: started.closure_digest,
        purchase: started.purchase,
        waiting: waiting_requests(&store, run_id)?,
    }))
}

// Function rationale: a listing row reads only the run's first and latest
// events, so a page costs the same however long its runs are.
fn summarize(store: &KernelStore, head: &Head) -> Result<Option<RunListEntry>, RunRecordError> {
    let Some(first) = store.first::<RunEvent>(&head.key)? else {
        return Ok(None);
    };
    let RunEvent::Started(started) = first.event else {
        return Ok(None);
    };
    let Some(latest) = standing_event(store, &head.key)? else {
        return Ok(None);
    };
    let (state, outcome, receipt_id) = standing(&latest.event);
    let pending_request_id = match latest.event {
        RunEvent::Suspended(_) => held(store, &head.key)?.map(|held| held.request.id),
        _ => None,
    };
    Ok(Some(RunListEntry {
        run_id: head.key.as_str().into(),
        skill: started.skill.into(),
        method: started.method.into(),
        state,
        outcome,
        pending_request_id: pending_request_id.map(Into::into),
        receipt_id: receipt_id.map(Into::into),
        created_at: started.created_at.into(),
        updated_at: head.updated_at.as_str().into(),
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
