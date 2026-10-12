// Module rationale: the skill front's run driver keeps the run record,
// host resolution and sealing in one path: a run starts or continues from its
// record, takes what its host brings, runs, and seals.
use super::{
    BoundRun, ExecutableAdapter, SkillOutputDiagnostics, SkillRunError, invalid, paused_output,
    sealed_output,
};

use std::collections::BTreeMap;

use runx_contracts::{
    ClosureDisposition, JsonObject, JsonValue, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor, SealReason, TurnCallOutcome, TurnCallResult, TurnStatus,
};
use runx_parser::{ExecutionGraph, SkillRunnerDefinition};

use crate::RuntimeError;
#[cfg(test)]
use crate::adapter::SkillInvocation;
use crate::execution::graph::bind_run_inputs;
use crate::execution::orchestrator::{RunResult, SkillRunRequest};
use crate::execution::run_record::{
    Driver, Hold, Origin, PendingRequest, RunContext, RunLog, RunRecord, StartRequest,
    waiting_requests,
};
use crate::execution::runner::turn::{Turn, call_tool};
use crate::execution::runner::{
    GraphRun, RUNX_RUN_ID_ENV, Runtime, RuntimeOptions, graph_run_context, graph_run_result,
    graph_run_skill_output, graph_run_trace, sealed_graph_run,
};
use crate::host::Host;
use crate::kernel::authority::RunAuthority;
use crate::kernel::store::KernelStore;
use crate::services::{ReceiptServices, WorkspaceEnv};

use super::resolution_answers::{ResolutionAnswers, read_answers};
use super::runner_manifest::write_skill_receipt;

/// How many times a run waits for an independent provider readback before
/// it is sealed deferred.
const MAX_READBACK_RETRIES: u32 = 24;

impl BoundRun {
    /// Start the run, or continue it with the answers its request brings, and
    /// drive it as far as it goes.
    pub(crate) fn execute(&self) -> Result<RunResult, SkillRunError> {
        let answers = match (&self.overrides.seeded_answers, &self.request.answers_path) {
            (Some(seeded), _) => seeded.clone(),
            (None, Some(path)) => read_answers(path)?,
            (None, None) => ResolutionAnswers::default(),
        };
        let mut host = RunHost::new(answers);
        host.scripts = std::rc::Rc::new(self.overrides.turn_scripts.clone());
        #[cfg(feature = "agent")]
        {
            host.managed = self.request.managed_agent.max_rounds().map(|max_rounds| {
                std::rc::Rc::new(crate::adapters::managed_agent::ManagedAgent::new(
                    self.workspace.skill_env_for_skill(&self.skill_dir),
                    self.skill_dir.clone(),
                    max_rounds,
                ))
            });
        }
        self.advance(&mut host)
    }

