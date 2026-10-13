//! The open turns of a paused run, held for an agent that works them from
//! outside the run. The run is bound once; each action drives the run to the
//! turn it names, by address, and acts there, so a call runs at the agent
//! step, under its runtime, at whatever depth the step runs. A turn a call
//! opens in a child skill is reached the same way. One action runs at a
//! time.

use std::sync::{Mutex, PoisonError};

use runx_contracts::{JsonObject, ResolutionRequest, TurnCallResult, TurnStatus};

use super::graph::{RunHost, TurnAct, TurnActed, TurnAction};
use super::{BoundRun, SkillRunError, invalid};
use crate::RuntimeError;
use crate::execution::orchestrator::RunResult;

pub struct TurnSession {
    run: BoundRun,
    serial: Mutex<()>,
}

impl std::fmt::Debug for TurnSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TurnSession")
            .field("run", &self.run_id())
            .finish_non_exhaustive()
    }
}

impl TurnSession {
    /// Hold `run`'s open turns. The run must wait on one, and be one the host
    /// it is attached to drives.
    pub(crate) fn open(run: BoundRun) -> Result<Self, SkillRunError> {
        let session = Self {
            run,
            serial: Mutex::new(()),
        };
        let turn = session
            .waiting()?
            .into_iter()
            .find_map(|request| match request {
                ResolutionRequest::AgentAct { id, .. } => Some(id),
                _ => None,
            })
            .ok_or_else(|| invalid(format!("run {} has no open turn", session.run_id())))?;
        session.status(turn.as_str())?;
        Ok(session)
    }

    #[must_use]
    pub fn run_id(&self) -> &str {
        self.run.request.run_id.as_deref().unwrap_or_default()
    }

    /// Everything the run waits on, by address: each open turn's act, and
    /// each ask, approval or retry beneath them.
    pub fn waiting(&self) -> Result<Vec<ResolutionRequest>, SkillRunError> {
        self.run
            .waiting()?
            .iter()
            .map(|request| {
                serde_json::to_value(&request.value)
                    .and_then(serde_json::from_value)
                    .map_err(|source| {
                        RuntimeError::json("reading a pending request", source).into()
                    })
            })
            .collect()
    }

    /// The turn at `turn` as its agent reads it: its act, its tools and its
    /// calls.
    pub fn status(&self, turn: &str) -> Result<TurnStatus, SkillRunError> {
        match self.act(turn, TurnAction::Status)?.0 {
            TurnActed::Status(status) => Ok(*status),
            _ => Err(invalid("reading a turn did something else")),
        }
    }

    /// Make the call the turn at `turn` offers as `tool`; the run admits,
    /// executes and records it.
    pub fn call(
        &self,
        turn: &str,
        tool: &str,
        input: JsonObject,
    ) -> Result<TurnCallResult, SkillRunError> {
        let action = TurnAction::Call {
            tool: tool.to_owned(),
            input,
        };
        match self.act(turn, action)?.0 {
            TurnActed::Call(result) => Ok(result),
            _ => Err(invalid("a turn call did something else")),
        }
    }

    /// Answer the act of the turn at `turn` with `output`; the run continues
    /// as far as it goes.
    pub fn complete(&self, turn: &str, output: JsonObject) -> Result<RunResult, SkillRunError> {
        Ok(self.act(turn, TurnAction::Complete(output))?.1)
    }

