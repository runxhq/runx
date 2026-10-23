// Module rationale: the skill front's run driver keeps the run record,
// host resolution and sealing in one path: a run starts or continues from its
// record, takes what its host brings, runs, and seals.
use super::{
    BoundRun, ExecutableAdapter, SkillOutputDiagnostics, SkillRunError, invalid, paused_output,
    sealed_output,
};

use std::collections::BTreeMap;

use runx_contracts::{
    CallAct, JsonObject, JsonValue, ResolutionRequest, ResolutionResponse, ResolutionResponseActor,
    RunDriver, TurnCallOutcome, TurnCallResult, TurnStatus,
};
use runx_parser::{ExecutionGraph, SkillRunnerDefinition};

use crate::RuntimeError;
#[cfg(test)]
use crate::adapter::SkillInvocation;
use crate::execution::graph::bind_run_inputs;
use crate::execution::orchestrator::{RunResult, SkillRunRequest};
use crate::execution::prepared_skill::PreparedContext;
use crate::execution::run_record::{
    Hold, Origin, PendingRequest, RunContext, RunLog, RunRecord, StartRequest, waiting_requests,
};
use crate::execution::runner::turn::{Admission, AdmittedCall, ExecutedCall, Turn, admit_tool};
use crate::execution::runner::{
    GraphRun, RUNX_RUN_ID_ENV, Runtime, RuntimeOptions, graph_run_context, graph_run_result,
    graph_run_skill_output, graph_run_trace, sealed_graph_run,
};
use crate::host::Host;
use crate::kernel::authority::RunAuthority;
use crate::kernel::store::KernelStore;
use crate::services::{ReceiptServices, WorkspaceEnv};

use super::resolution_answers::ResolutionAnswers;
use super::runner_manifest::write_skill_receipt;

/// How many times a run waits for an independent provider readback before
/// it is sealed deferred.
const MAX_READBACK_RETRIES: u32 = 24;