    // Function rationale: one run driver: open the record, bind what the host
    // brings, run or continue, and seal; each outcome branch seals exactly
    // once.
    /// Drive the run as far as it goes with what `host` brings: answers, an
    /// action on the turn the run waits on, or a managed agent.
    pub(super) fn advance(&self, host: &mut RunHost) -> Result<RunResult, SkillRunError> {
        let request = &self.request;
        let execution_closure_digest = self
            .execution_closure_digest
            .as_deref()
            .ok_or_else(|| invalid("graph execution requires an execution-closure digest"))?;
        let log = run_log(request, &self.workspace, &self.manifest.skill)?;
        let existing = log.load()?;
        let waiting = waiting_requests(log.store(), log.run_id())?;
        host.check_turn(log.run_id(), &waiting)?;
        host.answers.check_waiting(
            log.run_id(),
            &waiting
                .iter()
                .map(|request| request.id.as_str())
                .collect::<Vec<_>>(),
        )?;
        let request_context = RunContext {
            current: request.current_context.clone(),
            historical: request.historical_context.clone(),
        };
        let (graph_inputs, run_act, run_context, run_authority) = match &existing {
            Some(record) => {
                let started = continuation_identity(
                    record,
                    request,
                    &request_context,
                    &self.runner,
                    &self.package_digest,
                    execution_closure_digest,
                )?;
                host.check_driver(log.run_id(), started.driver)?;
                (
                    record.inputs.clone(),
                    record.act.clone(),
                    record.context.clone(),
                    started.run_authority.clone(),
                )
            }
            None if request.answers_path.is_some() => {
                return Err(invalid(format!(
                    "run {} has no record to continue",
                    log.run_id()
                )));
            }
            None => {
                let inputs = request
                    .inputs
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<JsonObject>();
                log.begin(StartRequest {
                    skill: &self.manifest.skill,
                    skill_ref: &request.skill_path.to_string_lossy(),
                    runner: &self.runner.name,
                    package_digest: &self.package_digest,
                    execution_closure_digest,
                    inputs: &inputs,
                    context: &request_context,
                    run_authority: request.run_authority.as_ref(),
                    act: request.act.as_ref(),
                    driver: if request.managed_agent.max_rounds().is_some() {
                        Driver::Managed
                    } else {
                        Driver::Attached
                    },
                    created_at: &crate::time::now_iso8601(),
                })?;
                (
                    inputs,
                    request.act.clone(),
                    request_context,
                    request.run_authority.clone(),
                )
            }
        };
        if let Some(record) = existing.as_ref().filter(|record| record.sealed.is_some()) {
            return self.restore_sealed_run(&log, record.clone());
        }
        if existing.is_some() {
            crate::input_contract::materialize_complete_runner_inputs(
                &self.runner.inputs,
                &graph_inputs,
            )
            .map_err(|error| error.into_runtime_error())?;
        }
        if let Some(suspension) = existing
            .as_ref()
            .and_then(|record| record.suspension.as_ref())
        {
            match &suspension.hold {
                Hold::Retry(retry) => {
                    let Some(answer) = host.answers.get(&retry.id, &retry.digest) else {
                        return self.paused(&log);
                    };
                    if answer
                        != &JsonValue::Object(JsonObject::from([(
                            "retry".to_owned(),
                            JsonValue::Bool(true),
                        )]))
                    {
                        return Err(invalid(format!(
                            "{} is answered with {{\"retry\": true}}",
                            retry.id
                        )));
                    }
                }
                // Nothing new to continue with: the run still waits on what
                // only a host can answer. A run that waits on nothing, a child
                // that sealed before its parent step was recorded, goes on.
                Hold::Request(_) | Hold::Child(_) if host.is_idle() && !waiting.is_empty() => {
                    return self.paused(&log);
                }
                Hold::Request(_) | Hold::Child(_) => {}
            }
        }
        let runtime = self.runtime(&log, run_context, run_authority);
        let graph = bind_run_inputs(self.runner.graph.clone(), &graph_inputs, run_act.as_ref())?;
        let run = Driven {
            bound: self,
            log: &log,
            runtime: &runtime,
            graph: &graph,
        };
        match runtime.run_graph_with_host(&self.skill_dir, graph.clone(), host) {
            Ok(completed) => run.seal_completed(completed),
            Err(RuntimeError::ResolutionPending { .. }) => self.paused(&log),
            Err(RuntimeError::StepClosed {
                step_id,
                disposition,
                reason,
                ..
            }) => run.seal_stopped(&step_id, reason, Stop::Closed(disposition)),
            Err(RuntimeError::GraphBlocked { step_id, reason }) => run.seal_stopped(
                &step_id.clone(),
                Some(SealReason::GraphBlocked),
                Stop::Blocked(RuntimeError::GraphBlocked { step_id, reason }),
            ),
            Err(RuntimeError::AuthorityDenied {
                verb,
                step_id,
                reason,
            }) => run.seal_stopped(
                &step_id.clone(),
                Some(SealReason::AuthorityDenied),
                Stop::Blocked(RuntimeError::AuthorityDenied {
                    verb,
                    step_id,
                    reason,
                }),
            ),
            // The mutation may be applied and its readback is pending. The
            // run stays open, unsealed, so the call seals once, when its
            // outcome is known: retried while readback may yet arrive, then
            // returned to the operator to reconcile before resuming.
            Err(RuntimeError::ProviderReadbackPending { step_id, reason }) => {
                let retries = existing.as_ref().map_or(0, |record| record.retries) + 1;
                if retries > MAX_READBACK_RETRIES {
                    return Err(RuntimeError::ProviderReadbackPending { step_id, reason }.into());
                }
                log.record_retry(
                    &step_id,
                    &ResolutionRequest::Retry {
                        id: format!("provider-readback-{step_id}-{retries}").into(),
                        reason: "Independent provider readback is pending".into(),
                        retry_after_seconds: 300,
                    },
                )?;
                self.paused(&log)
            }
            #[cfg(feature = "agent")]
            Err(RuntimeError::ManagedAgentResolution {
                step_id,
                request_id,
                source,
            }) => {
                let reason = source.reason();
                let error = RuntimeError::ManagedAgentResolution {
                    step_id: step_id.clone(),
                    request_id,
                    source,
                };
                run.seal_stopped(&step_id, Some(reason), Stop::Failed(error))
            }
            Err(error) if !error.is_fatal_step_fault() => {
                let step_id =
                    crate::execution::runner::failed_step_id(&graph, &error).ok_or_else(|| {
                        RuntimeError::EngineInvariant {
                            context: "sealing graph step failure",
                            message: format!(
                                "sealable graph error has no authoritative step: {error}"
                            ),
                        }
                    })?;
                run.seal_stopped(
                    &step_id,
                    Some(SealReason::GraphStepFailed),
                    Stop::Failed(error),
                )
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The run's runtime for one pass: the skill's environment, the run's
    /// identity and authority, and its log.
    fn runtime(
        &self,
        log: &RunLog,
        context: RunContext,
        run_authority: Option<RunAuthority>,
    ) -> Runtime<ExecutableAdapter> {
        let mut env = self.workspace.skill_env_for_skill(&self.skill_dir);
        env.insert(RUNX_RUN_ID_ENV.to_owned(), log.run_id().to_owned());
        env.insert(
            crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
            self.workspace
                .receipt_path(self.request.receipt_dir.as_deref())
                .path
                .to_string_lossy()
                .into_owned(),
        );
        let options = RuntimeOptions {
            current_context: context.current,
            historical_context: context.historical,
            run_authority,
            clock: crate::execution::runner::RunClock::System,
            env,
            receipt_signature: self.receipts.signature_config().clone(),
            effects: self.effects.clone(),
            run: log.clone(),
        };
        match &self.overrides.javascript {
            Some(javascript) => Runtime::with_native_services(
                ExecutableAdapter,
                options,
                javascript.clone(),
                crate::services::LocalArtifactService::default(),
            ),
            None => Runtime::new(ExecutableAdapter, options),
        }
    }

    /// Every request the run waits on, by address.
    pub(super) fn waiting(&self) -> Result<Vec<PendingRequest>, SkillRunError> {
        let log = run_log(&self.request, &self.workspace, &self.manifest.skill)?;
        Ok(waiting_requests(log.store(), log.run_id())?)
    }

    /// The run waits on every request open in it, at any depth.
    fn paused(&self, log: &RunLog) -> Result<RunResult, SkillRunError> {
        let waiting = waiting_requests(log.store(), log.run_id())?;
        Ok(paused_output(
            &self.manifest,
            &self.runner.name,
            log.run_id(),
            &waiting,
        ))
    }

    /// A sealed run returns what it sealed, from its record alone.
    fn restore_sealed_run(
        &self,
        log: &RunLog,
        record: RunRecord,
    ) -> Result<RunResult, SkillRunError> {
        let sealed = record
            .sealed
            .clone()
            .ok_or_else(|| invalid("the run has no seal"))?;
        let graph = bind_run_inputs(
            self.runner.graph.clone(),
            &record.inputs,
            record.act.as_ref(),
        )?;
        let run = sealed_graph_run(graph, record)?;
        write_graph_receipts(&self.request, &self.workspace, &self.receipts, &run)?;
        let result = match sealed.terminal_result {
            Some(result) => result,
            None => graph_run_result(&run)?,
        };
        let output = graph_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            &self.manifest,
            &self.runner.name,
            log.run_id(),
            &output,
            &result,
            SkillOutputDiagnostics {
                context: Some(graph_run_context(&run)),
                trace: Some(graph_run_trace(&run)),
                refusal: None,
            },
            &run.receipt,
        ))
    }
}

/// The run a request names: its pinned id, or a new one.
fn run_log(
    request: &SkillRunRequest,
    workspace: &WorkspaceEnv,
    skill: &str,
) -> Result<RunLog, SkillRunError> {
    let run_id = match (&request.run_id, &request.answers_path) {
        // A caller-pinned id is a submission identity: a repeated submission
        // continues the same run, and hosted execution keys its effects on
        // it, so a retry recovers the same mutation instead of minting one.
        (Some(run_id), _) => crate::execution::run_record::validate_run_id(run_id)?.to_owned(),
        (None, Some(_)) => {
            return Err(invalid(
                "skill continuation requires both run_id and answers",
            ));
        }
        (None, None) => crate::execution::run_record::mint_run_id(skill)?,
    };
    let store = KernelStore::for_project(workspace.paths())
        .map_err(crate::execution::run_record::RunRecordError::from)
        .map_err(RuntimeError::from)?;
    Ok(RunLog::open(store, run_id).map_err(RuntimeError::from)?)
}

/// A continuation must present the identity its run was started with. A
/// request that leaves inputs or context empty continues with the recorded
/// ones.
fn continuation_identity<'a>(
    record: &'a RunRecord,
    request: &SkillRunRequest,
    request_context: &RunContext,
    runner: &SkillRunnerDefinition,
    package_digest: &str,
    execution_closure_digest: &str,
) -> Result<&'a crate::execution::run_record::Started, SkillRunError> {
    let Origin::Run(started) = &record.origin else {
        return Err(invalid("a child run continues only through its parent"));
    };
    let refuse = |field: &str| {
        invalid(format!(
            "run identity cannot replace its original {field}: a continuation runs under the identity the run started with"
        ))
    };
    if started.runner != runner.name {
        return Err(refuse("runner"));
    }
    if started.package_digest != package_digest {
        return Err(refuse("package digest"));
    }
    if started.execution_closure_digest != execution_closure_digest {
        return Err(refuse("execution closure"));
    }
    let request_inputs = request
        .inputs
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<JsonObject>();
    if !request_inputs.is_empty() && request_inputs != record.inputs {
        return Err(refuse("inputs"));
    }
    if (!request_context.current.is_empty() || !request_context.historical.is_empty())
        && request_context != &record.context
    {
        return Err(refuse("agent context"));
    }
    if request.run_authority.is_some() && request.run_authority != started.run_authority {
        return Err(refuse("run authority"));
    }
    if request.act.is_some() && request.act != started.act {
        return Err(refuse("act"));
    }
    Ok(started)
}

