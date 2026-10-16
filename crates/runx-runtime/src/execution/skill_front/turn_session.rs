//! The open turns of a paused run, held for an agent that works them from
//! outside the run. The run is bound once; each action drives the run to the
//! turn it names, by address, and acts there, so a call runs at the agent
//! step, under its runtime, at whatever depth the step runs. A turn a call
//! opens in a child skill is reached the same way.
//!
//! The session is the run's one driver, and holds the run's driver lock for
//! as long as it is open. A call is admitted and claimed in
//! order under the driver, executes apart from it, concurrently with the
//! session's other admitted calls, and is recorded under it again. Where a
//! call reaches the world it takes the session's effect gate: a read shares
//! it and a change holds it alone, inside a child run too, so effects never
//! overlap and reads never wait on each other. Every other action waits
//! until no call is executing, so it reads the run as its calls left it.

use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use runx_contracts::{JsonObject, ResolutionRequest, TurnCallResult, TurnStatus};

use super::graph::{RunHost, TurnAct, TurnActed, TurnAction};
use super::{BoundRun, SkillRunError, invalid};
use crate::RuntimeError;
use crate::execution::orchestrator::RunResult;
use crate::execution::runner::EffectGate;
use crate::execution::runner::turn::Admission;

pub struct TurnSession {
    run: BoundRun,
    /// Held while the session admits a call, records one, or does anything
    /// else that writes the run.
    driver: Mutex<()>,
    /// The calls executing apart from the driver, by turn and index.
    in_flight: Mutex<BTreeSet<(String, u32)>>,
    /// Signalled whenever a call's execution ends.
    settled: Condvar,
    gate: Arc<EffectGate>,
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
            driver: Mutex::new(()),
            in_flight: Mutex::new(BTreeSet::new()),
            settled: Condvar::new(),
            gate: Arc::new(EffectGate::default()),
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
        self.run.run_id()
    }

    /// Where the run's receipts go, when its host named a directory.
    #[must_use]
    pub fn receipt_dir(&self) -> Option<&std::path::Path> {
        self.run.request.receipt_dir.as_deref()
    }

    /// Everything the run waits on, by address: each open turn's act, and
    /// each ask, approval or retry beneath them.
    pub fn waiting(&self) -> Result<Vec<ResolutionRequest>, SkillRunError> {
        let _driver = self.quiet();
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
        let driver = self.quiet();
        match self.pass(&driver, turn, TurnAction::Status)?.0 {
            TurnActed::Status(status) => Ok(*status),
            _ => Err(invalid("reading a turn did something else")),
        }
    }

    /// Make the call the turn at `turn` offers as `tool`; the run admits,
    /// executes and records it. A `call_id` pins the call: made again with
    /// it, the recorded call returns rather than running twice. Calls made
    /// at once execute at once, each taking the session's effect gate where
    /// it reaches the world.
    pub fn call(
        &self,
        turn: &str,
        tool: &str,
        call_id: Option<&str>,
        input: JsonObject,
    ) -> Result<TurnCallResult, SkillRunError> {
        let admitted = {
            let driver = self.lock(&self.driver);
            let action = TurnAction::Admit {
                tool: tool.to_owned(),
                call_id: call_id.map(str::to_owned),
                input,
            };
            let admission = match self.pass(&driver, turn, action)?.0 {
                TurnActed::Admitted(admission) => *admission,
                _ => return Err(invalid("a turn call did something else")),
            };
            match admission {
                Admission::Settled(result) => return Ok(*result),
                Admission::Admitted(admitted) => {
                    self.lock(&self.in_flight)
                        .insert((turn.to_owned(), admitted.index()));
                    *admitted
                }
            }
        };
        let index = admitted.index();
        let executed = self.execute(turn, admitted);
        let concluded = executed.and_then(|executed| {
            let driver = self.lock(&self.driver);
            match self
                .pass(&driver, turn, TurnAction::Conclude(Box::new(executed)))?
                .0
            {
                TurnActed::Call(result) => Ok(*result),
                _ => Err(invalid("a turn call did something else")),
            }
        });
        self.lock(&self.in_flight).remove(&(turn.to_owned(), index));
        self.settled.notify_all();
        concluded
    }

    /// Answer the act of the turn at `turn` with `output`; the run continues
    /// as far as it goes.
    pub fn complete(&self, turn: &str, output: JsonObject) -> Result<RunResult, SkillRunError> {
        let driver = self.quiet();
        Ok(self.pass(&driver, turn, TurnAction::Complete(output))?.1)
    }

    /// End the act of the turn at `turn` as failed, saying why; its step
    /// fails and the run continues as far as that goes.
    pub fn fail(&self, turn: &str, reason: &str) -> Result<RunResult, SkillRunError> {
        if reason.trim().is_empty() {
            return Err(invalid("a failed act says why"));
        }
        let driver = self.quiet();
        Ok(self
            .pass(&driver, turn, TurnAction::Fail(reason.to_owned()))?
            .1)
    }

    /// Execute an admitted call apart from the driver.
    fn execute(
        &self,
        turn: &str,
        admitted: crate::execution::runner::turn::AdmittedCall,
    ) -> Result<crate::execution::runner::turn::ExecutedCall, SkillRunError> {
        let mut host = RunHost::acting(TurnAct {
            turn: turn.to_owned(),
            action: TurnAction::Execute(Box::new(admitted)),
        })
        .in_session(Arc::clone(&self.gate), self.in_flight_now());
        self.run.advance(&mut host)?;
        match host.acted() {
            Some(TurnActed::Executed(executed)) => Ok(*executed),
            Some(_) => Err(invalid("a turn call did something else")),
            None => Err(invalid(format!(
                "run {} did not reach turn {turn}",
                self.run_id()
            ))),
        }
    }

    /// Drive the run to the turn at `turn` and do `action` there, under the
    /// driver.
    fn pass(
        &self,
        _driver: &MutexGuard<'_, ()>,
        turn: &str,
        action: TurnAction,
    ) -> Result<(TurnActed, RunResult), SkillRunError> {
        let mut host = RunHost::acting(TurnAct {
            turn: turn.to_owned(),
            action,
        })
        .in_session(Arc::clone(&self.gate), self.in_flight_now());
        let result = self.run.advance(&mut host)?;
        let acted = host
            .acted()
            .ok_or_else(|| invalid(format!("run {} did not reach turn {turn}", self.run_id())))?;
        Ok((acted, result))
    }

    /// The driver, once no call is executing.
    fn quiet(&self) -> MutexGuard<'_, ()> {
        let mut in_flight = self.lock(&self.in_flight);
        while !in_flight.is_empty() {
            in_flight = self
                .settled
                .wait(in_flight)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(in_flight);
        self.lock(&self.driver)
    }

    fn in_flight_now(&self) -> BTreeSet<(String, u32)> {
        self.lock(&self.in_flight).clone()
    }

    fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use runx_contracts::{
        CloseDisposition, ClosureDisposition, JsonObject, JsonValue, ResolutionRequest,
        ResumeDocument, RunClose, SealReason, TurnCallOutcome, TurnTarget,
    };

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

    /// Answer the request run `run_id` waits on at `address` with `value`
    /// in `lane`, bound to that request's digest.
    fn answer(
        root: &Path,
        run_id: &str,
        lane: &str,
        address: &str,
        value: serde_json::Value,
    ) -> TestResult<RunResult> {
        let store = KernelStore::for_project(&RunxPaths::resolve(&env(root), root))?;
        let digest = waiting_requests(&store, run_id)?
            .into_iter()
            .find(|request| request.id == address)
            .ok_or("the run waits on no such request")?
            .digest;
        Ok(LocalOrchestrator::default().resume(&ResumeRequest {
            run_id: run_id.to_owned(),
            answers: serde_json::from_value(serde_json::json!({
                lane: { address: { "request_digest": digest, "value": value } }
            }))?,
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
            package_digest: None,
            execution_closure_digest: None,
        })?)
    }

    #[test]
    fn a_close_seals_the_run_and_leaves_its_unsettled_call_unconfirmed() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let (run_id, log) = paused_with_unsettled_call(root)?;
        LocalOrchestrator::default().resume(&ResumeRequest {
            run_id,
            answers: ResumeDocument {
                close: Some(RunClose {
                    disposition: CloseDisposition::Superseded,
                }),
                ..ResumeDocument::default()
            },
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
            package_digest: None,
            execution_closure_digest: None,
        })?;
        let record = log.load()?.ok_or("the run has a record")?;
        let sealed = record.sealed.ok_or("the close seals the run")?;
        assert_eq!(
            sealed.receipt.seal.disposition,
            ClosureDisposition::Superseded
        );
        assert_eq!(sealed.receipt.seal.reason, Some(SealReason::HostClosed));
        assert!(
            matches!(record.calls.as_slice(), [call] if matches!(call.outcome, CallOutcome::Claimed(_))),
            "the claimed call stays unconfirmed"
        );
        Ok(())
    }

    #[test]
    fn a_close_seals_a_run_that_stopped_between_steps_under_a_changed_package() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let (run_id, log) = paused_with_unsettled_call(root)?;
        // The package changed since the run started, and its driver stopped
        // between steps.
        fs::write(
            root.join("turn-demo/SKILL.md"),
            "---\nname: turn-demo\ndescription: Digests, then answers.\n---\n# Turn demo\n\nChanged since.\n",
        )?;
        log.load()?;
        log.record_skipped(vec!["elsewhere".to_owned()])?;
        let close = |document: serde_json::Value| {
            LocalOrchestrator::default()
                .resume(&ResumeRequest {
                    run_id: run_id.clone(),
                    answers: serde_json::from_value(document)?,
                    receipt_dir: Some(root.join("receipts")),
                    env: env(root),
                    cwd: root.to_path_buf(),
                    managed_agent: ManagedAgentPolicy::HostDriven,
                    package_digest: None,
                    execution_closure_digest: None,
                })
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
        };
        let alone = close(serde_json::json!({
            "close": {"disposition": "declined"},
            "approvals": {"answer/0/retry": {"request_digest": "sha256:r", "value": {"retry": true}}}
        }))
        .err()
        .ok_or("a close comes alone")?;
        assert!(alone.to_string().contains("comes alone"), "{alone}");

        let closed = close(serde_json::json!({"close": {"disposition": "declined"}}))?;
        assert!(!closed.succeeded());
        let sealed = log
            .load()?
            .ok_or("the run has a record")?
            .sealed
            .ok_or("the close seals the run")?;
        assert_eq!(
            sealed.receipt.seal.disposition,
            ClosureDisposition::Declined
        );
        assert_eq!(sealed.receipt.seal.reason, Some(SealReason::HostClosed));
        let again = close(serde_json::json!({"close": {"disposition": "declined"}}))
            .err()
            .ok_or("a sealed run does not close again")?;
        assert!(again.to_string().contains("is sealed"), "{again}");
        Ok(())
    }

    #[test]
    fn a_stopped_call_admits_nothing_and_the_next_call_runs() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let (run_id, log) = paused_with_unsettled_call(root)?;
        let session = LocalOrchestrator::default().open_turns(&TurnRequest {
            run_id,
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
        })?;
        let input = || {
            serde_json::from_value::<JsonObject>(
                serde_json::json!({"value": "again", "encoding": "utf8_text"}),
            )
        };
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let stopped = crate::interrupt::while_working(&cancelled, || {
            session
                .call("answer", "data_digest", Some("stopped"), input()?)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
        })
        .err()
        .ok_or("a stopped driver admits no call")?;
        assert!(stopped.to_string().contains("stopped before"), "{stopped}");
        let record = log.load()?.ok_or("the run has a record")?;
        assert_eq!(record.calls.len(), 1, "the stop recorded nothing");
        assert!(record.sealed.is_none());

        let next = session.call("answer", "data_digest", Some("next"), input()?)?;
        assert_eq!(next.index, 1);
        Ok(())
    }

    /// A run paused at its turn, whose call 0 a driver claimed and stopped
    /// before it settled.
    fn paused_with_unsettled_call(root: &Path) -> TestResult<(String, RunLog)> {
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
            answers: None,
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
            resend: runx_contracts::RetryResend::Safe,
        };
        log.record_call(&CallRecord {
            step_id: "answer",
            attempt: 1,
            turn: &turn,
            index: 0,
            call_id: None,
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
            outputs: &[],
            standing: None,
            answers: &[],
        })?;
        Ok((run_id, log))
    }

    #[test]
    fn a_call_that_did_not_settle_runs_again_only_on_its_acknowledgement() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let (run_id, log) = paused_with_unsettled_call(root)?;

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
        // A person's answer needs the driver lock: the host closes the
        // session to deliver it.
        let held = answer(
            root,
            &run_id,
            "approvals",
            "answer/0/retry",
            serde_json::json!({"retry": true}),
        )
        .err()
        .ok_or("an open session refuses a second driver")?;
        assert!(held.to_string().contains("driven"), "{held}");
        drop(session);

        let refused = answer(
            root,
            &run_id,
            "approvals",
            "answer/0/retry",
            serde_json::json!({"retry": false}),
        )
        .err()
        .ok_or("only the exact acknowledgement runs the call again")?;
        assert!(
            refused.to_string().contains("acknowledged only"),
            "{refused}"
        );
        let agent = answer(
            root,
            &run_id,
            "answers",
            "answer/0/retry",
            serde_json::json!({"retry": true}),
        )
        .err()
        .ok_or("a retry is the person's to acknowledge")?;
        assert!(agent.to_string().contains("under approvals"), "{agent}");

        let settled = answer(
            root,
            &run_id,
            "approvals",
            "answer/0/retry",
            serde_json::json!({"retry": true}),
        )?;
        assert!(settled.needs_resolution());
        let session = open(root, &run_id)?;
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

    #[test]
    fn an_answer_admitted_before_a_crash_is_used_by_the_next_resume() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let skill = root.join("asks");
        fs::create_dir_all(&skill)?;
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: asks\ndescription: Asks, then answers.\n---\n# Asks\n\nAsk, then answer.\n",
        )?;
        fs::write(
            skill.join("run.yaml"),
            "skill: asks\nasks: true\nrunners:\n  answer:\n    default: true\n    outputs:\n      answer: string\n",
        )?;
        let started = LocalOrchestrator::default().run_skill(&SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
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
        let session = LocalOrchestrator::default().open_turns(&TurnRequest {
            run_id: run_id.clone(),
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
        })?;
        session.call(
            "answer",
            "ask_human",
            Some("ask-1"),
            serde_json::from_value(serde_json::json!({"reason": "Go on?"}))?,
        )?;
        drop(session);

        // A resume admitted the person's approval and stopped before the
        // call went further.
        let store = KernelStore::for_project(&RunxPaths::resolve(&env(root), root))?;
        let ask = waiting_requests(&store, &run_id)?
            .into_iter()
            .find(|request| request.id == "answer/0")
            .ok_or("the call waits on its ask")?;
        let log = RunLog::open(Arc::clone(&store), run_id.clone())?;
        log.record_answer(&crate::execution::run_record::Answer {
            request_id: ask.id.clone(),
            request_digest: ask.digest.clone(),
            value: serde_json::from_value(serde_json::json!({"approved": true}))?,
            human: true,
        })?;
        let read = crate::execution::run_record::read_run(&env(root), root, &run_id, None)?;
        assert!(read.events.iter().any(|record| matches!(
            &record.event,
            runx_contracts::RunEventFact::Answered { address, actor: runx_contracts::ResolutionResponseActor::Human, .. }
                if address.as_str() == "answer/0"
        )));
        assert_eq!(read.run.state, runx_contracts::RunState::Paused);

        // The next resume brings nothing and uses the recorded answer.
        let resume = |document: serde_json::Value| {
            LocalOrchestrator::default()
                .resume(&ResumeRequest {
                    run_id: run_id.clone(),
                    answers: serde_json::from_value(document)?,
                    receipt_dir: Some(root.join("receipts")),
                    env: env(root),
                    cwd: root.to_path_buf(),
                    managed_agent: ManagedAgentPolicy::HostDriven,
                    package_digest: None,
                    execution_closure_digest: None,
                })
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
        };
        resume(serde_json::json!({}))?;
        assert!(log.admitted()?.is_empty(), "the call used the answer");
        let session = LocalOrchestrator::default().open_turns(&TurnRequest {
            run_id: run_id.clone(),
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
        })?;
        assert!(matches!(
            &session.status("answer")?.calls[0].outcome,
            TurnCallOutcome::Completed { .. }
        ));
        drop(session);

        // Delivered again, it answers nothing the run still waits on.
        let again = resume(serde_json::json!({
            "approvals": {"answer/0": {"request_digest": ask.digest, "approved": true}}
        }))
        .err()
        .ok_or("the request is answered")?;
        assert!(again.to_string().contains("names no request"), "{again}");
        Ok(())
    }

    #[test]
    fn a_skill_calls_act_is_its_child_runs_act() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let child = root.join("child");
        fs::create_dir_all(&child)?;
        fs::write(
            child.join("SKILL.md"),
            "---\nname: child\ndescription: Digests.\n---\n# Child\n\nDigest.\n",
        )?;
        fs::write(
            child.join("run.yaml"),
            "skill: child\nrunners:\n  digest:\n    default: true\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: child\n          encoding: utf8_text\n",
        )?;
        let skill = root.join("parent");
        fs::create_dir_all(&skill)?;
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: parent\ndescription: Calls its child.\n---\n# Parent\n\nCall the child, then answer.\n",
        )?;
        fs::write(
            skill.join("run.yaml"),
            "skill: parent\nskills: [../child]\nrunners:\n  answer:\n    default: true\n    outputs:\n      answer: string\n",
        )?;
        let started = LocalOrchestrator::default().run_skill(&SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
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
        let session = LocalOrchestrator::default().open_turns(&TurnRequest {
            run_id: run_id.clone(),
            receipt_dir: Some(root.join("receipts")),
            env: env(root),
            cwd: root.to_path_buf(),
        })?;
        let input = |value: serde_json::Value| serde_json::from_value::<JsonObject>(value);

        let called = session.call(
            "answer",
            "skill__child",
            None,
            input(serde_json::json!({"act": {"purpose": "check the release notes"}}))?,
        )?;
        assert!(
            matches!(called.outcome, TurnCallOutcome::Completed { .. }),
            "{called:?}"
        );
        let store = KernelStore::for_project(&RunxPaths::resolve(&env(root), root))?;
        let child = RunLog::open(store, format!("{run_id}/answer/0"))?
            .load()?
            .ok_or("the call started its child run")?;
        assert_eq!(
            child.act.map(|act| act.purpose.as_str().to_owned()),
            Some("check the release notes".to_owned())
        );

        let malformed = session.call(
            "answer",
            "skill__child",
            None,
            input(serde_json::json!({"act": {"purpose": ""}}))?,
        )?;
        assert!(
            matches!(&malformed.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains("act")),
            "{malformed:?}"
        );
        Ok(())
    }

    /// A run of a one-turn skill that reads and writes files, waiting on its
    /// turn, and the session that works it.
    fn files_session(root: &Path) -> TestResult<(String, Arc<super::TurnSession>)> {
        let skill = root.join("files-demo");
        fs::create_dir_all(&skill)?;
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: files-demo\ndescription: Reads and writes files.\n---\n# Files demo\n\nRead, write, answer.\n",
        )?;
        fs::write(
            skill.join("run.yaml"),
            "skill: files-demo\ntools: [fs.read, fs.write]\nrunners:\n  answer:\n    default: true\n    outputs:\n      answer: string\n",
        )?;
        let started = LocalOrchestrator::default().run_skill(&SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
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
        let session = open(root, &run_id)?;
        Ok((run_id, session))
    }

    fn open(root: &Path, run_id: &str) -> TestResult<Arc<super::TurnSession>> {
        Ok(Arc::new(LocalOrchestrator::default().open_turns(
            &TurnRequest {
                run_id: run_id.to_owned(),
                receipt_dir: Some(root.join("receipts")),
                env: env(root),
                cwd: root.to_path_buf(),
            },
        )?))
    }

    fn input(value: serde_json::Value) -> TestResult<JsonObject> {
        Ok(serde_json::from_value(value)?)
    }

    #[test]
    fn calls_made_at_once_execute_at_once_and_a_change_waits_to_run_alone() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        for index in 0..8 {
            fs::write(
                root.join(format!("note-{index}.txt")),
                format!("note {index}\n"),
            )?;
        }
        let (_run_id, session) = files_session(root)?;

        // While the gate is held, no call reaches the world: every call made
        // at once is admitted, claimed and executing, and none waits on
        // another's execution to be admitted.
        let held = session.gate.pass(false);
        let mut calls = Vec::new();
        for index in 0..8 {
            let session = Arc::clone(&session);
            calls.push(std::thread::spawn(move || {
                session.call(
                    "answer",
                    "fs_read",
                    Some(&format!("read-{index}")),
                    input(serde_json::json!({"path": format!("note-{index}.txt")}))
                        .map_err(|error| super::invalid(error.to_string()))?,
                )
            }));
        }
        let writer = {
            let session = Arc::clone(&session);
            std::thread::spawn(move || {
                session.call(
                    "answer",
                    "fs_write",
                    Some("write"),
                    input(serde_json::json!({"path": "out.txt", "contents": "done\n", "create": true}))
                        .map_err(|error| super::invalid(error.to_string()))?,
                )
            })
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while session.in_flight_now().len() < 9 {
            assert!(
                std::time::Instant::now() < deadline,
                "nine calls admitted at once: {:?}",
                session.in_flight_now()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !root.join("out.txt").exists(),
            "nothing ran while the gate was held"
        );
        drop(held);

        for call in calls.into_iter().chain(std::iter::once(writer)) {
            let called = call.join().map_err(|_| "a call panicked")??;
            assert!(
                matches!(called.outcome, TurnCallOutcome::Completed { .. }),
                "{called:?}"
            );
        }
        assert_eq!(fs::read_to_string(root.join("out.txt"))?, "done\n");

        // The record holds each call once, by its id.
        let status = session.status("answer")?;
        let mut ids = status
            .calls
            .iter()
            .map(|call| call.call_id.as_ref().map(|id| id.as_str().to_owned()))
            .collect::<Option<Vec<_>>>()
            .ok_or("every call is pinned")?;
        ids.sort();
        let mut expected = (0..8)
            .map(|index| format!("read-{index}"))
            .collect::<Vec<_>>();
        expected.push("write".to_owned());
        expected.sort();
        assert_eq!(ids, expected);
        let mut indices = status
            .calls
            .iter()
            .map(|call| call.index)
            .collect::<Vec<_>>();
        indices.sort_unstable();
        assert_eq!(indices, (0..9).collect::<Vec<_>>());
        Ok(())
    }

    #[test]
    fn calls_a_crashed_session_admitted_are_recovered_by_their_ids() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::write(root.join("note.txt"), "note\n")?;
        let (run_id, session) = files_session(root)?;

        // Two calls admitted and claimed, and the session gone before either
        // was recorded as executed.
        for id in ["first", "second"] {
            let driver = session.lock(&session.driver);
            let admitted = session
                .pass(
                    &driver,
                    "answer",
                    super::TurnAction::Admit {
                        tool: "fs_read".to_owned(),
                        call_id: Some(id.to_owned()),
                        input: input(serde_json::json!({"path": "note.txt"}))?,
                    },
                )?
                .0;
            assert!(matches!(
                admitted,
                super::TurnActed::Admitted(admission)
                    if matches!(*admission, super::Admission::Admitted(_))
            ));
        }
        drop(session);

        // The next session finds each by its id, and a call made again with
        // its id returns it rather than running again.
        let session = open(root, &run_id)?;
        let again = session.call(
            "answer",
            "fs_read",
            Some("second"),
            input(serde_json::json!({"path": "note.txt"}))?,
        )?;
        assert_eq!(again.index, 1);
        assert!(
            matches!(again.outcome, TurnCallOutcome::Pending { .. }),
            "it waits on its retry: {again:?}"
        );
        let status = session.status("answer")?;
        assert_eq!(
            status
                .calls
                .iter()
                .map(|call| call.call_id.as_ref().map(|id| id.as_str().to_owned()))
                .collect::<Vec<_>>(),
            [Some("first".to_owned()), Some("second".to_owned())]
        );
        Ok(())
    }
}