    /// Drive the run to the turn at `turn` and do `action` there.
    fn act(&self, turn: &str, action: TurnAction) -> Result<(TurnActed, RunResult), SkillRunError> {
        let _serial = self.serial.lock().unwrap_or_else(PoisonError::into_inner);
        let mut host = RunHost::acting(TurnAct {
            turn: turn.to_owned(),
            action,
        });
        let result = self.run.advance(&mut host)?;
        let acted = host
            .acted()
            .ok_or_else(|| invalid(format!("run {} did not reach turn {turn}", self.run_id())))?;
        Ok((acted, result))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use runx_contracts::{JsonObject, JsonValue, ResolutionRequest, TurnCallOutcome, TurnTarget};

    use crate::execution::orchestrator::{
        LocalOrchestrator, ManagedAgentPolicy, ResumeRequest, RunResult, SkillRunRequest,
        TurnRequest,
    };
    use crate::execution::run_record::{
        CallOutcome, CallRecord, PendingRequest, RunLog, request_digest, waiting_requests,
    };
    use crate::kernel::store::KernelStore;
    use crate::paths::{RUNX_CWD_ENV, RunxPaths};

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn env(root: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([(RUNX_CWD_ENV.to_owned(), root.display().to_string())])
    }

    fn resume(root: &Path, run_id: &str, answers: serde_json::Value) -> TestResult<RunResult> {
        let path = root.join("answers.json");
        fs::write(&path, answers.to_string())?;
        Ok(LocalOrchestrator::default().resume(&ResumeRequest {
            run_id: run_id.to_owned(),
            answers_path: path,
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
            package_digest: None,
            execution_closure_digest: None,
        })?)
    }

    #[test]
    fn a_call_that_did_not_settle_runs_again_only_on_its_acknowledgement() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let skill = root.join("turn-demo");
        fs::create_dir_all(&skill)?;
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: turn-demo\ndescription: Digests, then answers.\n---\n# Turn demo\n\nDigest, then answer.\n",
        )?;
        fs::write(
            skill.join("run.yaml"),
            "skill: turn-demo\ntools: [data.digest]\nrunners:\n  answer:\n    default: true\n    outputs:\n      answer: string\n",
        )?;
        let started = LocalOrchestrator::default().run_skill(&SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers_path: None,
            inputs: BTreeMap::new(),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        })?;
        let run_id = started
            .output
            .as_object()
            .and_then(|output| output.get("run_id"))
            .and_then(JsonValue::as_str)
            .ok_or("missing run_id")?
            .to_owned();

        // A driver claimed call 0 and stopped before it settled.
        let store = KernelStore::for_project(&RunxPaths::resolve(&env(root), root))?;
        let log = RunLog::open(Arc::clone(&store), run_id.clone())?;
        log.load()?;
        let turn = waiting_requests(&store, &run_id)?.remove(0);
        let retry = ResolutionRequest::Retry {
            id: "answer/0/retry".into(),
            reason: "call 0 has not settled".into(),
            retry_after_seconds: 0,
        };
        log.record_call(&CallRecord {
            step_id: "answer",
            attempt: 1,
            turn: &turn,
            index: 0,
            target: &TurnTarget::Tool {
                tool: "data.digest".into(),
            },
            input: &serde_json::from_value(
                serde_json::json!({"value": "hello", "encoding": "utf8_text"}),
            )?,
            outcome: &CallOutcome::Claimed(PendingRequest {
                id: "answer/0/retry".to_owned(),
                value: serde_json::from_value(serde_json::to_value(&retry)?)?,
                digest: request_digest(&retry)?,
            }),
            answers: &[],
        })?;

        let session = LocalOrchestrator::default().open_turns(&TurnRequest {
            run_id: run_id.clone(),
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
        })?;
        let address = "answer";
        assert!(matches!(
            &session.status(address)?.calls[0].outcome,
            TurnCallOutcome::Pending { waiting } if matches!(waiting.as_slice(), [ResolutionRequest::Retry { .. }])
        ));
        let early = session
            .complete(
                address,
                JsonObject::from([("answer".to_owned(), JsonValue::String("early".to_owned()))]),
            )
            .err()
            .ok_or("a turn with an unsettled call cannot complete")?;
        assert!(
            early
                .to_string()
                .contains("call 0 of its turn has not settled"),
            "{early}"
        );

        let refused = resume(
            root,
            &run_id,
            serde_json::json!({"answer/0/retry": {"retry": false}}),
        )
        .err()
        .ok_or("only the exact acknowledgement runs the call again")?;
        assert!(refused.to_string().contains("acknowledgement"), "{refused}");

        let settled = resume(
            root,
            &run_id,
            serde_json::json!({"answer/0/retry": {"retry": true}}),
        )?;
        assert!(settled.needs_resolution());
        assert!(matches!(
            &session.status(address)?.calls[0].outcome,
            TurnCallOutcome::Completed { .. }
        ));
        // The acknowledgement is recorded once.
        let record = log.load()?.ok_or("the run has a record")?;
        assert_eq!(
            record
                .answers
                .iter()
                .filter(|answer| answer.request_id == "answer/0/retry")
                .count(),
            1
        );
        let sealed = session.complete(
            address,
            JsonObject::from([(
                "answer".to_owned(),
                JsonValue::String("digested".to_owned()),
            )]),
        )?;
        assert!(sealed.succeeded(), "{:?}", sealed.output);
        Ok(())
    }
}