/// Why a run stopped short of completion.
enum Stop {
    /// A guard or an authority refused a step. The run returns what its
    /// steps produced, and the refusal's public projection beside it,
    /// outside the signed receipt.
    Blocked(RuntimeError),
    /// A step sealed its own governed closure (an agent act that declined,
    /// deferred, blocked, or was superseded). The run stops with that
    /// disposition and returns the step's declared outputs.
    Closed(ClosureDisposition),
    Failed(RuntimeError),
}

/// A run being driven, and what sealing it needs.
struct Driven<'a> {
    bound: &'a BoundRun,
    log: &'a RunLog,
    runtime: &'a Runtime<ExecutableAdapter>,
    graph: &'a ExecutionGraph,
}

impl Driven<'_> {
    fn seal_completed(&self, run: GraphRun) -> Result<RunResult, SkillRunError> {
        let bound = self.bound;
        let result = graph_run_result(&run)?;
        self.log.record_sealed(&run.receipt, None)?;
        write_graph_receipts(&bound.request, &bound.workspace, &bound.receipts, &run)?;
        let output = graph_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            &bound.manifest,
            &bound.runner.name,
            self.log.run_id(),
            &output,
            &result,
            SkillOutputDiagnostics {
                context: Some(graph_run_context(&run)),
                trace: Some(graph_run_trace(&run)),
                refusal: None,
            },
            &run.receipt,
        ))
    }

    fn seal_stopped(
        &self,
        step_id: &str,
        reason: Option<SealReason>,
        stop: Stop,
    ) -> Result<RunResult, SkillRunError> {
        let bound = self.bound;
        let mut host = RunHost::new(ResolutionAnswers::default());
        let failure_result = match &stop {
            Stop::Failed(error) => Some(JsonValue::Object(error.public_failure_projection())),
            Stop::Blocked(_) | Stop::Closed(_) => None,
        };
        let refusal = match &stop {
            Stop::Blocked(error) => Some(error.public_failure_projection()),
            Stop::Failed(_) | Stop::Closed(_) => None,
        };
        let (disposition, failure) = match stop {
            Stop::Blocked(_) => (ClosureDisposition::Blocked, None),
            Stop::Closed(disposition) => (disposition, None),
            Stop::Failed(error) => (ClosureDisposition::Failed, Some(error)),
        };
        let closure = crate::receipts::GraphClosure {
            disposition,
            reason,
        };
        let run = match failure {
            Some(error) => self.runtime.seal_failed_with_host(
                self.graph.clone(),
                step_id,
                error,
                closure,
                &mut host,
            )?,
            None => self.runtime.seal_stopped_with_host(
                self.graph.clone(),
                step_id,
                closure,
                &mut host,
            )?,
        };
        let result = match failure_result {
            Some(result) => result,
            None => crate::execution::runner::graph_terminal_result(&run)?,
        };
        self.log.record_sealed(&run.receipt, Some(&result))?;
        write_graph_receipts(&bound.request, &bound.workspace, &bound.receipts, &run)?;
        let output = graph_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            &bound.manifest,
            &bound.runner.name,
            self.log.run_id(),
            &output,
            &result,
            SkillOutputDiagnostics {
                context: Some(graph_run_context(&run)),
                trace: Some(graph_run_trace(&run)),
                refusal,
            },
            &run.receipt,
        ))
    }
}

