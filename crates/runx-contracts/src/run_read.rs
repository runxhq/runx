//! One run as a host reads it: its summary, what it waits on, and its record
//! as events after a cursor. A host reconnects, recovers and shows progress
//! from this read alone, without a checkpoint reader or an output stream.
use serde::{Deserialize, Serialize};

use crate::decision::ClosureDisposition;
use crate::host_protocol::{ResolutionRequest, ResolutionResponseActor, TurnTarget};
use crate::schema::{NonEmptyString, RunxSchema};
use crate::skill_output::ProjectArtifactRef;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum RunReadSchema {
    #[serde(rename = "runx.run_read.v1")]
    V1,
}

/// `runx history <run-id> --json`.
///
/// Events come stream by stream, each stream in version order: a run and its
/// child runs are streams of their own, and the store has no order across
/// them. The cursor holds the last version returned for every stream of the
/// run's tree, so a read after it never skips or repeats an event, including
/// in child runs that started after the earlier read. `run` and `waiting` are
/// read after the events, so they are never older than the last event
/// returned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.run_read.v1")]
pub struct RunRead {
    pub schema: RunReadSchema,
    pub run: RunReadSummary,
    /// Every request the run waits on, in full, so a host can rebuild a
    /// person's decision after a crash.
    pub waiting: Vec<WaitedRequest>,
    pub events: Vec<RunEventRecord>,
    /// Opaque. Pass it as `--after` to read what follows.
    pub cursor: NonEmptyString,
    /// Whether events past this page exist now.
    pub more: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunReadSummary {
    pub run_id: NonEmptyString,
    pub skill: NonEmptyString,
    pub runner: NonEmptyString,
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<ClosureDisposition>,
    pub package_digest: NonEmptyString,
    pub execution_closure_digest: NonEmptyString,
    pub driver: RunDriver,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<NonEmptyString>,
    /// Whether a process holds the run's driver lock now. A host that
    /// restarts reconciles once this is false.
    pub driven: bool,
    pub created_at: NonEmptyString,
    pub updated_at: NonEmptyString,
}

/// Where a run stands, from its latest event alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Paused,
    Sealed,
}

impl RunState {
    /// The serde label, for listings and filters.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Sealed => "sealed",
        }
    }
}

/// Who works a run's agent turns, fixed when it starts: the host it is
/// attached to, or runx's managed agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunDriver {
    Attached,
    Managed,
}

/// A request the run waits on: its address, the digest an answer binds to,
/// and the request itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitedRequest {
    pub address: NonEmptyString,
    pub request_digest: NonEmptyString,
    pub request: ResolutionRequest,
}

/// One event of a run's record. It carries references and digests, never
/// outputs or secrets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.run_event.v1")]
pub struct RunEventRecord {
    /// The stream: the run id, or `<run>/<path>` for a child run.
    pub stream: NonEmptyString,
    pub version: u64,
    /// When the event committed, for display only: it orders nothing.
    pub committed_at: NonEmptyString,
    pub event: RunEventFact,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunEventFact {
    Started {
        skill: NonEmptyString,
        runner: NonEmptyString,
    },
    ChildStarted {
        parent: NonEmptyString,
        step_id: NonEmptyString,
        skill: NonEmptyString,
    },
    Step {
        step_id: NonEmptyString,
        attempt: u32,
        succeeded: bool,
        receipt_id: NonEmptyString,
    },
    Skipped {
        step_ids: Vec<NonEmptyString>,
    },
    Suspended {
        step_id: NonEmptyString,
        attempt: u32,
        hold: RunHold,
    },
    Call {
        step_id: NonEmptyString,
        attempt: u32,
        /// The address of the turn the call was made in.
        turn: NonEmptyString,
        index: u32,
        target: TurnTarget,
        input_digest: NonEmptyString,
        /// The id its host pinned on the call.
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<NonEmptyString>,
        /// The standing approval that admitted it on the approval floor,
        /// when one did.
        #[serde(skip_serializing_if = "Option::is_none")]
        standing: Option<crate::StandingApproval>,
        outcome: Box<RunCallOutcome>,
    },
    Sealed {
        disposition: ClosureDisposition,
        receipt_id: NonEmptyString,
    },
    /// The person replaced the run's standing approvals for every later
    /// step.
    Standing {
        approvals: Vec<crate::StandingApproval>,
    },
    /// A resume admitted a person's decision on the request at `address`,
    /// before it took the run further; the run's next driver to reach the
    /// request uses it.
    Answered {
        address: NonEmptyString,
        request_digest: NonEmptyString,
        /// `sha256:` over the answer's value.
        answer_digest: NonEmptyString,
        actor: ResolutionResponseActor,
    },
}

/// Where a suspended step waits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunHold {
    /// On a request it raised.
    Request {
        address: NonEmptyString,
        request_digest: NonEmptyString,
    },
    /// On a bounded retry; the step has not started.
    Retry {
        address: NonEmptyString,
        request_digest: NonEmptyString,
    },
    /// On its retry after it was claimed: it reaches the world, and its
    /// effect may have happened.
    Claimed {
        address: NonEmptyString,
        request_digest: NonEmptyString,
    },
    /// On what the child run it started waits on.
    Child { run_id: NonEmptyString },
}

