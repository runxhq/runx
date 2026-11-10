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
use runx_parser::{SkillMethodDefinition, Workflow};

use crate::RuntimeError;
#[cfg(test)]
use crate::adapter::SkillInvocation;
use crate::execution::engine::turn::{Admission, AdmittedCall, ExecutedCall, Turn, admit_tool};
use crate::execution::engine::{
    RUNX_RUN_ID_ENV, Runtime, RuntimeOptions, WorkflowRun, sealed_workflow_run,
    workflow_run_context, workflow_run_result, workflow_run_skill_output, workflow_run_trace,
};
use crate::execution::orchestrator::{RunResult, SkillRunRequest};
use crate::execution::prepared_skill::PreparedContext;
use crate::execution::run_record::{
    CallerContext, Hold, Origin, PendingRequest, RunLog, RunRecord, StartRequest,
};
use crate::execution::workflow::bind_run_inputs;
use crate::host::Host;
use crate::kernel::authority::RunAuthority;
use crate::kernel::store::KernelStore;
use crate::services::{ReceiptServices, WorkspaceEnv};

use super::method_manifest::write_skill_receipt;
use super::resolution_answers::ResolutionAnswers;

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
        let closure_digest = self
            .closure_digest
            .as_deref()
            .ok_or_else(|| invalid("workflow execution requires an closure digest"))?;
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
        let waiting = log.waiting()?;
        host.check_turn(log.run_id(), &waiting)?;
        // What the run's inbox delivered while it was driven reaches it here,
        // under the driver, before the host's own answers.
        if !host.executes() {
            let mut answers = self.delivered(&log, &waiting)?;
            answers.absorb(std::mem::take(&mut host.answers));
            host.answers = answers;
        }
        host.answers.check_waiting(log.run_id(), &waiting)?;
        let request_context = CallerContext {
            current: request.current_context.clone(),
            historical: request.historical_context.clone(),
        };
        let javascript = self.overrides.javascript.clone().unwrap_or_default();
        let (workflow_inputs, run_act, run_context, run_authority, prepared, started) =
            match &existing {
                Some(record) => {
                    let started = continuation_identity(
                        record,
                        request,
                        &request_context,
                        &self.method,
                        &self.package_digest,
                        closure_digest,
                    )?;
                    host.check_driver(log.run_id(), started.driver)?;
                    (
                        record.inputs.clone(),
                        record.act.clone(),
                        record.context.clone(),
                        started.run_authority.clone(),
                        started.prepared.clone(),
                        Some(started),
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
                    (
                        inputs,
                        request.act.clone(),
                        request_context.clone(),
                        request.run_authority.clone(),
                        prepared,
                        None,
                    )
                }
            };
        if let Some(record) = existing.as_ref().filter(|record| record.sealed.is_some()) {
            return self.restore_sealed_run(&log, record.clone());
        }
        if existing.is_some() {
            crate::input_contract::materialize_complete_method_inputs(
                &self.method.inputs,
                &self.method.input_constraints,
                &workflow_inputs,
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
        // The run holds what its host gave it, narrowed by its method's own
        // effects and reach.
        let run_authority = RunAuthority::of_method(run_authority, &self.method.workflow);
        let runtime = self.runtime(
            &log,
            run_context,
            run_authority,
            prepared.as_ref(),
            host,
            javascript,
        )?;
        // The run starts as its own method needs: its scripts' declared
        // operations admitted, then its worker started if it runs JavaScript.
        let start = self.start(&runtime)?;
        match started {
            // A continuation runs on the implementation the run started on,
            // unless it's refused as it starts and runs nothing.
            Some(started) => {
                if let Some(reason) = start
                    .refusal
                    .is_none()
                    .then(|| {
                        crate::implementation::refusal(
                            &started.implementation,
                            &start.implementation,
                        )
                    })
                    .flatten()
                {
                    return Err(invalid(format!(
                        "run identity cannot replace its original implementation: {reason}"
                    )));
                }
            }
            None => log.begin(StartRequest {
                skill: &self.manifest.skill,
                skill_path: &request.skill_path.to_string_lossy(),
                method: &self.method.name,
                package_digest: &self.package_digest,
                closure_digest,
                inputs: &workflow_inputs,
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
                implementation: &start.implementation,
                ran_on: crate::execution_environment::ran_on(&request.env),
                created_at: &crate::time::now_iso8601(),
            })?,
        }
        let workflow = self.workflow(&workflow_inputs, run_act.as_ref(), log.run_id())?;
        let run = Driven {
            bound: self,
            log: &log,
            runtime: &runtime,
            workflow: &workflow,
        };
        // A run refused as it starts runs nothing, and seals with the refusal
        // as its step's admission would.
        if let Some(refusal) = start.refusal {
            return run.seal_stopped(refusal);
        }
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
                if crate::execution::engine::floor::covers(&request_value, &approvals, &artifacts)?
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
                // that sealed before its parent step was recorded, goes on,
                // and so does one with a wait that's due, which is runx's own.
                Hold::Request(_) | Hold::Child(_)
                    if host.is_idle()
                        && !waiting.is_empty()
                        && !waiting.iter().any(is_due_wait) =>
                {
                    return self.paused(&log);
                }
                Hold::Request(_) | Hold::Child(_) => {}
            }
        }
        match runtime.run_workflow_with_host(&self.skill_dir, workflow.clone(), host) {
            Ok(completed) => run.seal_completed(completed),
            Err(RuntimeError::ResolutionPending { .. }) => self.paused(&log),
            // The mutation may be applied and its readback is pending. The
            // run stays open, unsealed, so the call seals once, when its
            // outcome is known: retried while readback may yet arrive, then
            // returned to the operator to reconcile before resuming.
            Err(RuntimeError::ProviderReadbackPending {
                step_id,
                reason,
                resend,
            }) => {
                if resend == runx_contracts::RetryResend::Unsafe {
                    // Keep the existing claimed-step hold. Its operator
                    // acknowledgement cannot bypass the kernel's unresolved
                    // charge custody or authorize an automatic resend.
                    return self.paused(&log);
                }
                let retries = existing.as_ref().map_or(0, |record| record.retries) + 1;
                if retries > MAX_READBACK_RETRIES {
                    return Err(RuntimeError::ProviderReadbackPending {
                        step_id,
                        reason,
                        resend,
                    }
                    .into());
                }
                log.record_retry(
                    &step_id,
                    &ResolutionRequest::Retry {
                        id: format!("provider-readback-{step_id}-{retries}").into(),
                        reason: "Independent provider readback is pending".into(),
                        retry_after_seconds: 300,
                        resend,
                    },
                    std::time::SystemTime::now(),
                )?;
                self.paused(&log)
            }
            // The method recorded the exact claimed step before dispatch.
            // Return that pending workflow, not an anonymous CLI failure.
            Err(RuntimeError::ProviderEffectUnknown { .. }) => self.paused(&log),
            Err(error) => run.seal_stopped(error),
        }
    }

    /// The registry reference this run buys: its method is one the registry
    /// sells and it was reached through the registry, so the kernel buys it
    /// rather than run it here (S10).
    fn purchase(&self) -> Option<&str> {
        let reference = self.overrides.purchase.as_deref()?;
        self.method.offer.is_some().then_some(reference)
    }

    /// How the run starts on `runtime`, decided from what it runs as its own
    /// method: for a purchase, the one step that calls the method, whose
    /// child run decides from the method itself.
    fn start(
        &self,
        runtime: &Runtime<ExecutableAdapter>,
    ) -> Result<crate::implementation::Start, RuntimeError> {
        let own = match self.purchase() {
            Some(reference) => {
                std::borrow::Cow::Owned(crate::execution::workflow::purchase_workflow(
                    reference,
                    &self.method.name,
                    &JsonObject::new(),
                ))
            }
            None => std::borrow::Cow::Borrowed(&self.method.workflow),
        };
        crate::implementation::start(runtime, &self.skill_dir, &own, false)
    }

    /// The workflow the run drives: the method's own, or for a purchase one step
    /// that calls the method through the registry, which the kernel buys as
    /// it buys any call.
    fn workflow(
        &self,
        inputs: &JsonObject,
        act: Option<&CallAct>,
        run_id: &str,
    ) -> Result<Workflow, RuntimeError> {
        match self.purchase() {
            Some(reference) => Ok(crate::execution::workflow::purchase_workflow(
                reference,
                &self.method.name,
                inputs,
            )),
            None => bind_run_inputs(
                self.method.workflow.clone(),
                inputs,
                act,
                run_id,
                Some(self.workspace.paths()),
            ),
        }
    }

    /// The run's runtime for one pass: the skill's environment, the run's
    /// identity, authority and prepared context, its log, the stops its host
    /// brings, and the JavaScript session it started.
    fn runtime(
        &self,
        log: &RunLog,
        context: CallerContext,
        run_authority: Option<RunAuthority>,
        prepared: Option<&PreparedContext>,
        host: &RunHost,
        javascript: crate::adapters::javascript::JavaScriptAdapter,
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
            clock: crate::execution::engine::RunClock::System,
            env,
            receipt_signature: self.receipts.signature_config().clone(),
            effects: self.effects.clone(),
            run: log.clone(),
            call_stops: host.stops.clone().unwrap_or_default(),
            packages: Some(self.packages.clone()),
            chain: Vec::new(),
            reads: crate::execution::engine::RunReads::default(),
            workspace: Some(self.workspace.paths().clone()),
            max_concurrency: self.max_concurrency,
            max_receipts: self.max_receipts,
            receipts: host.receipts.clone().unwrap_or_default(),
            pool: host.pool.clone().unwrap_or_default(),
        };
        let local_artifacts = crate::services::LocalArtifactService::for_run(
            &options.run.store().project_dir(),
            crate::execution::run_record::root_run_id(options.run.run_id()),
        );
        Ok(Runtime::with_native_services(
            ExecutableAdapter,
            options,
            javascript,
            local_artifacts,
        ))
    }

    /// Every request the run waits on, by address.
    pub(super) fn waiting(&self) -> Result<Vec<PendingRequest>, SkillRunError> {
        let log = self.log()?;
        Ok(log.waiting()?)
    }

    /// What the run's inbox delivered since this driver last took it, where
    /// it still answers what the run waits on, `waiting`, by digest and lane:
    /// the answers a resume brought while a driver held the run. A delivery
    /// whose request was answered since, or changed, is passed over, so a
    /// driver that takes the inbox from its start applies only what still
    /// waits.
    fn delivered(
        &self,
        log: &RunLog,
        waiting: &[PendingRequest],
    ) -> Result<ResolutionAnswers, SkillRunError> {
        let mut through = self
            .inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (read_to, documents) = crate::execution::run_record::inbox::delivered_after(
            log.store(),
            log.run_id(),
            *through,
        )
        .map_err(crate::execution::run_record::RunRecordError::from)
        .map_err(RuntimeError::from)?;
        *through = read_to;
        let mut answers = ResolutionAnswers::default();
        for document in documents {
            let mut delivered = ResolutionAnswers::from_document(document)?;
            delivered.retain_waiting(waiting);
            if delivered.check_waiting(log.run_id(), waiting).is_ok() {
                answers.absorb(delivered);
            }
        }
        Ok(answers)
    }

    /// The run this binding drives.
    pub(super) fn run_id(&self) -> &str {
        self.driver.run_id()
    }

    /// The record of the run this binding drives: one log for every pass,
    /// which reads again only what the run committed since it last read.
    fn log(&self) -> Result<RunLog, SkillRunError> {
        if let Some(log) = self.log.get() {
            return Ok(log.clone());
        }
        let store = KernelStore::for_project(self.workspace.paths())
            .map_err(crate::execution::run_record::RunRecordError::from)
            .map_err(RuntimeError::from)?;
        let log = RunLog::open(store, self.run_id()).map_err(RuntimeError::from)?;
        Ok(self.log.get_or_init(|| log).clone())
    }

    /// The run waits on every request open in it, at any depth.
    fn paused(&self, log: &RunLog) -> Result<RunResult, SkillRunError> {
        let waiting = log.waiting()?;
        Ok(paused_output(
            &self.manifest,
            &self.method.name,
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
        let workflow = self.workflow(&record.inputs, record.act.as_ref(), log.run_id())?;
        let run = sealed_workflow_run(workflow, record, log.store())?;
        write_workflow_receipts(&self.request, &self.workspace, &self.receipts, &run)?;
        let result = match sealed.terminal_result {
            Some(result) => result,
            None => workflow_run_result(&run)?,
        };
        let output = workflow_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            &self.manifest.skill,
            &self.method.name,
            log.run_id(),
            &output,
            &result,
            SkillOutputDiagnostics {
                context: Some(workflow_run_context(&run)),
                trace: Some(workflow_run_trace(&run)),
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
    request_context: &CallerContext,
    method: &SkillMethodDefinition,
    package_digest: &str,
    closure_digest: &str,
) -> Result<&'a crate::execution::run_record::Started, SkillRunError> {
    let Origin::Run(started) = &record.origin else {
        return Err(invalid("a child run continues only through its parent"));
    };
    let refuse = |field: &str| {
        invalid(format!(
            "run identity cannot replace its original {field}: a continuation runs under the identity the run started with"
        ))
    };
    if started.method != method.name {
        return Err(refuse("method"));
    }
    if started.package_digest != package_digest {
        return Err(refuse("package digest"));
    }
    if started.closure_digest != closure_digest {
        return Err(refuse("closure"));
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
    workflow: &'a Workflow,
}

impl Driven<'_> {
    fn seal_completed(&self, run: WorkflowRun) -> Result<RunResult, SkillRunError> {
        let bound = self.bound;
        let result = workflow_run_result(&run)?;
        self.log.record_sealed(&run.receipt, None)?;
        write_workflow_receipts(&bound.request, &bound.workspace, &bound.receipts, &run)?;
        let output = workflow_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            &bound.manifest.skill,
            &bound.method.name,
            self.log.run_id(),
            &output,
            &result,
            SkillOutputDiagnostics {
                context: Some(workflow_run_context(&run)),
                trace: Some(workflow_run_trace(&run)),
                refusal: None,
            },
            &run.receipt,
        ))
    }

    /// Seal the run where `error` stopped it. A run blocked by a guard or an
    /// authority returns what its steps produced, and the refusal's public
    /// projection beside it, outside the signed receipt; one a step's own
    /// governed ending stopped returns that step's declared outputs.
    fn seal_stopped(&self, error: RuntimeError) -> Result<RunResult, SkillRunError> {
        let bound = self.bound;
        let mut host = RunHost::new(ResolutionAnswers::default());
        let crate::execution::engine::Stopped { run, result } =
            self.runtime
                .seal_stop(self.workflow.clone(), error, &mut host)?;
        let refusal = run.refusal.clone();
        self.log.record_sealed(&run.receipt, Some(&result))?;
        write_workflow_receipts(&bound.request, &bound.workspace, &bound.receipts, &run)?;
        let output = workflow_run_skill_output(&result, &run)?;
        Ok(sealed_output(
            &bound.manifest.skill,
            &bound.method.name,
            self.log.run_id(),
            &output,
            &result,
            SkillOutputDiagnostics {
                context: Some(workflow_run_context(&run)),
                trace: Some(workflow_run_trace(&run)),
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
    /// Answer the act and go on from it as far as its run goes, `apart` from
    /// the driver for a turn in a child run, which then records nothing
    /// above that run: a completion's or a failure's continuation.
    Continue {
        answer: JsonValue,
        apart: bool,
    },
    /// Take further the calls whose child runs held the act, under the
    /// driver: a continuation's conclusion.
    Settle,
    /// Record what an executed call came to.
    Conclude(Box<ExecutedCall>),
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
/// brought, an action on one of the run's open turns, a test caller's
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
    /// The stops of the calls its session executes, through which the
    /// session's host cancels one.
    stops: Option<std::sync::Arc<crate::execution::engine::CallStops>>,
    /// What its session's run has drawn of its receipt budget, which every
    /// pass of the session draws from.
    receipts: Option<std::sync::Arc<crate::execution::engine::ReceiptTally>>,
    /// What its session's run has executing at once, within its
    /// `max_concurrency`, in every pass of the session.
    pool: Option<std::sync::Arc<crate::execution::engine::CallPool>>,
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
            stops: None,
            receipts: None,
            pool: None,
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

    /// Work the run as a session whose calls execute concurrently: `stops`
    /// where each is registered while it executes so the session's host can
    /// cancel it, `shared` the run's receipt tally and pool, which every pass
    /// draws from, and `in_flight` the calls executing apart from this pass.
    pub(super) fn in_session(
        mut self,
        stops: std::sync::Arc<crate::execution::engine::CallStops>,
        shared: &super::turn_session::SessionShared,
        in_flight: std::collections::BTreeSet<(String, u32)>,
    ) -> Self {
        self.stops = Some(stops);
        self.receipts = Some(std::sync::Arc::clone(&shared.receipts));
        self.pool = Some(std::sync::Arc::clone(&shared.pool));
        self.in_flight = in_flight;
        self
    }

    /// Whether this pass executes a call apart from the session's driver: it
    /// writes nothing of the run's own, so it reads the run as it finds it.
    pub(super) fn executes(&self) -> bool {
        self.act.as_ref().is_some_and(|act| {
            matches!(
                act.action,
                TurnAction::Execute(_) | TurnAction::Continue { apart: true, .. }
            )
        })
    }

    /// Whether this pass goes only where its act is: one that executes, or
    /// one that concludes a continuation.
    fn goes_to_act(&self) -> bool {
        self.executes()
            || self
                .act
                .as_ref()
                .is_some_and(|act| matches!(act.action, TurnAction::Settle))
    }

    /// Whether the call at `call` leads to this host's act: its child run is
    /// the run that holds the act's turn, or holds that run.
    fn leads_to_call(&self, call: &str) -> bool {
        self.act
            .as_ref()
            .is_some_and(|act| holds(&child_path(call), run_path(&act.turn)))
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

    /// An action on a turn needs that turn open, but for concluding its
    /// continuation, once its act was answered.
    fn check_turn(&self, run_id: &str, waiting: &[PendingRequest]) -> Result<(), SkillRunError> {
        let Some(act) = self
            .act
            .as_ref()
            .filter(|act| !matches!(act.action, TurnAction::Settle))
        else {
            return Ok(());
        };
        if waiting
            .iter()
            .any(|request| request.id == act.turn && is_turn(request))
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
            let leads = |host: &Self| {
                host.leads_to_act(&waiting_of(&call.outcome))
                    || host.leads_to_call(&format!("{address}/{}", call.index))
            };
            if let TurnCallOutcome::Pending { waiting } = &call.outcome
                && !self.in_flight.contains(&(address.clone(), call.index))
                && if self.goes_to_act() {
                    leads(self)
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
            let turn = matches!(request, ResolutionRequest::Turn { .. });
            if self.answers.get(id, &digest).is_some()
                || self.act.as_ref().is_some_and(|act| act.turn == id)
                || (self.manages() && turn)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// What a call waits on, as its outcome says: nothing once it settled.
fn waiting_of(outcome: &TurnCallOutcome) -> Vec<ResolutionRequest> {
    match outcome {
        TurnCallOutcome::Pending { waiting } => waiting.clone(),
        _ => Vec::new(),
    }
}

/// The path, below the root, of the run that raises the request at
/// `address`: empty at the root.
pub(super) fn run_path(address: &str) -> &str {
    address.split_once('#').map_or("", |(path, _)| path)
}

/// The path, below the root, of the child run the call at `call`, an
/// address in the run, starts.
fn child_path(call: &str) -> String {
    match call.split_once('#') {
        Some((path, local)) => format!("{path}/{local}"),
        None => call.to_owned(),
    }
}

/// Whether the run at `path` is the run at `held` or holds it.
fn holds(path: &str, held: &str) -> bool {
    !path.is_empty()
        && (held == path
            || held
                .strip_prefix(path)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// The call that starts the run holding the turn at `turn`, as the step at
/// whose position it is and its index: none for a turn of the root run.
pub(super) fn leading_call(turn: &str) -> Option<(String, u32)> {
    let path = run_path(turn);
    let (parent, index) = path.rsplit_once('/')?;
    let index = index.parse().ok()?;
    Some(match parent.rsplit_once('/') {
        Some((above, step)) => (format!("{above}#{step}"), index),
        None => (parent.to_owned(), index),
    })
}

/// The answer `given` or `answers` hold for `request`, by its address.
fn answer_from(
    given: &mut BTreeMap<String, JsonValue>,
    answers: &ResolutionAnswers,
    request: &ResolutionRequest,
) -> Result<Option<ResolutionResponse>, RuntimeError> {
    let request_id = resolution_request_id(request).to_owned();
    if let Some(answer) = given.remove(&request_id) {
        return Ok(Some(ResolutionResponse {
            actor: ResolutionResponseActor::Agent,
            payload: answer,
        }));
    }
    let current = crate::execution::run_record::request_digest(request)?;
    let Some(answer) = answers.get(&request_id, &current).cloned() else {
        return Ok(None);
    };
    let actor = if answers.is_human_approval(&request_id) {
        ResolutionResponseActor::Human
    } else {
        ResolutionResponseActor::Agent
    };
    Ok(Some(ResolutionResponse {
        actor,
        payload: answer,
    }))
}

impl Host for RunHost {
    /// Only a managed agent at a loopback endpoint is known to run here; a
    /// host that works the run's turns from outside may run its model anywhere.
    fn reads_local_only(&self) -> bool {
        #[cfg(feature = "agent")]
        {
            self.managed.as_ref().is_some_and(|agent| agent.is_local())
        }
        #[cfg(not(feature = "agent"))]
        {
            false
        }
    }

    fn report(&mut self, _event: runx_contracts::ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        answer_from(&mut self.given, &self.answers, &request)
    }

    /// A call apart from the driver is answered from the same answers.
    /// Turns stay on the driver, since its managed agent works them.
    fn detached(&self) -> Option<crate::host::DetachedHost> {
        let mut given = self.given.clone();
        let answers = self.answers.clone();
        Some(crate::host::DetachedHost::new(Box::new(move |request| {
            answer_from(&mut given, &answers, &request)
        })))
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn records(&self) -> bool {
        self.records
    }

    fn executes(&self) -> bool {
        RunHost::executes(self)
    }

    fn leads_to(&self, call: &str) -> bool {
        !self.goes_to_act() || self.leads_to_call(call)
    }

    fn executing(&self, step: &str, index: u32) -> bool {
        self.in_flight.contains(&(step.to_owned(), index))
    }

    /// The calls this host brings something for go further first. Then the
    /// host does what it came to do on this turn, works it from a test
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
                TurnAction::Continue { answer, .. } => {
                    self.given.insert(address, answer);
                    TurnActed::Completed
                }
                TurnAction::Settle => TurnActed::Completed,
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

/// Whether `request` is a script's wait the clock has reached.
fn is_due_wait(request: &PendingRequest) -> bool {
    let request = serde_json::to_value(&request.value).and_then(serde_json::from_value);
    match (
        request,
        crate::execution::engine::RunClock::System.now_millis(),
    ) {
        (Ok(ResolutionRequest::Wait { until, .. }), Some(now)) => now >= until,
        _ => false,
    }
}

fn is_turn(request: &PendingRequest) -> bool {
    request
        .value
        .as_object()
        .and_then(|value| value.get("kind"))
        == Some(&JsonValue::String("turn".to_owned()))
}

fn resolution_request_id(request: &ResolutionRequest) -> &str {
    match request {
        ResolutionRequest::Input { id, .. }
        | ResolutionRequest::Approval { id, .. }
        | ResolutionRequest::Turn { id, .. }
        | ResolutionRequest::Retry { id, .. }
        | ResolutionRequest::Purchase { id, .. }
        | ResolutionRequest::Wait { id, .. } => id.as_str(),
    }
}

fn write_workflow_receipts(
    request: &SkillRunRequest,
    workspace: &WorkspaceEnv,
    receipts: &ReceiptServices,
    run: &WorkflowRun,
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
    fn terminal_skill_failure_uses_only_an_exact_workflow_step_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let workflow = crate::execution::workflow::testing::method_workflow(
            "authoring",
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: apply\n        skill: ./apply\n",
        )?;
        let owned = RuntimeError::SkillFailed {
            skill_name: "apply".to_owned(),
            message: "candidate test failed".to_owned(),
        };
        let nested = RuntimeError::SkillFailed {
            skill_name: "runx.skill.apply".to_owned(),
            message: "candidate test failed".to_owned(),
        };

        assert_eq!(
            crate::execution::engine::failed_step_id(&workflow, &owned).as_deref(),
            Some("apply")
        );
        assert_eq!(
            crate::execution::engine::failed_step_id(&workflow, &nested),
            None
        );
        Ok(())
    }

    #[test]
    fn the_executable_adapter_refuses_an_turn() {
        let invocation = SkillInvocation {
            reach: runx_contracts::Reach::Inputs,
            historical_context: Vec::new(),
            skill_name: "fixture".to_owned(),
            step_id: None,
            artifacts: None,
            environment: Default::default(),
            executable: crate::adapter::Executable::Agent(runx_parser::AgentTask {
                task: "review".to_owned(),
                allowance: Default::default(),
                connectors: Default::default(),
                requires: Default::default(),
                context_kinds: Default::default(),
                context_entries: Default::default(),
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
            "unexpected turn result: {result:?}"
        );
    }

    #[cfg(feature = "agent")]
    #[test]
    fn managed_agent_workflow_failure_is_typed_sealed_and_visible_in_history()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::LocalReceiptStore;
        use crate::adapters::agent::{
            AgentExecutionTelemetry, AgentToolExecutionTrace, ManagedAgentError,
        };
        use crate::execution::run_record::{RunFilter, RunState, list_runs};
        use crate::receipts::archive::{ReceiptFilter, list_receipts};
        use runx_core::state_machine::WorkflowState;

        let signature = crate::receipts::RuntimeReceiptSignatureConfig::fixture()?;
        for suspended in [false, true] {
            let temp = tempfile::tempdir()?;
            let receipt_dir = temp.path().join("receipts");
            let env = BTreeMap::from([(
                crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
                temp.path().join(".runx").display().to_string(),
            )]);
            let workflow = crate::execution::workflow::testing::method_workflow(
                "managed-agent-workflow-failure",
                r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: compose
        skill: ./compose
"#,
            )?;
            let store =
                KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, temp.path()))?;
            let log = RunLog::open(store, "run_managed_agent_failure")?;
            log.begin(StartRequest {
                skill: "managed-agent-workflow-failure",
                skill_path: "skill",
                method: "default",
                package_digest: "sha256:package",
                closure_digest: "sha256:closure",
                inputs: &JsonObject::new(),
                context: &CallerContext::default(),
                run_authority: None,
                act: None,
                driver: RunDriver::Managed,
                prepared: None,
                purchase: None,
                implementation: &runx_contracts::RunImplementation {
                    runtime: "sha256:runtime".into(),
                    worker: None,
                },
                ran_on: runx_contracts::RanOn::Local,
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
                crate::execution::engine::RunClock::Fixed("2026-07-24T00:00:00Z".to_owned());
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
                RuntimeError::managed_agent_resolution("referenced-agent-method", "compose", error)
                    .at_workflow_step("compose");
            let mut host = RunHost::new(ResolutionAnswers::default());
            let run = runtime.seal_failed_with_host(
                workflow,
                "compose",
                runtime_error,
                crate::receipts::WorkflowEnding {
                    outcome: RunOutcome::Failed,
                    reason: Some(runx_contracts::SealReason::ManagedAgentRoundBudgetExhausted),
                },
                &mut host,
            )?;

            assert_eq!(run.progress.state, WorkflowState::Failed);
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
            write_workflow_receipts(&request, &workspace, &receipts, &run)?;
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
    fn workflow_process_uses_structured_credential_redaction()
    -> Result<(), Box<dyn std::error::Error>> {
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
            reach: runx_contracts::Reach::Inputs,
            historical_context: Vec::new(),
            skill_name: "credential-observation".to_owned(),
            step_id: None,
            artifacts: None,
            environment: Default::default(),
            executable: crate::adapter::Executable::Process(runx_parser::ProcessTool {
                command: "/bin/sh".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "test -n \"$TWITTER_TOKEN\" && printf '%s' \"$1\"".to_owned(),
                    "runx-credential-redaction".to_owned(),
                    encoded,
                ],
                cwd: None,
                timeout_seconds: None,
                inputs_via: None,
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
        let credentials = serde_json::to_string(output.observations.credentials())?;
        assert!(credentials.contains("runx:credential:local:"));
        assert!(!credentials.contains(MARKER));

        let receipt = crate::receipts::step_receipt(
            crate::receipts::RunScope {
                run_id: "run_credential",
                workflow_name: "credential_workflow",
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