/// An action on one open turn of a paused run, by the turn's address.
pub(super) struct TurnAct {
    pub(super) turn: String,
    pub(super) action: TurnAction,
}

pub(super) enum TurnAction {
    Status,
    /// Make the call the turn offers as `tool`.
    Call {
        tool: String,
        input: JsonObject,
    },
    /// Answer the act; the run continues.
    Complete(JsonObject),
}

/// What an action on a turn did.
pub(super) enum TurnActed {
    Status(Box<TurnStatus>),
    Call(TurnCallResult),
    Completed,
}

/// The host a run's driver works it through: the answers the caller
/// brought, an action on one of the run's open turns, a harness caller's
/// scripted turns, or the managed agent the run consents to. Anything else
/// suspends the run where it asks.
pub(super) struct RunHost {
    answers: ResolutionAnswers,
    /// The turns a harness caller works call by call, by request id.
    scripts: std::rc::Rc<BTreeMap<String, runx_parser::harness_fixture::HarnessTurnScript>>,
    /// The answers turns are given while the run is driven, by address.
    given: BTreeMap<String, JsonValue>,
    act: Option<TurnAct>,
    /// What the act did, once the run reached its turn.
    acted: Option<TurnActed>,
    #[cfg(feature = "agent")]
    managed: Option<std::rc::Rc<crate::adapters::managed_agent::ManagedAgent>>,
}