/// How a call stood when this event recorded it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunCallOutcome {
    Completed {
        succeeded: bool,
        receipt_id: NonEmptyString,
        /// The call's output as project artifacts of its run: one per
        /// packet output, and one for the rest.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        outputs: Vec<RunCallOutput>,
        /// Why a call that ran did not succeed.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<NonEmptyString>,
    },
    /// Admission refused the call; nothing ran.
    Refused {
        reason: NonEmptyString,
    },
    /// The call could not run.
    Failed {
        reason: NonEmptyString,
    },
    Pending {
        address: NonEmptyString,
        request_digest: NonEmptyString,
    },
    /// Claimed and not settled: its effect may have happened.
    Claimed {
        address: NonEmptyString,
        request_digest: NonEmptyString,
    },
    Child {
        run_id: NonEmptyString,
    },
}

/// A project artifact holding part of a call's output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunCallOutput {
    /// The packet output it holds; absent on the artifact of the call's
    /// other outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<NonEmptyString>,
    pub artifact: ProjectArtifactRef,
}

#[cfg(test)]
mod tests {
    use super::{RunEventRecord, RunRead};

    #[test]
    fn a_read_round_trips_and_refuses_unknown_fields() -> Result<(), serde_json::Error> {
        let value = serde_json::json!({
            "schema": "runx.run_read.v1",
            "run": {
                "run_id": "run_1",
                "skill": "reply",
                "runner": "reply",
                "state": "paused",
                "package_digest": "sha256:p",
                "execution_closure_digest": "sha256:c",
                "driver": "attached",
                "driven": false,
                "created_at": "2026-10-14T00:00:00Z",
                "updated_at": "2026-10-14T00:00:01Z"
            },
            "waiting": [{
                "address": "send.approval",
                "request_digest": "sha256:r",
                "request": {"kind": "approval", "id": "send.approval", "gate": {"id": "send.approval", "reason": "send it"}}
            }],
            "events": [
                {"stream": "run_1", "version": 1, "committed_at": "2026-10-14T00:00:00Z", "event": {"kind": "started", "skill": "reply", "runner": "reply"}},
                {"stream": "run_1", "version": 2, "committed_at": "2026-10-14T00:00:01Z", "event": {"kind": "suspended", "step_id": "send", "attempt": 1, "hold": {"kind": "request", "address": "send.approval", "request_digest": "sha256:r"}}},
                {"stream": "run_1/draft", "version": 3, "committed_at": "2026-10-14T00:00:01Z", "event": {"kind": "call", "step_id": "draft", "attempt": 1, "turn": "draft", "index": 0, "target": {"kind": "ask"}, "input_digest": "sha256:i", "call_id": "toolu_1", "outcome": {"status": "refused", "reason": "ask is not in the turn's allowance"}}}
            ],
            "cursor": "c1",
            "more": false
        });
        let read: RunRead = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(read)?, value);
        assert!(
            serde_json::from_value::<RunEventRecord>(serde_json::json!({
                "stream": "run_1", "version": 1, "committed_at": "t",
                "event": {"kind": "sealed", "disposition": "closed", "receipt_id": "sha256:x", "output": {}}
            }))
            .is_err(),
            "an event never carries an output"
        );
        Ok(())
    }
}