impl BoundRun {
    /// Start the run, or continue it with the answers its request brings, and
    /// drive it as far as it goes.
    pub(crate) fn execute(&self) -> Result<RunResult, SkillRunError> {
        let answers = match &self.request.answers {
            Some(answers) => ResolutionAnswers::from_document(answers.clone())?,
            None => ResolutionAnswers::default(),
        };
        let mut host = RunHost::new(answers);
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
        let log = self.log()?;
        // A pass that executes a call apart from its session's driver writes
        // nothing of the run's own, and reads the run as it finds it.
        let log = if host.executes() { log.reading() } else { log };
        // A change a crash left half-applied is rolled back before the run
        // goes further.
        if host.recovers() {
            crate::filesystem::recover(log.store(), log.run_id())?;
        }
        let existing = log.load()?;
        let waiting = waiting_requests(log.store(), log.run_id())?;
        host.check_turn(log.run_id(), &waiting)?;
        host.answers.check_waiting(log.run_id(), &waiting)?;
        let request_context = RunContext {
            current: request.current_context.clone(),
            historical: request.historical_context.clone(),
        };
        let (graph_inputs, run_act, run_context, run_authority, prepared) = match &existing {
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
                    started.prepared.clone(),
                )
            }
            None if request.answers.is_some() => {
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
                let prepared = PreparedContext::from_env(self.workspace.env())?;
                log.begin(StartRequest {
                    skill: &self.manifest.skill,
                    skill_path: &request.skill_path.to_string_lossy(),
                    runner: &self.runner.name,
                    package_digest: &self.package_digest,
                    execution_closure_digest,
                    inputs: &inputs,
                    context: &request_context,
                    run_authority: request.run_authority.as_ref(),
                    act: request.act.as_ref(),
                    driver: if request.managed_agent.max_rounds().is_some() {
                        RunDriver::Managed
                    } else {
                        RunDriver::Attached
                    },
                    prepared: prepared.as_ref(),
                    purchase: self.purchase(),
                    created_at: &crate::time::now_iso8601(),
                })?;
                (
                    inputs,
                    request.act.clone(),
                    request_context,
                    request.run_authority.clone(),
                    prepared,
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
        // Every later step runs under the person's latest standing approvals:
        // those this host brings, or those last recorded, or the set the run
        // started with.
        let run_authority = run_authority.map(|mut authority| {
            if let Some(standing) = host
                .answers
                .standing()
                .map(<[_]>::to_vec)
                .or_else(|| existing.as_ref().and_then(|record| record.standing.clone()))
            {
                authority.standing_approvals = standing;
            }
            authority
        });
        let runtime = self.runtime(
            &log,
            run_context,
            run_authority,
            prepared.as_ref(),
            host.gate.clone(),
        )?;
        let graph = self.graph(&graph_inputs, run_act.as_ref(), log.run_id())?;
        let run = Driven {
            bound: self,
            log: &log,
            runtime: &runtime,
            graph: &graph,
        };
        let suspension = existing
            .as_ref()
            .and_then(|record| record.suspension.as_ref());
        // Every decision a person brings is the run's before it goes
        // further: recorded in the stream whose request it answers, so the
        // next driver to reach that request uses it, even after a crash.
        // Decisions recorded earlier that no step has used yet are used now.
        // The person's new standing approvals replace the run's set for every
        // later step, and approve each open local effect they cover.
        if let Some(approvals) = host.answers.standing().map(<[_]>::to_vec)
            && existing.is_some()
        {
            log.record_standing(&approvals)?;
            let artifacts = self.workspace.paths().artifacts();
            for request in &waiting {
                let request_value: ResolutionRequest = serde_json::to_value(&request.value)
                    .and_then(serde_json::from_value)
                    .map_err(|source| RuntimeError::json("reading a waited request", source))?;
                if crate::execution::runner::floor::covers(&request_value, &approvals, &artifacts)?
                {
                    host.answers.grant(&request.id, &request.digest);
                }
            }
        }
        if existing.is_some() {
            host.answers.join_recorded(log.admitted()?);
            for answer in host.answers.brought() {
                match answer.request_id.split_once('#') {
                    Some((path, _)) => log.child(path)?.record_answer(&answer)?,
                    None => log.record_answer(&answer)?,
                }
            }
        }
        if let Some(suspension) = suspension {
            match &suspension.hold {
                // A bounded retry is runx's own: it runs once it is due.
                Hold::Retry { due_at, .. } if !is_due(due_at.as_deref()) => {
                    return self.paused(&log);
                }
                // A claimed step runs again only on the person's
                // acknowledgement, which its lane checked.
                Hold::Claimed(retry) if host.answers.get(&retry.id, &retry.digest).is_none() => {
                    return self.paused(&log);
                }
                Hold::Retry { .. } | Hold::Claimed(_) => {}
                // Nothing new to continue with: the run still waits on what
                // only a host can answer. A run that waits on nothing, a child
                // that sealed before its parent step was recorded, goes on.
                Hold::Request(_) | Hold::Child(_) if host.is_idle() && !waiting.is_empty() => {
                    return self.paused(&log);
                }
                Hold::Request(_) | Hold::Child(_) => {}
            }
        }
        match runtime.run_graph_with_host(&self.skill_dir, graph.clone(), host) {
            Ok(completed) => run.seal_completed(completed),
            Err(RuntimeError::ResolutionPending { .. }) => self.paused(&log),
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
                        resend: runx_contracts::RetryResend::Safe,
                    },
                    std::time::SystemTime::now(),
                )?;
                self.paused(&log)
            }
            Err(error) => run.seal_stopped(error),
        }
    }

    /// The registry reference this run buys: its runner is one the registry
    /// sells and it was reached through the registry, so the kernel buys it
    /// rather than run it here (S10).
    fn purchase(&self) -> Option<&str> {
        let reference = self.overrides.purchase.as_deref()?;
        self.manifest
            .marketplace
            .as_ref()?
            .offers
            .contains_key(&self.runner.name)
            .then_some(reference)
    }

    /// The graph the run drives: the runner's own, or for a purchase one step
    /// that calls the runner through the registry, which the kernel buys as
    /// it buys any call.
    fn graph(
        &self,
        inputs: &JsonObject,
        act: Option<&CallAct>,
        run_id: &str,
    ) -> Result<ExecutionGraph, RuntimeError> {
        match self.purchase() {
            Some(reference) => Ok(crate::execution::graph::purchase_graph(
                reference,
                &self.runner.name,
                inputs,
            )),
            None => bind_run_inputs(self.runner.graph.clone(), inputs, act, run_id),
        }
    }

    /// The run's runtime for one pass: the skill's environment, the run's
    /// identity, authority and prepared context, and its log.
    fn runtime(
        &self,
        log: &RunLog,
        context: RunContext,
        run_authority: Option<RunAuthority>,
        prepared: Option<&PreparedContext>,
        effect_gate: Option<std::sync::Arc<crate::execution::runner::EffectGate>>,
    ) -> Result<Runtime<ExecutableAdapter>, RuntimeError> {
        let mut env = self.workspace.skill_env_for_skill(&self.skill_dir);
        PreparedContext::apply(prepared, &mut env)?;
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
            effect_gate,
            packages: Some(self.packages.clone()),
        };
        Ok(match &self.overrides.javascript {
            Some(javascript) => Runtime::with_native_services(
                ExecutableAdapter,
                options,
                javascript.clone(),
                crate::services::LocalArtifactService::default(),
            ),
            None => Runtime::new(ExecutableAdapter, options),
        })
    }

    /// Every request the run waits on, by address.
    pub(super) fn waiting(&self) -> Result<Vec<PendingRequest>, SkillRunError> {
        let log = self.log()?;
        Ok(waiting_requests(log.store(), log.run_id())?)
    }

    /// The run this binding drives.
    pub(super) fn run_id(&self) -> &str {
        self.driver.run_id()
    }

    /// The record of the run this binding drives.
    fn log(&self) -> Result<RunLog, SkillRunError> {
        let store = KernelStore::for_project(self.workspace.paths())
            .map_err(crate::execution::run_record::RunRecordError::from)
            .map_err(RuntimeError::from)?;
        Ok(RunLog::open(store, self.run_id()).map_err(RuntimeError::from)?)
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
        let graph = self.graph(&record.inputs, record.act.as_ref(), log.run_id())?;
        let run = sealed_graph_run(graph, record)?;
        write_graph_receipts(&self.request, &self.workspace, &self.receipts, &run)?;
        let result = match sealed.terminal_result {
            Some(result) => result,
            None => graph_run_result(&run)?,
        };
        let output = graph_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            &self.manifest.skill,
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
            &bound.manifest.skill,
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

    /// Seal the run where `error` stopped it. A run blocked by a guard or an
    /// authority returns what its steps produced, and the refusal's public
    /// projection beside it, outside the signed receipt; one a step's own
    /// governed closure stopped returns that step's declared outputs.
    fn seal_stopped(&self, error: RuntimeError) -> Result<RunResult, SkillRunError> {
        let bound = self.bound;
        let mut host = RunHost::new(ResolutionAnswers::default());
        let crate::execution::runner::Stopped { run, result } =
            self.runtime
                .seal_stop(self.graph.clone(), error, &mut host)?;
        let refusal = run.refusal.clone();
        self.log.record_sealed(&run.receipt, Some(&result))?;
        write_graph_receipts(&bound.request, &bound.workspace, &bound.receipts, &run)?;
        let output = graph_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            &bound.manifest.skill,
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
    /// Read a page of what a locator of the turn names.
    Read {
        locator: String,
        offset: u64,
        limit: Option<u64>,
    },
    /// Admit the call the turn offers as `tool`, under the id its host pins.
    Admit {
        tool: String,
        call_id: Option<String>,
        input: JsonObject,
    },
    /// Execute an admitted call, recording nothing.
    Execute(Box<AdmittedCall>),
    /// Record what an executed call came to.
    Conclude(Box<ExecutedCall>),
    /// Answer the act; the run continues.
    Complete(JsonObject),
    /// End the act as failed, saying why: its step fails, and a skill call
    /// that started this run settles failed with the reason.
    Fail(String),
}

/// What an action on a turn did.
pub(super) enum TurnActed {
    Status(Box<TurnStatus>),
    Read(Box<runx_contracts::TurnReadPage>),
    Admitted(Box<Admission>),
    Executed(Box<ExecutedCall>),
    Call(Box<TurnCallResult>),
    Completed,
}

/// The host a run's driver works it through: the answers the caller
/// brought, an action on one of the run's open turns, a harness caller's
/// scripted turns, or the managed agent the run consents to. Anything else
/// suspends the run where it asks.
pub(super) struct RunHost {
    answers: ResolutionAnswers,
    /// The answers turns are given while the run is driven, by address.
    given: BTreeMap<String, JsonValue>,
    act: Option<TurnAct>,
    /// Whether the act only records what an executed call came to.
    records: bool,
    /// What the act did, once the run reached its turn.
    acted: Option<TurnActed>,
    /// The calls a session executes apart from this pass, by turn and
    /// index: this pass never takes them further.
    in_flight: std::collections::BTreeSet<(String, u32)>,
    /// The gate the run's calls take where they reach the world, when its
    /// session executes calls concurrently.
    gate: Option<std::sync::Arc<crate::execution::runner::EffectGate>>,
    #[cfg(feature = "agent")]
    managed: Option<std::rc::Rc<crate::adapters::managed_agent::ManagedAgent>>,
}

impl RunHost {
    fn new(answers: ResolutionAnswers) -> Self {
        Self {
            answers,
            given: BTreeMap::new(),
            act: None,
            records: false,
            acted: None,
            in_flight: std::collections::BTreeSet::new(),
            gate: None,
            #[cfg(feature = "agent")]
            managed: None,
        }
    }

    /// A host that does one thing on one of a paused run's turns.
    pub(super) fn acting(act: TurnAct) -> Self {
        Self {
            records: matches!(act.action, TurnAction::Conclude(_)),
            act: Some(act),
            ..Self::new(ResolutionAnswers::default())
        }
    }

    /// What the act did; `None` when the run never reached its turn.
    pub(super) fn acted(&mut self) -> Option<TurnActed> {
        self.acted.take()
    }

    /// Work the run as a session whose calls execute concurrently: `gate` is
    /// what they take where they reach the world, and `in_flight` the calls
    /// executing apart from this pass.
    pub(super) fn in_session(
        mut self,
        gate: std::sync::Arc<crate::execution::runner::EffectGate>,
        in_flight: std::collections::BTreeSet<(String, u32)>,
    ) -> Self {
        self.gate = Some(gate);
        self.in_flight = in_flight;
        self
    }

    /// Whether this pass executes a call apart from the session's driver: it
    /// writes nothing of the run's own, so it reads the run as it finds it.
    pub(super) fn executes(&self) -> bool {
        self.act
            .as_ref()
            .is_some_and(|act| matches!(act.action, TurnAction::Execute(_)))
    }

    /// Whether this pass may roll back a change a crash left half-applied:
    /// never while a call of the session executes, whose change it would be.
    fn recovers(&self) -> bool {
        self.in_flight.is_empty() && !self.executes()
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
        !self.manages() && self.answers.is_empty() && self.act.is_none()
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

    /// One driver works a run's turns: the one it started with. A run the
    /// managed agent drives takes a person's approvals from any host, never
    /// another host's agent work.
    fn check_driver(&self, run_id: &str, driver: RunDriver) -> Result<(), SkillRunError> {
        match driver {
            RunDriver::Attached if self.manages() => Err(invalid(format!(
                "run {run_id} is driven by the host it is attached to; the managed agent cannot work its turns"
            ))),
            RunDriver::Managed
                if !self.manages() && (self.act.is_some() || self.answers.answers_agents()) =>
            {
                Err(invalid(format!(
                    "run {run_id} is driven by the managed agent; another host cannot work its turns"
                )))
            }
            RunDriver::Attached | RunDriver::Managed => Ok(()),
        }
    }

    /// Take further each pending call of `turn` this host brings something
    /// for. A pass that executes a call takes nothing else further: it only
    /// descends through the skill call whose child holds its act's turn, which
    /// records nothing while that child still waits.
    fn settle(&mut self, turn: &mut dyn Turn) -> Result<(), RuntimeError> {
        let address = turn.invocation().id.as_str().to_owned();
        let mut reached = Vec::new();
        for call in turn.calls()? {
            if let TurnCallOutcome::Pending { waiting } = &call.outcome
                && !self.in_flight.contains(&(address.clone(), call.index))
                && if self.executes() {
                    self.leads_to_act(waiting)
                } else {
                    self.reaches(waiting)?
                }
            {
                reached.push(call.index);
            }
        }
        for index in reached {
            turn.settle(self, index)?;
        }
        Ok(())
    }

    /// Whether a call that waits on `waiting` leads to this host's act: the
    /// turn the act is for waits in the call's child.
    fn leads_to_act(&self, waiting: &[ResolutionRequest]) -> bool {
        self.act.as_ref().is_some_and(|act| {
            waiting
                .iter()
                .any(|request| resolution_request_id(request) == act.turn)
        })
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
        Ok(Some(ResolutionResponse {
            actor,
            payload: answer,
        }))
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn records(&self) -> bool {
        self.records
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
                TurnAction::Read {
                    locator,
                    offset,
                    limit,
                } => TurnActed::Read(Box::new(turn.read(&locator, offset, limit)?)),
                TurnAction::Admit {
                    tool,
                    call_id,
                    input,
                } => TurnActed::Admitted(Box::new(admit_tool(turn, self, &tool, call_id, input)?)),
                TurnAction::Execute(admitted) => {
                    TurnActed::Executed(Box::new(turn.execute(self, *admitted)?))
                }
                TurnAction::Conclude(executed) => {
                    TurnActed::Call(Box::new(turn.conclude(*executed)?))
                }
                TurnAction::Complete(output) => {
                    self.given.insert(address, JsonValue::Object(output));
                    TurnActed::Completed
                }
                TurnAction::Fail(reason) => {
                    self.given.insert(address, failed_act(reason));
                    TurnActed::Completed
                }
            });
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

/// Whether a bounded retry due at `due_at` may run now. One recorded before
/// retries carried their time is due.
fn is_due(due_at: Option<&str>) -> bool {
    let parse = runx_core::policy::parse_rfc3339_moment;
    match due_at.map(|due| (parse(due), parse(&crate::time::now_iso8601()))) {
        Some((Some(due), Some(now))) => now >= due,
        _ => true,
    }
}

/// The answer of an act that failed: its closure, with why.
fn failed_act(reason: String) -> JsonValue {
    JsonValue::Object(JsonObject::from([(
        "closure".to_owned(),
        JsonValue::Object(JsonObject::from([
            ("outcome".to_owned(), JsonValue::String("failed".to_owned())),
            ("reason".to_owned(), JsonValue::String(reason)),
        ])),
    )]))
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
        | ResolutionRequest::Retry { id, .. }
        | ResolutionRequest::Purchase { id, .. } => id.as_str(),
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
    #[cfg(feature = "agent")]
    use runx_contracts::RunOutcome;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn a_managed_run_takes_approvals_but_no_other_hosts_agent_work() -> Result<(), SkillRunError> {
        let acting = RunHost::acting(TurnAct {
            turn: "answer".to_owned(),
            action: TurnAction::Status,
        });
        assert!(acting.check_driver("run", RunDriver::Attached).is_ok());
        assert!(acting.check_driver("run", RunDriver::Managed).is_err());

        let agent_work = RunHost::new(ResolutionAnswers::from_document(
            runx_contracts::ResumeDocument {
                answers: [(
                    "answer".to_owned(),
                    runx_contracts::BoundAnswer {
                        request_digest: "sha256:answer".into(),
                        value: JsonValue::String("done".to_owned()),
                    },
                )]
                .into(),
                ..Default::default()
            },
        )?);
        let refused = agent_work
            .check_driver("run", RunDriver::Managed)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(refused.contains("driven by the managed agent"), "{refused}");
        assert!(agent_work.check_driver("run", RunDriver::Attached).is_ok());

        let approvals = RunHost::new(ResolutionAnswers::default());
        assert!(approvals.check_driver("run", RunDriver::Managed).is_ok());
        Ok(())
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
        use runx_core::state_machine::GraphState;
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
                skill_path: "skill",
                runner: "default",
                package_digest: "sha256:package",
                execution_closure_digest: "sha256:closure",
                inputs: &JsonObject::new(),
                context: &RunContext::default(),
                run_authority: None,
                act: None,
                driver: RunDriver::Managed,
                prepared: None,
                purchase: None,
                created_at: "2026-07-24T00:00:00Z",
            })?;
            if suspended {
                log.record_request(
                    "compose",
                    1,
                    &ResolutionRequest::Retry {
                        id: "compose".into(),
                        reason: "the turn is open".into(),
                        retry_after_seconds: 1,
                        resend: runx_contracts::RetryResend::Safe,
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
            let runtime_error =
                RuntimeError::managed_agent_resolution("referenced-agent-runner", "compose", error)
                    .at_graph_step("compose");
            let mut host = RunHost::new(ResolutionAnswers::default());
            let run = runtime.seal_failed_with_host(
                graph,
                "compose",
                runtime_error,
                crate::receipts::GraphClosure {
                    outcome: RunOutcome::Failed,
                    reason: Some(runx_contracts::SealReason::ManagedAgentRoundBudgetExhausted),
                },
                &mut host,
            )?;

            assert_eq!(run.progress.state, GraphState::Failed);
            assert_eq!(run.receipt.seal.outcome, RunOutcome::Failed);
            assert_eq!(run.steps.len(), 1);
            assert_eq!(run.progress.steps[0].attempts, 1);
            let step = &run.steps[0];
            assert_eq!(step.attempt, 1);
            assert_eq!(step.receipt.seal.outcome, RunOutcome::Failed);
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
            assert_eq!(runs.runs[0].outcome, Some(RunOutcome::Failed));
            assert_eq!(runs.runs[0].pending_request_id, None);

            let request = SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: None,
                act: None,
                skill_path: temp.path().join("skill"),
                receipt_dir: Some(receipt_dir.clone()),
                run_id: None,
                answers: None,
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
                    .all(|receipt| receipt.outcome == "failed")
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