impl RunHost {
    fn new(answers: ResolutionAnswers) -> Self {
        Self {
            answers,
            scripts: std::rc::Rc::default(),
            given: BTreeMap::new(),
            act: None,
            acted: None,
            #[cfg(feature = "agent")]
            managed: None,
        }
    }

    /// A host that does one thing on one of a paused run's turns.
    pub(super) fn acting(act: TurnAct) -> Self {
        Self {
            act: Some(act),
            ..Self::new(ResolutionAnswers::default())
        }
    }

    /// What the act did; `None` when the run never reached its turn.
    pub(super) fn acted(&mut self) -> Option<TurnActed> {
        self.acted.take()
    }

    #[cfg(feature = "agent")]
    fn manages(&self) -> bool {
        self.managed.is_some()
    }

    #[cfg(not(feature = "agent"))]
    fn manages(&self) -> bool {
        false
    }

    /// Whether the host brings nothing a paused run could continue with.
    fn is_idle(&self) -> bool {
        !self.manages() && self.answers.is_empty() && self.act.is_none() && self.scripts.is_empty()
    }

    /// An action on a turn needs that turn open.
    fn check_turn(&self, run_id: &str, waiting: &[PendingRequest]) -> Result<(), SkillRunError> {
        let Some(act) = &self.act else {
            return Ok(());
        };
        if waiting
            .iter()
            .any(|request| request.id == act.turn && is_agent_act(request))
        {
            return Ok(());
        }
        Err(invalid(format!(
            "run {run_id} has no open turn {}",
            act.turn
        )))
    }

    /// One driver works a run's turns: the one it started with. Answers
    /// alone drive no turn.
    fn check_driver(&self, run_id: &str, driver: Option<Driver>) -> Result<(), SkillRunError> {
        let attached = self.act.is_some() || !self.scripts.is_empty();
        let drives = self.manages() || attached;
        match driver {
            None if drives => Err(invalid(format!(
                "run {run_id} was started before runs recorded their driver; no driver can work its turns, so start it again"
            ))),
            Some(Driver::Attached) if self.manages() => Err(invalid(format!(
                "run {run_id} is driven by the host it is attached to; the managed agent cannot work its turns"
            ))),
            Some(Driver::Managed) if attached => Err(invalid(format!(
                "run {run_id} is driven by the managed agent; another host cannot work its turns"
            ))),
            None | Some(Driver::Attached | Driver::Managed) => Ok(()),
        }
    }

    /// Take further each pending call of `turn` this host brings something
    /// for.
    fn settle(&mut self, turn: &mut dyn Turn) -> Result<(), RuntimeError> {
        let mut reached = Vec::new();
        for call in turn.calls()? {
            if let TurnCallOutcome::Pending { waiting } = &call.outcome
                && self.reaches(waiting)?
            {
                reached.push(call.index);
            }
        }
        for index in reached {
            turn.settle(self, index)?;
        }
        Ok(())
    }

