//! The open turn of a paused run, held for an agent that works it from
//! outside the run. The run is bound once; each action drives the run to its
//! turn and acts there, so a call runs at the agent step, under its runtime,
//! at whatever depth the step runs. One action runs at a time: a turn has
//! one driver.

use std::sync::{Mutex, PoisonError};

use runx_contracts::{AgentActInvocation, JsonObject, TurnCallResult, TurnStatus, TurnTarget};

use super::graph::{RunHost, TurnAct, TurnActed, TurnAction};
use super::{BoundRun, SkillRunError, invalid};
use crate::execution::orchestrator::RunResult;
use crate::execution::runner::turn::TurnTool;

pub struct TurnSession {
    run: BoundRun,
    invocation: AgentActInvocation,
    tools: Vec<TurnTool>,
    serial: Mutex<()>,
}

impl std::fmt::Debug for TurnSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TurnSession")
            .field("turn", &self.invocation.id)
            .finish_non_exhaustive()
    }
}

impl TurnSession {
    /// Open the turn `run` waits on.
    pub(crate) fn open(run: BoundRun) -> Result<Self, SkillRunError> {
        let (acted, _) = act(&run, None, TurnAction::Open)?;
        let TurnActed::Opened { invocation, tools } = acted else {
            return Err(invalid("opening a turn read something else"));
        };
        Ok(Self {
            run,
            invocation: *invocation,
            tools,
            serial: Mutex::new(()),
        })
    }

    /// The act the turn answers: its instructions, inputs and output
    /// contract.
    #[must_use]
    pub fn invocation(&self) -> &AgentActInvocation {
        &self.invocation
    }

    /// The tools the turn offers its agent.
    #[must_use]
    pub fn tools(&self) -> &[TurnTool] {
        &self.tools
    }

    /// Make a call within the turn's allowance; the run admits, executes and
    /// records it.
    pub fn call(
        &self,
        target: TurnTarget,
        input: JsonObject,
    ) -> Result<TurnCallResult, SkillRunError> {
        match self.act(TurnAction::Call(target, input))?.0 {
            TurnActed::Call(result) => Ok(result),
            _ => Err(invalid("a turn call did something else")),
        }
    }

    /// The turn as its agent reads it.
    pub fn status(&self) -> Result<TurnStatus, SkillRunError> {
        match self.act(TurnAction::Status)?.0 {
            TurnActed::Status(status) => Ok(status),
            _ => Err(invalid("reading a turn did something else")),
        }
    }

    /// Answer the act with `output`; the run continues as far as it goes.
    pub fn complete(&self, output: JsonObject) -> Result<RunResult, SkillRunError> {
        Ok(self.act(TurnAction::Complete(output))?.1)
    }

    fn act(&self, action: TurnAction) -> Result<(TurnActed, RunResult), SkillRunError> {
        let _serial = self.serial.lock().unwrap_or_else(PoisonError::into_inner);
        act(&self.run, Some(self.invocation.id.as_str()), action)
    }
}

/// Drive `run` to the turn it waits on and do `action` there.
fn act(
    run: &BoundRun,
    turn: Option<&str>,
    action: TurnAction,
) -> Result<(TurnActed, RunResult), SkillRunError> {
    let mut host = RunHost::acting(TurnAct {
        turn: turn.map(str::to_owned),
        action,
    });
    let result = run.advance(&mut host)?;
    let acted = host.acted().ok_or_else(|| {
        invalid(format!(
            "run {} did not reach its turn",
            run.request.run_id.as_deref().unwrap_or_default()
        ))
    })?;
    Ok((acted, result))
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
        CallOutcome, CallRecord, PendingRequest, RunLog, pending_request, request_digest,
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
        let turn = pending_request(&store, &run_id)?.ok_or("the run waits on its turn")?;
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
            outcome: &CallOutcome::Pending(PendingRequest {
                id: "answer/0/retry".to_owned(),
                value: serde_json::from_value(serde_json::to_value(&retry)?)?,
                digest: request_digest(&retry)?,
            }),
            answers: &[],
        })?;

        let session = LocalOrchestrator::default().open_turn(&TurnRequest {
            run_id: run_id.clone(),
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
        })?;
        assert!(matches!(
            &session.status()?.calls[0].outcome,
            TurnCallOutcome::Pending {
                request: ResolutionRequest::Retry { .. }
            }
        ));
        let behind = session
            .call(
                TurnTarget::Tool {
                    tool: "data.digest".into(),
                },
                serde_json::from_value(
                    serde_json::json!({"value": "again", "encoding": "utf8_text"}),
                )?,
            )
            .err()
            .ok_or("no call is made behind one that may have run")?;
        assert!(
            behind.to_string().contains("call 0 has not settled"),
            "{behind}"
        );
        let early = session
            .complete(JsonObject::from([(
                "answer".to_owned(),
                JsonValue::String("early".to_owned()),
            )]))
            .err()
            .ok_or("a turn with an unsettled call cannot complete")?;
        assert!(
            early.to_string().contains("still waits on answer/0/retry"),
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
            &session.status()?.calls[0].outcome,
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
        let sealed = session.complete(JsonObject::from([(
            "answer".to_owned(),
            JsonValue::String("digested".to_owned()),
        )]))?;
        assert!(sealed.succeeded(), "{:?}", sealed.output);
        Ok(())
    }
}