    /// Whether this host brings something for a call that waits on
    /// `waiting`: an answer to one of them, an action on one of its turns, a
    /// script for one of its turns, or the managed agent for a turn. A call
    /// that waits on nothing, a child run that sealed or never started, is
    /// taken further by any host.
    fn reaches(&self, waiting: &[ResolutionRequest]) -> Result<bool, RuntimeError> {
        if waiting.is_empty() {
            return Ok(true);
        }
        for request in waiting {
            let id = resolution_request_id(request);
            let digest = crate::execution::run_record::request_digest(request)?;
            let agent_act = matches!(request, ResolutionRequest::AgentAct { .. });
            if self.answers.get(id, &digest).is_some()
                || self.act.as_ref().is_some_and(|act| act.turn == id)
                || (agent_act
                    && (self.scripts.contains_key(id)
                        || self
                            .scripts
                            .contains_key(crate::execution::run_record::local_request_id(id))))
                || (self.manages() && agent_act)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Host for RunHost {
    fn report(&mut self, _event: runx_contracts::ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        let request_id = resolution_request_id(&request).to_owned();
        if let Some(answer) = self.given.remove(&request_id) {
            return Ok(Some(ResolutionResponse {
                actor: ResolutionResponseActor::Agent,
                payload: answer,
            }));
        }
        let current = crate::execution::run_record::request_digest(&request)?;
        let Some(answer) = self.answers.get(&request_id, &current).cloned() else {
            return Ok(None);
        };
        let actor = if self.answers.is_human_approval(&request_id) {
            ResolutionResponseActor::Human
        } else {
            ResolutionResponseActor::Agent
        };
        // An answer answers its request: a different request at the same
        // address waits.
        self.answers.spend(&request_id, &current);
        Ok(Some(ResolutionResponse {
            actor,
            payload: answer,
        }))
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }

    /// The calls this host brings something for go further first. Then the
    /// host does what it came to do on this turn, works it from a harness
    /// script, or its managed agent works it once nothing in it waits on a
    /// human.
    fn drive_turn(&mut self, turn: &mut dyn Turn) -> Result<(), RuntimeError> {
        self.settle(turn)?;
        let address = turn.invocation().id.as_str().to_owned();
        if let Some(act) = self.act.take_if(|act| act.turn == address) {
            self.acted = Some(match act.action {
                TurnAction::Status => TurnActed::Status(Box::new(turn.status()?)),
                TurnAction::Call { tool, input } => {
                    TurnActed::Call(call_tool(turn, self, &tool, input)?)
                }
                TurnAction::Complete(output) => {
                    self.given.insert(address, JsonValue::Object(output));
                    TurnActed::Completed
                }
            });
            return Ok(());
        }
        let scripts = std::rc::Rc::clone(&self.scripts);
        let answered = self.answers.answers_address(&address);
        if let Some(output) =
            crate::execution::harness::turns::work_scripted_turn(turn, self, &scripts, answered)?
        {
            self.given.insert(address, output);
            return Ok(());
        }
        #[cfg(feature = "agent")]
        if let Some(managed) = self.managed.clone()
            && turn
                .calls()?
                .iter()
                .all(|call| !matches!(call.outcome, TurnCallOutcome::Pending { .. }))
            && let Some(output) = managed.drive(turn, self)?
        {
            self.given.insert(address, output);
        }
        Ok(())
    }
}

fn is_agent_act(request: &PendingRequest) -> bool {
    request
        .value
        .as_object()
        .and_then(|value| value.get("kind"))
        == Some(&JsonValue::String("agent_act".to_owned()))
}

fn resolution_request_id(request: &ResolutionRequest) -> &str {
    match request {
        ResolutionRequest::Input { id, .. }
        | ResolutionRequest::Approval { id, .. }
        | ResolutionRequest::AgentAct { id, .. }
        | ResolutionRequest::Retry { id, .. } => id.as_str(),
    }
}

fn write_graph_receipts(
    request: &SkillRunRequest,
    workspace: &WorkspaceEnv,
    receipts: &ReceiptServices,
    run: &GraphRun,
) -> Result<(), SkillRunError> {
    for step in &run.steps {
        for receipt in &step.nested_receipts {
            write_skill_receipt(request, workspace, receipts, receipt)?;
        }
        write_skill_receipt(request, workspace, receipts, &step.receipt)?;
    }
    write_skill_receipt(request, workspace, receipts, &run.receipt)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn a_run_without_a_recorded_driver_takes_answers_but_no_driver() {
        let answers = RunHost::new(ResolutionAnswers::default());
        assert!(answers.check_driver("run_old", None).is_ok());
        let acting = RunHost::acting(TurnAct {
            turn: "agent.answer.output".to_owned(),
            action: TurnAction::Status,
        });
        let refused = acting
            .check_driver("run_old", None)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            refused.contains("started before runs recorded their driver"),
            "{refused}"
        );
        assert!(
            acting
                .check_driver("run_new", Some(Driver::Attached))
                .is_ok()
        );
        assert!(
            acting
                .check_driver("run_new", Some(Driver::Managed))
                .is_err()
        );
    }
    use crate::adapter::SkillAdapter;

    #[test]
    fn terminal_skill_failure_uses_only_an_exact_graph_step_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let graph = runx_parser::validate_graph(runx_parser::parse_graph_yaml(
            "name: authoring\nsteps:\n  - id: apply\n    skill: ./apply\n",
        )?)?;
        let owned = RuntimeError::SkillFailed {
            skill_name: "apply".to_owned(),
            message: "candidate harness failed".to_owned(),
        };
        let nested = RuntimeError::SkillFailed {
            skill_name: "runx.skill.apply".to_owned(),
            message: "candidate harness failed".to_owned(),
        };

        assert_eq!(
            crate::execution::runner::failed_step_id(&graph, &owned).as_deref(),
            Some("apply")
        );
        assert_eq!(
            crate::execution::runner::failed_step_id(&graph, &nested),
            None
        );
        Ok(())
    }

    #[test]
    fn the_executable_adapter_refuses_an_agent_act() {
        let invocation = SkillInvocation {
            historical_context: Vec::new(),
            skill_name: "fixture".to_owned(),
            step_id: None,
            artifacts: None,
            requirements: Default::default(),
            executable: crate::adapter::Executable::Agent(runx_parser::AgentTask {
                role: "reviewer".to_owned(),
                task: "review".to_owned(),
                allowance: Default::default(),
                connectors: Default::default(),
            }),
            outputs: None,
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: PathBuf::from("."),
            env: BTreeMap::new(),
            credential_delivery: crate::credentials::CredentialDelivery::none(),
        };

        let result = ExecutableAdapter.invoke(invocation);
        assert!(
            matches!(
                &result,
                Err(RuntimeError::UnsupportedExecutable { kind }) if kind == "agent"
            ),
            "unexpected agent act result: {result:?}"
        );
    }

    #[cfg(feature = "agent")]
    #[test]
    fn managed_agent_graph_failure_is_typed_sealed_and_visible_in_history()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::LocalReceiptStore;
        use crate::adapters::agent::{
            AgentExecutionTelemetry, AgentToolExecutionTrace, ManagedAgentError,
        };
        use crate::execution::run_record::{RunFilter, RunState, list_runs};
        use crate::receipts::archive::{ReceiptFilter, list_receipts};
        use runx_core::state_machine::GraphStatus;
        use runx_parser::{parse_graph_yaml, validate_graph};

        let signature = crate::receipts::RuntimeReceiptSignatureConfig::fixture()?;
        for suspended in [false, true] {
            let temp = tempfile::tempdir()?;
            let receipt_dir = temp.path().join("receipts");
            let env = BTreeMap::from([(
                crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
                temp.path().join(".runx").display().to_string(),
            )]);
            let graph = validate_graph(parse_graph_yaml(
                r#"
name: managed-agent-graph-failure
steps:
  - id: compose
    skill: ./compose
"#,
            )?)?;
            let store =
                KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, temp.path()))?;
            let log = RunLog::open(store, "run_managed_agent_failure")?;
            log.begin(StartRequest {
                skill: "managed-agent-graph-failure",
                skill_ref: "skill",
                runner: "default",
                package_digest: "sha256:package",
                execution_closure_digest: "sha256:closure",
                inputs: &JsonObject::new(),
                context: &RunContext::default(),
                run_authority: None,
                act: None,
                driver: crate::execution::run_record::Driver::Managed,
                created_at: "2026-07-24T00:00:00Z",
            })?;
            if suspended {
                log.record_request(
                    "compose",
                    1,
                    &ResolutionRequest::Retry {
                        id: "agent.compose.output".into(),
                        reason: "the turn is open".into(),
                        retry_after_seconds: 1,
                    },
                )?;
            }
            let mut options = RuntimeOptions::new(env.clone(), signature.clone(), log.clone());
            options.clock =
                crate::execution::runner::RunClock::Fixed("2026-07-24T00:00:00Z".to_owned());
            let runtime = Runtime::new(ExecutableAdapter, options);
            let error = ManagedAgentError::bounded_failure(
                runx_contracts::SealReason::ManagedAgentRoundBudgetExhausted,
                "Managed agent exceeded 3 tool-call rounds without finalizing.",
                AgentExecutionTelemetry {
                    rounds: Some(3),
                    model_calls: Some(4),
                    tool_calls: Some(3),
                    tools: Some(vec!["data.read".to_owned()]),
                    tool_executions: Some(vec![AgentToolExecutionTrace {
                        tool: "data.read".to_owned(),
                        status: "success".to_owned(),
                        receipt_id: Some("rct_child".to_owned()),
                        resolution_kind: None,
                    }]),
                },
            );
            let runtime_error = RuntimeError::managed_agent_resolution(
                "referenced-agent-runner",
                "agent.compose.output",
                error,
            )
            .at_graph_step("compose");
            let mut host = RunHost::new(ResolutionAnswers::default());
            let run = runtime.seal_failed_with_host(
                graph,
                "compose",
                runtime_error,
                crate::receipts::GraphClosure {
                    disposition: ClosureDisposition::Failed,
                    reason: Some(runx_contracts::SealReason::ManagedAgentRoundBudgetExhausted),
                },
                &mut host,
            )?;

            assert_eq!(run.state.status, GraphStatus::Failed);
            assert_eq!(run.receipt.seal.disposition, ClosureDisposition::Failed);
            assert_eq!(run.steps.len(), 1);
            assert_eq!(run.state.steps[0].attempts, 1);
            let step = &run.steps[0];
            assert_eq!(step.attempt, 1);
            assert_eq!(step.receipt.seal.disposition, ClosureDisposition::Failed);
            assert_eq!(
                step.receipt.seal.reason,
                Some(runx_contracts::SealReason::ManagedAgentRoundBudgetExhausted)
            );
            let encoded = serde_json::to_string(&step.receipt)?;
            assert!(!encoded.contains("prompt"));
            assert!(!encoded.contains("credential"));
            assert!(!encoded.contains("raw_output"));

            log.record_sealed(&run.receipt, None)?;
            let runs = list_runs(&env, temp.path(), &RunFilter::default())?;
            assert_eq!(runs.runs.len(), 1);
            assert_eq!(runs.runs[0].state, RunState::Sealed);
            assert_eq!(runs.runs[0].disposition, Some(ClosureDisposition::Failed));
            assert_eq!(runs.runs[0].pending_request_id, None);

            let request = SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: None,
                act: None,
                skill_path: temp.path().join("skill"),
                receipt_dir: Some(receipt_dir.clone()),
                run_id: None,
                answers_path: None,
                inputs: Default::default(),
                env: Default::default(),
                cwd: temp.path().to_path_buf(),
                managed_agent: Default::default(),
            };
            let workspace = WorkspaceEnv::new(Default::default(), temp.path().to_path_buf())?;
            let receipts = ReceiptServices::from_signature_config(signature.clone());
            write_graph_receipts(&request, &workspace, &receipts, &run)?;
            let store = LocalReceiptStore::new(&receipt_dir);
            let listing = list_receipts(
                &store,
                workspace.paths(),
                &ReceiptFilter::default(),
                signature.signature_policy(),
            )?;
            assert_eq!(listing.receipts.len(), 1);
            assert!(
                listing
                    .receipts
                    .iter()
                    .all(|receipt| receipt.status == "failed")
            );
            let with_steps = list_receipts(
                &store,
                workspace.paths(),
                &ReceiptFilter {
                    include_internal: true,
                    ..ReceiptFilter::default()
                },
                signature.signature_policy(),
            )?;
            assert_eq!(with_steps.receipts.len(), 2);
        }
        Ok(())
    }

    #[test]
    fn graph_process_uses_structured_credential_redaction() -> Result<(), Box<dyn std::error::Error>>
    {
        const MARKER: &str = "cli-credential-redaction-sentinel";
        let workspace = tempfile::tempdir()?;
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        env.insert(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().to_string_lossy().into_owned(),
        );
        env.remove(crate::paths::RUNX_PROJECT_DIR_ENV);
        let secret = "cli-credential-redaction-sentinel-\"quoted\\slash\ncontrol";
        let delivery = crate::credentials::CredentialDelivery::process_env(
            "twitter",
            "oauth1_user",
            "TWITTER_TOKEN",
            "ref:twitter:primary",
            secret,
        )?;
        let encoded = serde_json::to_string(&JsonValue::Object(JsonObject::from([
            (secret.to_owned(), JsonValue::String(secret.to_owned())),
            (
                "nested".to_owned(),
                JsonValue::Array(vec![JsonValue::String(secret.to_owned())]),
            ),
        ])))?;
        let invocation = SkillInvocation {
            historical_context: Vec::new(),
            skill_name: "credential-observation".to_owned(),
            step_id: None,
            artifacts: None,
            requirements: Default::default(),
            executable: crate::adapter::Executable::Process(runx_parser::ProcessTool {
                command: "/bin/sh".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "test -n \"$TWITTER_TOKEN\" && printf '%s' \"$1\"".to_owned(),
                    "runx-credential-redaction".to_owned(),
                    encoded,
                ],
                cwd: None,
                timeout_seconds: Some(5),
                input_mode: None,
                environment: Default::default(),
            }),
            outputs: None,
            inputs: JsonObject::new(),
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: workspace.path().to_path_buf(),
            env,
            credential_delivery: delivery,
        };

        let output = ExecutableAdapter.invoke(invocation)?;

        assert!(output.succeeded());
        assert!(!serde_json::to_string(&output.value)?.contains(MARKER));
        let projection =
            crate::execution::output_projection::project_step_claim(JsonObject::new(), None);
        let value = serde_json::to_string(&output.value)?;
        assert!(projection.outputs.is_empty());
        assert!(value.contains("[redacted-credential]"));
        assert!(!value.contains(MARKER));
        assert!(!format!("{output:?}").contains(MARKER));
        let credentials = serde_json::to_string(output.observations().credentials())?;
        assert!(credentials.contains("runx:credential:local:"));
        assert!(!credentials.contains(MARKER));

        let receipt = crate::receipts::step_receipt(
            crate::receipts::RunScope {
                run_id: "run_credential",
                graph_name: "credential_graph",
            },
            "credential_cli",
            1,
            &output,
            &JsonObject::new(),
            "2026-07-15T00:00:00Z",
            crate::receipts::RuntimeReceiptSignatureConfig::fixture()?.signature_policy(),
        )?;
        assert!(
            receipt.seal.criteria[0]
                .verification_refs
                .iter()
                .any(|reference| {
                    reference.reference_type == runx_contracts::ReferenceType::Credential
                })
        );
        assert!(!serde_json::to_string(&receipt)?.contains(MARKER));
        Ok(())
    }
}
