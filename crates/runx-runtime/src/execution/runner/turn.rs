//! Agent turns. An agent step whose act is unanswered holds its turn open,
//! and its host works the turn: the agent calls back into the run within the
//! act's allowance. A call runs as one step under the agent step's own
//! runtime, at whatever depth the step runs, so it is admitted, executed and
//! sealed exactly as the step kind it names would be, and it is recorded in
//! the run's stream before its outcome is returned. A skill call runs its
//! child skill as a child run of its own, whose turns are open turns too. The
//! turn ends when the host answers the act.

use std::cell::{Cell, OnceCell};
use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::{ConnectorBinding, OperationAccess, OperationName};
use runx_contracts::schema::{NonEmptyString, RunxSchema};
use runx_contracts::{
    AllowedOperation, Artifact, ArtifactLabels, BoundClosure, CallAct, CallTarget, InputDefinition,
    JsonObject, JsonValue, LoadedPackage, ProjectBinding, ProvenanceEntry, Reference,
    ReferenceType, ResolutionRequest, RunCallOutput, TurnAllowance, TurnCallOutcome,
    TurnCallResult, TurnInvocation, TurnLoad, TurnLocator, TurnReadPage, TurnStatus, TurnTool,
    TurnView,
};
use runx_parser::{
    AskGate, AskReason, ConnectorRequirement, LoadStep, LoadTarget, OperationStepTarget,
    SkillTarget, StepKind, ToolTarget, WorkflowStep,
};

use super::super::run_record::{
    Answer, CallOutcome, CallRecord, PendingRequest, RecordedCall, call_step_id, request_digest,
    waiting_requests,
};
use super::super::workflow::{
    LoadedStepSkill, LoadedStepSkillPackage, StepSkillLoadOptions, load_step_skill, step_skill,
    turn_skill_target,
};
use super::step_handlers::{
    StepRunRequest, output_error, run_step_with_inputs, run_step_with_loaded_skill_inputs,
};
use super::workflow_engine::AnsweringHost;
use super::{CallStop, CallWork, Runtime, StepRun};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;

/// The turn an unanswered agent act holds open, as its host works it.
pub trait Turn {
    /// The act the turn answers; its id is the turn's address.
    fn invocation(&self) -> &TurnInvocation;

    /// The agent step that holds the turn.
    fn step_id(&self) -> &str;

    /// The calls the turn offers its agent: one per allowance entry, named
    /// for MCP.
    fn tools(&self) -> Result<Vec<TurnTool>, RuntimeError>;

    /// How each call the turn made so far ended.
    fn calls(&self) -> Result<Vec<TurnCallResult>, RuntimeError>;

    /// The turn as its agent reads it, its act's context as an index.
    fn status(&self) -> Result<TurnStatus, RuntimeError> {
        Ok(TurnStatus {
            invocation: TurnView::from(self.invocation()),
            tools: self.tools()?,
            calls: self.calls()?,
        })
    }

    /// A page of what `locator` names in this turn: an entry of its act's
    /// context, an input of its act, or a call it made. A locator the turn
    /// does not hold is refused, so a read never reaches past its turn.
    fn read(
        &self,
        locator: &str,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<TurnReadPage, RuntimeError> {
        let refused = || RuntimeError::ContinuationRejected {
            reason: format!(
                "turn {} holds no {locator}: read a locator its status lists, an input of its act as input:<name>, or one of its calls as call:<index>",
                self.invocation().id
            ),
        };
        let named = TurnLocator::parse(locator).ok_or_else(refused)?;
        let envelope = &self.invocation().envelope;
        let entry = |entries: &[runx_contracts::ContextEntry], index: usize| {
            entries
                .get(index)
                .map(|entry| JsonValue::Object(entry.data.clone()))
        };
        let value = match &named {
            TurnLocator::Current(index) => entry(&envelope.current_context, *index),
            TurnLocator::Historical(index) => entry(&envelope.historical_context, *index),
            TurnLocator::Input(name) => envelope.inputs.get(name).cloned(),
            TurnLocator::Call(index) => self
                .calls()?
                .into_iter()
                .find(|call| call.index == *index)
                .map(|call| JsonValue::from_serialize(&call))
                .transpose()
                .map_err(|error| RuntimeError::effect_state("reading a call", error.to_string()))?,
        }
        .ok_or_else(refused)?;
        let text = match value {
            JsonValue::String(text) => text,
            other => serde_json::to_string_pretty(&other)
                .map_err(|source| RuntimeError::json("reading a turn's item", source))?,
        };
        Ok(TurnReadPage::of(&named, &text, offset, limit))
    }

    /// Make a new call. `host` answers what the call itself asks; a call it
    /// cannot answer is pending until it can. A `call_id` the turn already
    /// records returns that call when its target and input are the same, and
    /// is refused otherwise, so a call made again after a crash never runs
    /// twice.
    fn call(
        &mut self,
        host: &mut dyn Host,
        call_id: Option<String>,
        target: CallTarget,
        input: JsonObject,
    ) -> Result<TurnCallResult, RuntimeError> {
        match self.admit(host, call_id, target, input)? {
            Admission::Settled(result) => Ok(*result),
            Admission::Admitted(admitted) => {
                let executed = self.execute(host, *admitted)?;
                self.conclude(executed)
            }
        }
    }

    /// Admit a new call as `call` makes it, without executing it. A call that
    /// settles at admission (refused, held on the floor, made before under
    /// its `call_id`, or one that reaches nothing and runs at once) returns
    /// its result; one that reaches the world is claimed in the run's record,
    /// its index reserved, and returned to execute apart from the turn's
    /// driver.
    fn admit(
        &mut self,
        host: &mut dyn Host,
        call_id: Option<String>,
        target: CallTarget,
        input: JsonObject,
    ) -> Result<Admission, RuntimeError>;

    /// Execute an admitted call, recording nothing: what it came to is
    /// recorded by `conclude`.
    fn execute(
        &self,
        host: &mut dyn Host,
        admitted: AdmittedCall,
    ) -> Result<ExecutedCall, RuntimeError>;

    /// Record what an executed call came to, and return it as its agent reads
    /// it.
    fn conclude(&mut self, executed: ExecutedCall) -> Result<TurnCallResult, RuntimeError>;

    /// Take the pending call `index` further under `host`, which now brings
    /// something for what it waits on.
    fn settle(&mut self, host: &mut dyn Host, index: u32) -> Result<TurnCallResult, RuntimeError>;
}

/// Make the call the turn offers as `tool`.
pub(crate) fn call_tool(
    turn: &mut dyn Turn,
    host: &mut dyn Host,
    tool: &str,
    call_id: Option<String>,
    input: JsonObject,
) -> Result<TurnCallResult, RuntimeError> {
    let target = offered_target(turn, tool)?;
    turn.call(host, call_id, target, input)
}

/// Admit the call the turn offers as `tool`.
pub(crate) fn admit_tool(
    turn: &mut dyn Turn,
    host: &mut dyn Host,
    tool: &str,
    call_id: Option<String>,
    input: JsonObject,
) -> Result<Admission, RuntimeError> {
    let target = offered_target(turn, tool)?;
    turn.admit(host, call_id, target, input)
}

/// What the tool a turn offers as `tool` names.
fn offered_target(turn: &dyn Turn, tool: &str) -> Result<CallTarget, RuntimeError> {
    let tools = turn.tools()?;
    let names = tools
        .iter()
        .map(|offered| offered.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    tools
        .into_iter()
        .find(|offered| offered.name.as_str() == tool)
        .map(|offered| offered.target)
        .ok_or_else(|| RuntimeError::ContinuationRejected {
            reason: format!(
                "turn {} offers no tool {tool}; it offers: {names}",
                turn.invocation().id.as_str()
            ),
        })
}

/// The controls a host presents beside a turn's own tools; a turn's tools
/// never take these names.
pub(crate) const TURN_STATUS_TOOL: &str = "turn_status";
pub(crate) const TURN_READ_TOOL: &str = "turn_read";
pub(crate) const TURN_COMPLETE_TOOL: &str = "turn_complete";

const OPERATION_CALL_FIELDS: &[&str] = &["connector", "input", "idempotency_key", "amount", "act"];

/// The longest call id a host may pin.
const CALL_ID_BYTES: usize = 200;

/// Where a step's calls run: the step, its attempt, what it may call, and
/// its runtime.
pub(super) struct TurnScope<'a, A> {
    pub(super) runtime: &'a Runtime<A>,
    pub(super) workflow_dir: &'a Path,
    pub(super) workflow_name: &'a str,
    pub(super) step_id: &'a str,
    pub(super) attempt: u32,
    /// What the step may call: its `calls`.
    pub(super) allowance: &'a TurnAllowance,
    /// The requirements of the connectors the allowance's operations name.
    pub(super) connectors: &'a BTreeMap<String, ConnectorRequirement>,
    /// The step's own `effects` and `reach`, which each call it makes runs
    /// under.
    pub(super) effects: Option<runx_contracts::Effects>,
    pub(super) reach: Option<runx_contracts::Reach>,
    /// Whether the step's host says its model runs on this machine, so the
    /// turn may be given a `local_only` result.
    pub(super) reads_local_only: bool,
}

impl<A> Clone for TurnScope<'_, A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A> Copy for TurnScope<'_, A> {}

impl<A> TurnScope<'_, A>
where
    A: SkillAdapter,
{
    /// Execute an admitted call through `host`, which is the driver's or a
    /// host detached for this call alone.
    pub(super) fn execute_call(
        &self,
        host: &mut dyn Host,
        admitted: AdmittedCall,
    ) -> Result<ExecutedCall, RuntimeError> {
        self.execute(&mut AnsweringHost::new(host, None), admitted)
    }

    /// Execute an admitted call through `host`; what it consumed rides with
    /// what it came to. It reads nothing of its step's record, so it runs
    /// apart from the driver as well as under it.
    pub(super) fn execute(
        &self,
        host: &mut AnsweringHost<'_>,
        admitted: AdmittedCall,
    ) -> Result<ExecutedCall, RuntimeError> {
        let AdmittedCall {
            made,
            call,
            claim,
            admitted_by,
            held_by_effects,
            stop,
        } = admitted;
        // A call its host cancelled before it started never runs, nor one
        // in a child run its caller's host cancelled. A read runs under its
        // own stop and its callers', so a cancel ends it, and however it
        // ended it settles cancelled. A skill call's child run runs under
        // the call, and heeds its cancel. An effect runs to its end.
        let ran = if !stop.start() || crate::interrupt::caller_cancelled() {
            Ran::Cancelled
        } else if let Some(working) = stop.working() {
            let ran = crate::interrupt::while_reading(Some(working), || {
                self.run_call_step(&call, &made.target, admitted_by, held_by_effects, host)
            });
            if stop.cancelled() || crate::interrupt::caller_cancelled() {
                Ran::Cancelled
            } else {
                ran?
            }
        } else if let Some(caller) = stop.caller() {
            crate::interrupt::while_called(caller, || {
                self.run_call_step(&call, &made.target, admitted_by, held_by_effects, host)
            })?
        } else {
            crate::interrupt::note_effect();
            self.run_call_step(&call, &made.target, admitted_by, held_by_effects, host)?
        };
        Ok(ExecutedCall {
            made,
            claim,
            ran,
            answers: std::mem::take(&mut host.answers),
            stop,
        })
    }

    /// Run a call as its step; a skill call's act becomes its child run's.
    /// What admitted it on the floor, and the artifacts its inputs were given
    /// by, ride in its receipt; where those inputs came from rides in its
    /// provenance.
    fn run_call_step(
        &self,
        call: &CallStep,
        target: &CallTarget,
        admitted_by: Vec<Reference>,
        held_by_effects: bool,
        host: &mut dyn Host,
    ) -> Result<Ran, RuntimeError> {
        let scope = self;
        let step = &call.step;
        let mut admission_refs = admitted_by;
        admission_refs.extend(call.fed.iter().map(|fed| fed.artifact.clone()));
        let request = StepRunRequest {
            runtime: scope.runtime,
            workflow_dir: scope.workflow_dir,
            workflow_name: scope.workflow_name,
            step,
            attempt: 1,
            inputs: step.inputs.clone(),
            child_act: call.act.clone(),
            provenance: call.fed.iter().filter_map(|fed| fed.edge.clone()).collect(),
            admission_refs,
            held_by_effects,
            host,
            // A turn's call is a tool, operation or skill, never an agent
            // step, so it reads no earlier steps' context entries, and an ask
            // it makes states its own reason.
            context_entries: Vec::new(),
            ask_reason: None,
        };
        let result = match target {
            CallTarget::Skill {
                bound: Some(bound), ..
            } => self
                .bound_skill(step, bound)
                .and_then(|skill| run_step_with_loaded_skill_inputs(request, skill)),
            CallTarget::Skill { .. } => load_step_skill(
                scope.workflow_dir,
                step,
                StepSkillLoadOptions::of_run(&scope.runtime.options),
            )
            .and_then(|skill| run_step_with_loaded_skill_inputs(request, skill)),
            _ => run_step_with_inputs(request),
        };
        Ok(match result {
            Ok(run) => Ran::Settled(CallOutcome::Completed(Box::new(run))),
            Err(RuntimeError::ResolutionPending { request, .. }) => Ran::Waiting(request),
            Err(
                RuntimeError::AuthorityDenied { reason, .. }
                | RuntimeError::CostLimitExceeded { reason, .. },
            ) => Ran::Settled(CallOutcome::Refused(reason)),
            // The act may have reached its provider: its custody holds it, and
            // the call stays claimed until a retry with the same input
            // reconciles it.
            Err(RuntimeError::ProviderEffectUnknown { .. })
            | Err(RuntimeError::ProviderReadbackPending { .. }) => Ran::Unconfirmed,
            Err(error) if error.is_fatal_step_fault() => return Err(error),
            Err(error) => Ran::Settled(CallOutcome::Failed(error.to_string())),
        })
    }

    /// The skill a loaded method's call runs: the package its load bound,
    /// read from the project again and refused if it changed since, with the
    /// packages its closure binds for its child run to execute.
    fn bound_skill(
        &self,
        step: &WorkflowStep,
        bound: &BoundClosure,
    ) -> Result<LoadedStepSkill, RuntimeError> {
        let options = &self.runtime.options;
        let target = step.skill().ok_or_else(|| RuntimeError::StepMissingSkill {
            step_id: step.id.clone(),
        })?;
        let method = target
            .method
            .as_deref()
            .ok_or_else(|| RuntimeError::StepFailed {
                step_id: step.id.clone(),
                reason: "a loaded skill's call names its method".to_owned(),
            })?;
        let changed = |reason: String| RuntimeError::AuthorityDenied {
            verb: runx_contracts::AuthorityVerb::Execute,
            step_id: step.id.clone(),
            reason: format!(
                "skill {} is not what the turn loaded: {reason}",
                target.skill
            ),
        };
        let paths = crate::paths::RunxPaths::resolve(&options.env, self.workflow_dir);
        let directory = super::step_handlers::resolve_loaded(&target.skill, &paths, &options.env)
            .map_err(changed)?
            .runnable_path;
        let package = std::sync::Arc::new(
            crate::load_validated_skill_package(&directory)
                .map_err(|error| changed(error.to_string()))?,
        );
        let binding = crate::skill_package::verify_loaded_execution_binding(
            std::sync::Arc::clone(&package),
            method,
            &options.env,
            Some(bound.package_digest.as_str()),
            Some(bound.closure_digest.as_str()),
        )
        .map_err(|error| changed(error.to_string()))?;
        let mut skill = step_skill(
            step,
            LoadedStepSkillPackage {
                package,
                registry: None,
            },
            &options.env,
        )?;
        skill.admitted = Some(binding.packages);
        Ok(skill)
    }
}

/// The calls one step makes, as its run records them: admitted, executed and
/// recorded the same way whichever drives them, an agent's turn or a script.
pub(super) struct CallLedger<'a, A> {
    scope: TurnScope<'a, A>,
    /// The act of the turn that makes the calls, as the run records it; a
    /// script's calls belong to no turn.
    turn: Option<PendingRequest>,
    /// What the run records of the step, read when first needed.
    record: OnceCell<StepRecord>,
    /// The place the step's next call event takes, once known.
    events: Cell<Option<u64>>,
}

/// What a run records of one step: its calls, in order, and its script's
/// clock reads, by position.
struct StepRecord {
    calls: Vec<RecordedCall>,
    clock: BTreeMap<u32, i64>,
}

/// The open turn of one agent step: the act it answers, and its calls.
pub(super) struct OpenTurn<'a, A> {
    ledger: CallLedger<'a, A>,
    invocation: &'a TurnInvocation,
}

/// A call as its host made it: its place in the turn, the id its host
/// pinned on it, what it names and its input.
#[derive(Clone)]
struct Made {
    index: u32,
    call_id: Option<String>,
    target: CallTarget,
    input: JsonObject,
    /// The standing approval that admitted it on the floor, if one did.
    standing: Option<runx_contracts::StandingApproval>,
    /// Its host cancelled it, which every later record of it keeps.
    cancelled: bool,
}

impl Made {
    fn of(call: &RecordedCall) -> Self {
        Self {
            index: call.index,
            call_id: call.call_id.clone(),
            target: call.target.clone(),
            input: call.input.clone(),
            standing: call.standing.clone(),
            cancelled: call.cancelled,
        }
    }
}

/// The step a call runs as, with what rides beside its inputs.
struct CallStep {
    step: WorkflowStep,
    /// A skill call's act, which becomes its child run's.
    act: Option<CallAct>,
    /// The inputs the call gave by reference.
    fed: Vec<Fed>,
}

/// An input given by reference to a packet the project holds: the artifact
/// the call's receipt names for it, and, when a call of the turn returned
/// it, where it came from.
struct Fed {
    edge: Option<ProvenanceEntry>,
    artifact: Reference,
}

/// What admitting a call came to.
pub enum Admission {
    /// It settled at admission.
    Settled(Box<TurnCallResult>),
    /// It is claimed, and executes apart from the turn's driver.
    Admitted(Box<AdmittedCall>),
}

/// A call claimed in its run's record, its index reserved, to execute.
pub struct AdmittedCall {
    made: Made,
    call: CallStep,
    claim: CallOutcome,
    /// What admitted it on the floor, for its receipt.
    admitted_by: Vec<Reference>,
    /// The floor holds its write under `effects: ask`.
    held_by_effects: bool,
    /// Its place among the run's executing calls, until it's recorded.
    stop: CallStop,
}

impl AdmittedCall {
    /// The call's place in its turn.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.made.index
    }

    /// Dispatch the call to execute: from here it's in flight, unless its
    /// host cancelled it first.
    pub(super) fn dispatch(&self) -> bool {
        self.stop.start()
    }
}

/// What an admitted call's execution came to, to record.
pub struct ExecutedCall {
    made: Made,
    claim: CallOutcome,
    ran: Ran,
    /// The answers its execution consumed.
    answers: Vec<Answer>,
    stop: CallStop,
}

/// The most a field of a script call's result holds inline, in bytes of
/// its canonical JSON (`runx.stable-json.v1`): a field past it reaches the
/// script as a reference to the artifact that holds it.
pub(super) const SCRIPT_INLINE_BYTES: u64 = 64 * 1024;

/// Whether `value` is past `bound`, when there is one.
pub(super) fn past_inline_bound(
    value: &JsonValue,
    bound: Option<u64>,
) -> Result<bool, RuntimeError> {
    let Some(bound) = bound else {
        return Ok(false);
    };
    let bytes = runx_contracts::to_canonical_bytes(value).map_err(|error| {
        RuntimeError::effect_state("measuring a call's result", error.to_string())
    })?;
    Ok(u64::try_from(bytes.len()).unwrap_or(u64::MAX) > bound)
}

/// A script's wait at position `index`, as its ledger records it.
fn waiting(index: u32, until: i64) -> Made {
    script_call(index, CallTarget::Wait { until })
}

/// A call of runx's own a script made at position `index`, naming `target`.
fn script_call(index: u32, target: CallTarget) -> Made {
    Made {
        index,
        call_id: None,
        target,
        input: JsonObject::new(),
        standing: None,
        cancelled: false,
    }
}

/// How a call's step went.
enum Ran {
    Settled(CallOutcome),
    /// The step waits on `request`: its own, or one its child run raised.
    Waiting(Box<ResolutionRequest>),
    /// The step's effect may have happened and is not confirmed.
    Unconfirmed,
    /// Its host cancelled it before it started, or while it only read.
    Cancelled,
}

impl<'a, A> OpenTurn<'a, A>
where
    A: SkillAdapter,
{
    /// The turn `request` opens, with the calls the run records for it.
    pub(super) fn open(
        scope: TurnScope<'a, A>,
        request: &'a ResolutionRequest,
    ) -> Result<Self, RuntimeError> {
        let ResolutionRequest::Turn { invocation, .. } = request else {
            return Err(RuntimeError::EngineInvariant {
                context: "opening an agent turn",
                message: "an agent step asked for something other than its act".to_owned(),
            });
        };
        Ok(Self {
            ledger: CallLedger {
                scope,
                turn: Some(pending_request(request)?),
                record: OnceCell::new(),
                events: Cell::new(None),
            },
            invocation,
        })
    }

    /// The turn's calls.
    pub(super) fn ledger(&self) -> &CallLedger<'a, A> {
        &self.ledger
    }
}

impl<'a, A> CallLedger<'a, A>
where
    A: SkillAdapter,
{
    /// The calls of a step no turn drives: a script's.
    pub(super) fn of_step(scope: TurnScope<'a, A>) -> Self {
        Self {
            scope,
            turn: None,
            record: OnceCell::new(),
            events: Cell::new(None),
        }
    }

    /// Take call `index` further under `host`, which now brings something for
    /// what it waits on. A settled call returns how it settled.
    pub(super) fn further(
        &mut self,
        host: &mut dyn Host,
        index: u32,
    ) -> Result<TurnCallResult, RuntimeError> {
        let Some(call) = self
            .recorded()?
            .iter()
            .find(|call| call.index == index)
            .cloned()
        else {
            return Err(RuntimeError::ContinuationRejected {
                reason: format!("step {} has no call {index}", self.scope.step_id),
            });
        };
        match call.outcome.clone() {
            CallOutcome::Child(_) => self.enter(host, call),
            // The call asks again what it waits on, unchanged.
            CallOutcome::Pending(waiting) => {
                self.run(AnsweringHost::new(host, Some(&waiting)), Made::of(&call))
            }
            // A claimed call runs again only on the host's exact
            // acknowledgement, which its new claim records; with no answer
            // yet it still waits on its retry.
            CallOutcome::Claimed(retry) => {
                let mut answering = AnsweringHost::new(host, Some(&retry));
                match answering.resolve(resolution_request(&retry)?)? {
                    None => self.result(&Made::of(&call), &call.outcome, &call.outputs),
                    Some(response) if response.payload == retry_acknowledgement() => {
                        self.run(answering, Made::of(&call))
                    }
                    Some(_) => Err(RuntimeError::ContinuationRejected {
                        reason: format!(
                            "call {index} runs again only on the acknowledgement {{\"retry\": true}}"
                        ),
                    }),
                }
            }
            settled => self.result(&Made::of(&call), &settled, &call.outputs),
        }
    }

    /// The step's calls as the run records them, in order.
    pub(super) fn recorded(&self) -> Result<&[RecordedCall], RuntimeError> {
        Ok(&self.step_record()?.calls)
    }

    /// What the step's script read of the clock, by position.
    pub(super) fn clock(&self) -> Result<&BTreeMap<u32, i64>, RuntimeError> {
        Ok(&self.step_record()?.clock)
    }

    fn step_record(&self) -> Result<&StepRecord, RuntimeError> {
        if let Some(record) = self.record.get() {
            return Ok(record);
        }
        let (calls, clock) = self
            .scope
            .runtime
            .options
            .run
            .step_calls(self.scope.step_id, self.scope.attempt)?;
        Ok(self.record.get_or_init(|| StepRecord { calls, clock }))
    }

    /// Record the script's wait at position `index` until `until`: a request
    /// the run waits on, runx's own, until a drive at or after `until`
    /// settles it. The request's id is the call's address.
    pub(super) fn wait(
        &mut self,
        index: u32,
        until: i64,
    ) -> Result<ResolutionRequest, RuntimeError> {
        let request = ResolutionRequest::Wait {
            id: self.call_address(index).into(),
            until,
        };
        let pending = CallOutcome::Pending(pending_request(&request)?);
        self.record(&waiting(index, until), pending, &[])?;
        Ok(request)
    }

    /// Settle the script's wait at position `index` at `at`, in
    /// milliseconds since the epoch: what the wait returns.
    pub(super) fn settle_wait(
        &mut self,
        index: u32,
        until: i64,
        at: i64,
    ) -> Result<(), RuntimeError> {
        self.record(&waiting(index, until), CallOutcome::Waited(at), &[])
            .map(drop)
    }

    /// Record the script's read at position `index` of the reference whose
    /// artifact's digest is `digest`: settled with its content when a call of
    /// the step returned it and it fits the worker's input bound, refused as
    /// too large past the bound, and refused when no call of the step
    /// returned it. Its outcome is the read's settlement.
    pub(super) fn read(&mut self, index: u32, digest: &str) -> Result<(), RuntimeError> {
        let held = self
            .recorded()?
            .iter()
            .flat_map(|call| &call.outputs)
            .find(|part| part.artifact.digest.as_str() == digest)
            .map(|part| part.artifact.clone());
        let bound =
            u64::try_from(runx_contracts::javascript_worker::MAX_INPUT_BYTES).unwrap_or(u64::MAX);
        let outcome = match held {
            None => CallOutcome::Refused(format!(
                "{digest} names no artifact a call of step {} returned",
                self.scope.step_id
            )),
            Some(artifact) if artifact.size_bytes > bound => CallOutcome::TooLarge {
                size_bytes: artifact.size_bytes,
                bound,
            },
            Some(artifact) => CallOutcome::Read(Box::new(artifact)),
        };
        let target = CallTarget::Read {
            digest: digest.into(),
        };
        self.record(&script_call(index, target), outcome, &[])
            .map(drop)
    }

    /// The artifact a reference a call of the step returned names, by
    /// `digest`, and that artifact's content; none for a digest no call of
    /// the step returned.
    pub(super) fn returned(
        &self,
        digest: &str,
    ) -> Result<Option<(Artifact, JsonValue)>, RuntimeError> {
        let Some(artifact) = self
            .recorded()?
            .iter()
            .flat_map(|call| &call.outputs)
            .find(|part| part.artifact.digest.as_str() == digest)
            .map(|part| part.artifact.clone())
        else {
            return Ok(None);
        };
        let content = self.content(&artifact)?;
        Ok(Some((artifact, content)))
    }

    /// The content of `artifact`, a call's output this run holds.
    pub(super) fn content(&self, artifact: &Artifact) -> Result<JsonValue, RuntimeError> {
        let options = &self.scope.runtime.options;
        let artifacts =
            crate::paths::RunxPaths::resolve(&options.env, self.scope.workflow_dir).artifacts();
        crate::services::read_json_artifact(&artifacts, artifact)
            .map_err(|message| RuntimeError::effect_state("reading a call's output", message))
    }

    /// Record what the step's script read of the clock at position `index`.
    pub(super) fn record_clock(&mut self, index: u32, millis: i64) -> Result<(), RuntimeError> {
        self.step_record()?;
        self.scope.runtime.options.run.record_clock(
            self.scope.step_id,
            self.scope.attempt,
            index,
            millis,
        )?;
        if let Some(record) = self.record.get_mut() {
            record.clock.insert(index, millis);
        }
        Ok(())
    }

    /// The place the step's next call event takes among its call events.
    fn next_order(&self) -> Result<u64, RuntimeError> {
        if let Some(next) = self.events.get() {
            return Ok(next);
        }
        let next = self
            .recorded()?
            .iter()
            .map(|call| call.order + 1)
            .max()
            .unwrap_or(0);
        self.events.set(Some(next));
        Ok(next)
    }

    fn recorded_mut(&mut self) -> Result<&mut Vec<RecordedCall>, RuntimeError> {
        self.recorded()?;
        self.record
            .get_mut()
            .map(|record| &mut record.calls)
            .ok_or_else(|| RuntimeError::EngineInvariant {
                context: "recording a turn call",
                message: "the turn's calls were not read".to_owned(),
            })
    }

    /// The runs of the turn's completed calls, in order.
    pub(super) fn completed(&self) -> Result<Vec<&StepRun>, RuntimeError> {
        Ok(self
            .recorded()?
            .iter()
            .filter_map(|call| match &call.outcome {
                CallOutcome::Completed(run) => Some(run.as_ref()),
                _ => None,
            })
            .collect())
    }

    /// The first call that has not settled.
    pub(super) fn unsettled(&self) -> Result<Option<u32>, RuntimeError> {
        Ok(self
            .recorded()?
            .iter()
            .find_map(|call| match &call.outcome {
                CallOutcome::Pending(_) | CallOutcome::Claimed(_) | CallOutcome::Child(_) => {
                    Some(call.index)
                }
                CallOutcome::Completed(_)
                | CallOutcome::Refused(_)
                | CallOutcome::Failed(_)
                | CallOutcome::Cancelled
                | CallOutcome::Waited(_)
                | CallOutcome::Read(_)
                | CallOutcome::TooLarge { .. } => None,
            }))
    }

    /// Why the turn's allowance refuses `target`, if it does.
    fn refusal(&self, target: &CallTarget) -> Result<Option<String>, RuntimeError> {
        let allowance = &self.scope.allowance;
        let allowed =
            match target {
                CallTarget::Tool { tool } => allowance.tools.contains(tool),
                CallTarget::Operation {
                    requirement,
                    operation,
                } => {
                    allowance.operations.iter().any(|item| {
                        &item.requirement == requirement && &item.operation == operation
                    }) || self.loaded_offers(target)?
                }
                CallTarget::Skill {
                    skill,
                    method: None,
                    bound: None,
                } => allowance.named_skills().any(|listed| listed == skill),
                CallTarget::Skill { .. } => self.loaded_offers(target)?,
                CallTarget::Ask => allowance.asks,
                CallTarget::Load => allowance.rules().next().is_some(),
                // A script's wait and read are its ledger's own, never
                // admitted.
                CallTarget::Wait { .. } | CallTarget::Read { .. } => false,
            };
        if !allowed {
            return Ok(Some(format!(
                "{} is not in the turn's allowance",
                label(target)
            )));
        }
        if matches!(target, CallTarget::Skill { .. }) {
            return self.child_refusal();
        }
        Ok(None)
    }

    /// Why the run's delegation refuses the child run a skill call would
    /// start: one level too deep, or one more than the turn may start.
    fn child_refusal(&self) -> Result<Option<String>, RuntimeError> {
        let options = &self.scope.runtime.options;
        if let Some(reason) = super::child_depth_refusal(options) {
            return Ok(Some(reason));
        }
        let Some(limit) = options
            .run_authority
            .as_ref()
            .and_then(|authority| authority.max_fanout)
        else {
            return Ok(None);
        };
        let mut started = 0_usize;
        for call in self.recorded()? {
            if !matches!(call.target, CallTarget::Skill { .. }) {
                continue;
            }
            let child = options
                .run
                .child(&call_step_id(self.scope.step_id, call.index))?;
            if child.load()?.is_some() {
                started += 1;
            }
        }
        Ok((started >= usize::try_from(limit).unwrap_or(usize::MAX))
            .then(|| format!("the run's delegation lets a turn start at most {limit} child runs")))
    }

    /// Run the call `made` through `host`, which carries what the call
    /// already consumed: admitted, executed and recorded at once.
    fn run(
        &mut self,
        mut host: AnsweringHost<'_>,
        made: Made,
    ) -> Result<TurnCallResult, RuntimeError> {
        match self.admit_at(&mut host, made)? {
            Admission::Settled(result) => Ok(*result),
            Admission::Admitted(admitted) => {
                let executed = self.execute_admitted(&mut host, *admitted)?;
                self.conclude(executed)
            }
        }
    }

    /// Admit call `index` under the driver, without executing it: it settles
    /// at admission, or is claimed and comes back to execute, under the
    /// driver or apart from it.
    pub(super) fn admit(
        &mut self,
        host: &mut dyn Host,
        index: u32,
        target: CallTarget,
        input: JsonObject,
    ) -> Result<Admission, RuntimeError> {
        self.admit_at(
            &mut AnsweringHost::new(host, None),
            Made {
                index,
                call_id: None,
                target,
                input,
                standing: None,
                cancelled: false,
            },
        )
    }

    fn admit_at(
        &mut self,
        host: &mut AnsweringHost<'_>,
        made: Made,
    ) -> Result<Admission, RuntimeError> {
        #[cfg(feature = "crash-points")]
        let call = self.call_address(made.index);
        #[cfg(feature = "crash-points")]
        crate::crash::at("before_admission", &call);
        let admission = self.admit_made(host, made)?;
        #[cfg(feature = "crash-points")]
        if matches!(admission, Admission::Admitted(_)) {
            crate::crash::at("after_admission", &call);
        }
        Ok(admission)
    }

    /// Record what an executed call came to, under the driver.
    pub(super) fn conclude(
        &mut self,
        executed: ExecutedCall,
    ) -> Result<TurnCallResult, RuntimeError> {
        // For a skill call, its child run has sealed here and the call isn't
        // recorded yet.
        #[cfg(feature = "crash-points")]
        crate::crash::at("after_execution", &self.call_address(executed.made.index));
        self.conclude_executed(executed)
    }

    /// Where the step's calls run, to execute one apart from the driver.
    pub(super) fn scope(&self) -> TurnScope<'a, A> {
        self.scope
    }

    /// Call `index`'s address in its run: `<step>/<index>`.
    fn call_address(&self, index: u32) -> String {
        self.scope
            .runtime
            .options
            .run
            .address(&call_step_id(self.scope.step_id, index))
    }

    /// Admit the call `made`. A call outside the allowance, or malformed, is
    /// refused before it runs, and the floor decides before the claim: a
    /// call waiting on the person is pending on its gate, never "may have
    /// run", and one they declined is refused. A call that performs an effect
    /// is claimed, and a skill call links the child run it starts: either
    /// enters the run's record, with what authorized it, under the version
    /// this driver last read, so a second driver of the turn is refused
    /// before anything runs. A call that reaches nothing runs at once.
    fn admit_made(
        &mut self,
        host: &mut AnsweringHost<'_>,
        made: Made,
    ) -> Result<Admission, RuntimeError> {
        self.admits(made.index)?;
        let settled = |turn: &mut Self, made, outcome, answers: &[Answer]| {
            turn.settled(made, outcome, answers)
                .map(|result| Admission::Settled(Box::new(result)))
        };
        let call = match self.refusal(&made.target)?.map_or_else(
            || self.call_step(made.index, &made.target, &made.input),
            Err,
        ) {
            Ok(call) => call,
            Err(reason) => {
                let answers = host.answers.clone();
                return settled(self, made, CallOutcome::Refused(reason), &answers);
            }
        };
        let admitted = match super::floor::hold(
            self.scope.runtime,
            self.scope.workflow_dir,
            &call.step,
            &call.step.inputs,
            host,
        ) {
            Ok(admitted) => admitted,
            Err(RuntimeError::ResolutionPending { request, .. }) => {
                let pending = CallOutcome::Pending(pending_request(&request)?);
                let answers = host.answers.clone();
                return settled(self, made, pending, &answers);
            }
            Err(RuntimeError::AuthorityDenied { reason, .. }) => {
                let answers = host.answers.clone();
                return settled(self, made, CallOutcome::Refused(reason), &answers);
            }
            Err(error) if error.is_fatal_step_fault() => return Err(error),
            // A call whose effect cannot even be described could not run.
            Err(error) => {
                let answers = host.answers.clone();
                return settled(self, made, CallOutcome::Failed(error.to_string()), &answers);
            }
        };
        let made = Made {
            standing: admitted.standing().cloned(),
            ..made
        };
        let admitted_by = admitted.reference()?.into_iter().collect::<Vec<_>>();
        let held_by_effects = admitted == super::floor::Admitted::Held;
        let claim = match &made.target {
            _ if super::reaches_world(&call.step.kind) => {
                CallOutcome::Claimed(self.retry(made.index, &call.step)?)
            }
            CallTarget::Skill { .. } => {
                CallOutcome::Child(self.scope.runtime.options.run.child_id(&call.step.id))
            }
            CallTarget::Wait { .. } | CallTarget::Read { .. } => {
                return Err(RuntimeError::EngineInvariant {
                    context: "admitting a call",
                    message:
                        "a wait or a read is its script's ledger's to record, never a call to run"
                            .to_owned(),
                });
            }
            // A call that reaches nothing holds no claim: it runs now, under
            // the driver.
            CallTarget::Tool { .. }
            | CallTarget::Operation { .. }
            | CallTarget::Ask
            | CallTarget::Load => {
                let ran = self.scope.run_call_step(
                    &call,
                    &made.target,
                    admitted_by,
                    held_by_effects,
                    host,
                )?;
                return match ran {
                    Ran::Settled(outcome) => {
                        let answers = host.answers.clone();
                        settled(self, made, outcome, &answers)
                    }
                    Ran::Waiting(request) => {
                        let pending = CallOutcome::Pending(pending_request(&request)?);
                        let answers = host.answers.clone();
                        settled(self, made, pending, &answers)
                    }
                    Ran::Unconfirmed | Ran::Cancelled => Err(RuntimeError::EngineInvariant {
                        context: "running a turn call",
                        message: "a call that ran at once, unclaimed, has no effect to leave unconfirmed and was never its host's to cancel".to_owned(),
                    }),
                };
            }
        };
        let authorized = std::mem::take(&mut host.answers);
        self.record(&made, claim.clone(), &authorized)?;
        // From here until it's recorded, its host can cancel it.
        let stop = self.stop(made.index, &call.step);
        Ok(Admission::Admitted(Box::new(AdmittedCall {
            made,
            call,
            claim,
            admitted_by,
            held_by_effects,
            stop,
        })))
    }

    /// Execute an admitted call through `host`, under the driver.
    fn execute_admitted(
        &self,
        host: &mut AnsweringHost<'_>,
        admitted: AdmittedCall,
    ) -> Result<ExecutedCall, RuntimeError> {
        self.scope.execute(host, admitted)
    }

    /// Record what an executed call came to. A claimed call whose effect is
    /// unconfirmed, or whose driver stopped, waits on its retry; a skill call
    /// waits on its child, which holds what it waits on.
    fn conclude_executed(
        &mut self,
        executed: ExecutedCall,
    ) -> Result<TurnCallResult, RuntimeError> {
        let ExecutedCall {
            made,
            claim,
            ran,
            answers,
            stop,
        } = executed;
        // Released under the driver, so a cancel is recorded with the call
        // or names no executing call. A call in a child run its caller's
        // host cancelled is cancelled too.
        let changed_nothing = stop.changed_nothing();
        let cancelled = stop.release() || crate::interrupt::caller_cancelled();
        let ran = if cancelled && changed_nothing {
            Ran::Cancelled
        } else {
            ran
        };
        self.concluded(made, claim, ran, &answers, cancelled)
    }

    /// Record what call `made`, claimed as `claim`, came to. A cancel its
    /// host made while it ran is recorded with it, even where it still waits.
    fn concluded(
        &mut self,
        made: Made,
        claim: CallOutcome,
        ran: Ran,
        answers: &[Answer],
        cancelled: bool,
    ) -> Result<TurnCallResult, RuntimeError> {
        let newly = cancelled && !made.cancelled;
        let made = Made {
            cancelled: made.cancelled || cancelled,
            ..made
        };
        match (ran, claim) {
            (Ran::Cancelled, _) => self.settled(made, CallOutcome::Cancelled, answers),
            (Ran::Settled(outcome), _) => self.settled(made, outcome, answers),
            (Ran::Waiting(_) | Ran::Unconfirmed, claim @ CallOutcome::Child(_)) => {
                self.still(made, claim, newly)
            }
            (Ran::Waiting(request), _) => {
                let pending = CallOutcome::Pending(pending_request(&request)?);
                self.settled(made, pending, answers)
            }
            (Ran::Unconfirmed, claim) => self.still(made, claim, newly),
        }
    }

    /// A call that still stands as `claim`: recorded again only to keep a
    /// cancel its host just made.
    fn still(
        &mut self,
        made: Made,
        claim: CallOutcome,
        newly: bool,
    ) -> Result<TurnCallResult, RuntimeError> {
        if newly {
            return self.settled(made, claim, &[]);
        }
        self.result(&made, &claim, &[])
    }

    /// Register call `index`, which runs as `step`, among the run's
    /// executing calls, so its host can cancel it until it's recorded.
    fn stop(&self, index: u32, step: &WorkflowStep) -> CallStop {
        let options = &self.scope.runtime.options;
        let work = if super::only_reads(step, self.scope.workflow_dir, &options.env) {
            CallWork::Reads
        } else if matches!(step.kind, StepKind::Skill(_)) {
            CallWork::RunsAChild
        } else {
            CallWork::Changes
        };
        options.call_stops.register(self.call_address(index), work)
    }

    /// Take a skill call further: run its child again, from the child's own
    /// record. The parent records the call again only once the child settles.
    /// A pass that only records what a call did enters its child even after
    /// a stop, since the call it records may be the child's.
    fn enter(
        &mut self,
        host: &mut dyn Host,
        call: RecordedCall,
    ) -> Result<TurnCallResult, RuntimeError> {
        let made = Made::of(&call);
        if !host.records() {
            self.admits(made.index)?;
        }
        let step = self
            .call_step(made.index, &made.target, &made.input)
            .map_err(|reason| RuntimeError::EngineInvariant {
                context: "entering a skill call",
                message: reason,
            })?;
        let mut answering = AnsweringHost::new(host, None);
        // Its child runs further under the driver, and under the call, so
        // its host can cancel it meanwhile.
        let stop = self.stop(made.index, &step.step);
        stop.start();
        let mut run = || {
            self.scope
                .run_call_step(&step, &made.target, Vec::new(), false, &mut answering)
        };
        let ran = match stop.caller() {
            Some(caller) => crate::interrupt::while_called(caller, run)?,
            None => run()?,
        };
        let changed_nothing = stop.changed_nothing();
        let cancelled = stop.release() || crate::interrupt::caller_cancelled();
        let ran = if cancelled && changed_nothing {
            Ran::Cancelled
        } else {
            ran
        };
        let answers = answering.answers;
        self.concluded(made, call.outcome, ran, &answers, cancelled)
    }

    /// A driver asked to stop admits no further call; the turn stays where
    /// it is.
    fn admits(&self, index: u32) -> Result<(), RuntimeError> {
        if crate::interrupt::stop_requested() {
            return Err(RuntimeError::Stopped {
                run_id: self.scope.runtime.options.run.run_id().to_owned(),
                step_id: format!("call {index} of {}", self.scope.step_id),
            });
        }
        Ok(())
    }

    /// What a claimed call waits on until it settles: the person's
    /// acknowledgement to run it again with the same input, when it can be
    /// sent again.
    fn retry(&self, index: u32, step: &WorkflowStep) -> Result<PendingRequest, RuntimeError> {
        pending_request(&super::unsettled_retry(
            self.retry_id(index),
            &format!("call {index}"),
            super::resend_of(
                step,
                self.scope.workflow_dir,
                &self.scope.runtime.options.env,
            ),
        ))
    }

    /// The address of call `index`'s retry: `<step>/<index>/retry` in this
    /// run.
    fn retry_id(&self, index: u32) -> String {
        self.scope.runtime.options.run.address(&format!(
            "{}/retry",
            call_step_id(self.scope.step_id, index)
        ))
    }

    /// Record how a call ended, and return it as its agent reads it.
    fn settled(
        &mut self,
        made: Made,
        outcome: CallOutcome,
        answers: &[Answer],
    ) -> Result<TurnCallResult, RuntimeError> {
        let outcome_now = outcome.clone();
        let outputs = self.record(&made, outcome, answers)?;
        self.result(&made, &outcome_now, &outputs)
    }

    /// Record a call's latest word on its index. A completed call's output
    /// is persisted first, as project artifacts its event names.
    fn record(
        &mut self,
        made: &Made,
        outcome: CallOutcome,
        answers: &[Answer],
    ) -> Result<Vec<RunCallOutput>, RuntimeError> {
        // A script's call holds a field of its result inline only up to the
        // bound, which its record keeps, so replay gives the same references.
        let inline_bound = (self.turn.is_none() && matches!(outcome, CallOutcome::Completed(_)))
            .then_some(SCRIPT_INLINE_BYTES);
        let outputs = match &outcome {
            CallOutcome::Completed(run) => self.output_artifacts(made, run, inline_bound)?,
            _ => Vec::new(),
        };
        self.scope.runtime.options.run.record_call(&CallRecord {
            step_id: self.scope.step_id,
            attempt: self.scope.attempt,
            turn: self.turn.as_ref(),
            index: made.index,
            call_id: made.call_id.as_deref(),
            target: &made.target,
            input: &made.input,
            outcome: &outcome,
            outputs: &outputs,
            standing: made.standing.as_ref(),
            answers,
            cancelled: made.cancelled,
            inline_bound,
        })?;
        let (step_id, attempt) = (self.scope.step_id.to_owned(), self.scope.attempt);
        // Each event takes the next place among the step's call events.
        let order = self.next_order()?;
        self.events.set(Some(order + 1));
        let calls = self.recorded_mut()?;
        let call = RecordedCall {
            step_id,
            attempt,
            index: made.index,
            call_id: made.call_id.clone(),
            target: made.target.clone(),
            input: made.input.clone(),
            standing: made.standing.clone(),
            outcome,
            outputs: outputs.clone(),
            order,
            cancelled: made.cancelled,
            inline_bound,
        };
        match calls.iter_mut().find(|known| known.index == call.index) {
            Some(known) => *known = call,
            None => calls.push(call),
        }
        Ok(outputs)
    }

    /// A completed call's output as project artifacts of its run: one per
    /// packet output, holding the packet's data, and one for the rest. Each
    /// is content-addressed, so a host reads the call's outcome from the
    /// run's record alone. Each is `local_only` when any artifact the call
    /// was given or names is, or any artifact in it or named in it is.
    fn output_artifacts(
        &self,
        made: &Made,
        run: &StepRun,
        inline_bound: Option<u64>,
    ) -> Result<Vec<RunCallOutput>, RuntimeError> {
        let options = &self.scope.runtime.options;
        let artifacts =
            crate::paths::RunxPaths::resolve(&options.env, self.scope.workflow_dir).artifacts();
        let label = call_step_id(self.scope.step_id, made.index);
        let snapshots = &self.scope.runtime.local_artifacts;
        let input = JsonValue::Object(made.input.clone());
        let given = ArtifactLabels::derived_from([
            &ArtifactLabels::of_value(&input),
            &snapshots.labels_named_in(&input),
        ]);
        let persist = |label: &str, value: &JsonValue| {
            crate::services::persist_json_artifact(
                &artifacts,
                options.run.run_id(),
                "calls",
                label,
                value,
                ArtifactLabels::derived_from([
                    &given,
                    &ArtifactLabels::of_value(value),
                    &snapshots.labels_named_in(value),
                ]),
            )
            .map_err(|message| RuntimeError::effect_state("persisting a call's output", message))
        };
        let mut outputs = Vec::new();
        let mut rest = JsonObject::new();
        for (name, value) in &run.contract {
            if run.artifact_outputs.contains(name) {
                let data = run.output(name).unwrap_or(value);
                outputs.push(RunCallOutput {
                    output: NonEmptyString::new(name.clone()),
                    artifact: persist(&format!("{label}-{name}"), data)?,
                });
            } else if past_inline_bound(value, inline_bound)? {
                // A field a script takes by reference is an artifact of its own.
                outputs.push(RunCallOutput {
                    output: NonEmptyString::new(name.clone()),
                    artifact: persist(&format!("{label}-{name}"), value)?,
                });
            } else {
                rest.insert(name.clone(), value.clone());
            }
        }
        if !rest.is_empty() {
            outputs.push(RunCallOutput {
                output: None,
                artifact: persist(&label, &JsonValue::Object(rest))?,
            });
        }
        Ok(outputs)
    }

    /// Whether a call's result is withheld from the step that made it: it
    /// completed reading what's labelled `local_only`, and the step's host
    /// can't say its model runs on this machine. A `local_only` result
    /// reaches a step, a turn or a script, only where a `local_only` context
    /// entry can.
    pub(super) fn withholds(&self, outcome: &CallOutcome, outputs: &[RunCallOutput]) -> bool {
        matches!(outcome, CallOutcome::Completed(run) if run.outcome.succeeded())
            && !self.scope.reads_local_only
            && outputs
                .iter()
                .any(|part| part.artifact.labels.is_local_only())
    }

    /// How a call stands, as its agent reads it. A skill call waits on what
    /// its child run waits on. A result it [withholds](Self::withholds) is
    /// refused, and the turn never sees what it read.
    fn result(
        &self,
        made: &Made,
        outcome: &CallOutcome,
        outputs: &[RunCallOutput],
    ) -> Result<TurnCallResult, RuntimeError> {
        let outcome = match outcome {
            _ if self.withholds(outcome, outputs) => TurnCallOutcome::Refused {
                reason: reason(format!(
                    "call {} reads what's labelled local_only, and this turn's host can't say its model runs on this machine",
                    made.index
                )),
            },
            CallOutcome::Completed(run) if run.outcome.succeeded() => TurnCallOutcome::Completed {
                output: run.contract.clone(),
                receipt_id: run.receipt.id.to_string().into(),
                artifacts: outputs
                    .iter()
                    .filter_map(|part| {
                        part.output
                            .as_ref()
                            .map(|name| (name.as_str().to_owned(), part.artifact.clone()))
                    })
                    .collect(),
            },
            CallOutcome::Completed(run) => TurnCallOutcome::Failed {
                reason: reason(output_error(run)),
                receipt_id: Some(run.receipt.id.to_string().into()),
            },
            CallOutcome::Refused(why) => TurnCallOutcome::Refused {
                reason: reason(why.clone()),
            },
            CallOutcome::Failed(why) => TurnCallOutcome::Failed {
                reason: reason(why.clone()),
                receipt_id: None,
            },
            CallOutcome::Pending(request) | CallOutcome::Claimed(request) => {
                TurnCallOutcome::Pending {
                    waiting: vec![resolution_request(request)?],
                }
            }
            CallOutcome::Child(run_id) => TurnCallOutcome::Pending {
                waiting: waiting_requests(self.scope.runtime.options.run.store(), run_id)?
                    .iter()
                    .map(resolution_request)
                    .collect::<Result<_, _>>()?,
            },
            CallOutcome::Cancelled => TurnCallOutcome::Cancelled,
            CallOutcome::Waited(_) | CallOutcome::Read(_) | CallOutcome::TooLarge { .. } => {
                return Err(RuntimeError::EngineInvariant {
                    context: "reading a turn's call",
                    message: "a turn's call never waits or reads by reference".to_owned(),
                });
            }
        };
        Ok(TurnCallResult {
            index: made.index,
            target: made.target.clone(),
            input_digest: crate::execution::run_record::input_digest(&made.input)?.into(),
            call_id: made.call_id.clone().map(Into::into),
            outcome,
        })
    }

    /// The step a call runs as, a skill call's act, and the inputs it gave
    /// by reference. A malformed call is refused before it runs.
    fn call_step(
        &self,
        index: u32,
        target: &CallTarget,
        input: &JsonObject,
    ) -> Result<CallStep, String> {
        let step_id = call_step_id(self.scope.step_id, index);
        let mut inputs = input.clone();
        let mut act = None;
        let kind = match target {
            // A tool the turn lists resolves before its call runs.
            CallTarget::Tool { tool } => {
                self.resolve_tool(tool.as_str())?;
                StepKind::Tool(ToolTarget::Catalog(tool.as_str().to_owned()))
            }
            CallTarget::Operation {
                requirement,
                operation,
            } => {
                if let Some(field) = input
                    .keys()
                    .find(|field| !OPERATION_CALL_FIELDS.contains(&field.as_str()))
                {
                    return Err(format!(
                        "an operation call takes connector, input, idempotency_key, amount and act, not {field}"
                    ));
                }
                // The call's arguments are its step's `inputs`.
                if let Some(arguments) = inputs.remove("input") {
                    inputs.insert("inputs".to_owned(), arguments);
                }
                let declared = self
                    .declared(requirement.as_str())
                    .map_err(|error| error.to_string())?;
                let target = OperationStepTarget {
                    requirement: requirement.as_str().to_owned(),
                    operation: Some(
                        OperationName::new(operation.as_str().to_owned())
                            .ok_or_else(|| format!("{operation} is not an operation name"))?,
                    ),
                    from: Vec::new(),
                    loaded: self
                        .loaded_binding(requirement.as_str())
                        .map_err(|error| error.to_string())?,
                    declared,
                };
                // A turn's own call never writes to store (D19): a stream is
                // written by a package's deterministic step.
                if let Some(write) = crate::kernel::operation::operation_write(
                    &target,
                    &inputs,
                    &self.scope.runtime.options.env,
                    self.scope.workflow_dir,
                )
                .filter(|write| write.store)
                {
                    return Err(format!(
                        "{}/{} writes to store, and a turn's own call never does: writes to store go through a package's deterministic step",
                        write.connector, write.operation
                    ));
                }
                StepKind::Operation(target)
            }
            CallTarget::Skill { skill, method, .. } => {
                // `act` is never a skill input: it is why the skill is run.
                act = inputs
                    .remove("act")
                    .map(|value| CallAct::parse(&value))
                    .transpose()?;
                let target = match method {
                    Some(method) => SkillTarget {
                        skill: skill.as_str().to_owned(),
                        method: Some(method.as_str().to_owned()),
                    },
                    None => turn_skill_target(skill.as_str()),
                };
                StepKind::Skill(target)
            }
            CallTarget::Wait { .. } | CallTarget::Read { .. } => {
                return Err("a wait or a read runs as no step".to_owned());
            }
            CallTarget::Load => StepKind::Load(LoadStep {
                target: load_target(input)?,
                rules: self.scope.allowance.rules().copied().collect(),
                held: self.held().map_err(|error| error.to_string())?,
                projects: self.held_projects().map_err(|error| error.to_string())?,
            }),
            // An ask is a yes or no on its reason, or questions the person
            // answers.
            CallTarget::Ask if input.len() == 1 && input.contains_key("questions") => {
                let questions = super::step_handlers::ask_questions(&input["questions"])?;
                StepKind::Ask(AskGate {
                    gate: step_id.clone(),
                    reason: AskReason::Text(
                        questions
                            .iter()
                            .map(|question| question.prompt.as_str())
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    gate_type: Some(super::step_handlers::QUESTIONS_GATE.to_owned()),
                })
            }
            CallTarget::Ask => {
                let reason = input
                    .get("reason")
                    .and_then(JsonValue::as_str)
                    .filter(|reason| !reason.trim().is_empty())
                    .ok_or("an ask names its reason, or its questions")?;
                if input.len() > 1 {
                    return Err("an ask takes its reason, or its questions, alone".to_owned());
                }
                StepKind::Ask(AskGate {
                    gate: step_id.clone(),
                    reason: AskReason::Text(reason.to_owned()),
                    gate_type: None,
                })
            }
        };
        let mut step = WorkflowStep {
            id: step_id,
            kind,
            artifacts: None,
            outputs: None,
            // An agent's call carries values, never workflow bindings. An
            // operation call's act is one of its fields.
            inputs,
            bindings: Vec::new(),
            act: None,
            context_skills: Vec::new(),
            when: None,
            guard: Vec::new(),
            retry_unknown_outcomes: false,
            effects: self.scope.effects,
            reach: self.scope.reach,
        };
        let fed = self.admit_references(&mut step, target)?;
        Ok(CallStep { step, act, fed })
    }

    /// Admit the inputs a call gives by reference. An input of a packet type
    /// may be a reference to a packet the project holds: one a call of this
    /// turn returned, or one an earlier run made, such as a voice packet its
    /// host reuses. Its content is read through the project's artifact store,
    /// confined to it and bound to the reference's digest, and the target's
    /// input contract checks it as that input's packet. A packet a call of
    /// this turn returned is read as that call recorded it, and the input's
    /// provenance names the call.
    fn admit_references(
        &self,
        step: &mut WorkflowStep,
        target: &CallTarget,
    ) -> Result<Vec<Fed>, String> {
        let given = step
            .inputs
            .iter()
            .filter(|(_, value)| crate::services::is_project_artifact(value))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        if given.is_empty() {
            return Ok(Vec::new());
        }
        let (declared_by, declared) = self.declared_inputs(step, target)?;
        let options = &self.scope.runtime.options;
        let artifacts =
            crate::paths::RunxPaths::resolve(&options.env, self.scope.workflow_dir).artifacts();
        let recorded = self.recorded().map_err(|error| error.to_string())?;
        // A script's reference is one a call of its step returned it, given
        // to any input as that artifact's content.
        let script = self.turn.is_none();
        let mut fed = Vec::new();
        for name in given {
            // Only an input of a packet type is given by reference; any other
            // takes the value as it is.
            let Some(input) = declared
                .get(&name)
                .filter(|input| input.packet.is_some() || script)
            else {
                continue;
            };
            let named = step.inputs[&name]
                .clone()
                .deserialize_into::<Artifact>()
                .map_err(|error| {
                    format!("input {name} is not a project artifact reference: {error}")
                })?;
            // An input explicitly taking the descriptor's own packet type
            // receives the descriptor as its value. Other packet types use
            // it as a reference to retained content below.
            if input.packet.as_ref().map(|packet| packet.as_str())
                == step.inputs[&name]
                    .as_object()
                    .and_then(|value| value.get("schema"))
                    .and_then(JsonValue::as_str)
            {
                let value = step.inputs[&name].clone();
                crate::input_contract::admit_input_value(declared_by, &name, input, &value)
                    .map_err(|error| error.to_string())?;
                step.inputs.insert(name, value);
                continue;
            }
            let produced = recorded.iter().find_map(|call| {
                let CallOutcome::Completed(run) = &call.outcome else {
                    return None;
                };
                call.outputs.iter().find_map(|part| {
                    let output = part.output.as_ref()?;
                    (part.artifact.digest == named.digest).then_some((
                        call,
                        run,
                        output,
                        &part.artifact,
                    ))
                })
            });
            if script && produced.is_none() {
                return Err(format!(
                    "input {name}: {} names no artifact a call of step {} returned",
                    named.digest, self.scope.step_id
                ));
            }
            let reference = produced.map_or(&named, |(_, _, _, artifact)| artifact);
            let value = crate::services::read_json_artifact(&artifacts, reference)
                .map_err(|error| format!("input {name}: {error}"))?;
            crate::input_contract::admit_input_value(declared_by, &name, input, &value)
                .map_err(|error| error.to_string())?;
            step.inputs.insert(name.clone(), value);
            fed.push(Fed {
                edge: produced.map(|(call, run, output, _)| ProvenanceEntry {
                    input: name.clone().into(),
                    output: output.clone(),
                    from_step: Some(call_step_id(self.scope.step_id, call.index)),
                    artifact_id: Some(reference.digest.as_str().to_owned()),
                    receipt_id: Some(run.receipt.id.to_string()),
                }),
                artifact: Reference {
                    provider: Some("runx".into()),
                    locator: Some(reference.digest.as_str().into()),
                    label: Some(name.into()),
                    ..Reference::with_uri(ReferenceType::Artifact, reference.reference.as_str())
                },
            });
        }
        Ok(fed)
    }

    /// The inputs a call's target declares, and whose they are: a catalog
    /// tool's, or a child skill's method's. An operation's input and an ask
    /// declare none.
    fn declared_inputs(
        &self,
        step: &WorkflowStep,
        target: &CallTarget,
    ) -> Result<(&'static str, BTreeMap<String, InputDefinition>), String> {
        let options = &self.scope.runtime.options;
        match target {
            CallTarget::Tool { tool } => crate::tool_catalogs::dispatch::inspect_catalog_tool(
                tool.as_str(),
                &options.env,
                self.scope.workflow_dir,
                &options.effects,
            )
            .map(|inspected| ("tool", inspected.inputs))
            .map_err(|error| format!("tool {tool} could not be resolved: {error}")),
            CallTarget::Skill {
                skill,
                bound: Some(bound),
                ..
            } => self
                .scope
                .bound_skill(step, bound)
                .map(|loaded| ("method", loaded.method.inputs))
                .map_err(|error| format!("skill {skill} could not be loaded: {error}")),
            CallTarget::Skill { skill, .. } => load_step_skill(
                self.scope.workflow_dir,
                step,
                StepSkillLoadOptions::of_run(options),
            )
            .map(|loaded| ("method", loaded.method.inputs))
            .map_err(|error| format!("skill {skill} could not be loaded: {error}")),
            CallTarget::Operation { .. }
            | CallTarget::Ask
            | CallTarget::Load
            | CallTarget::Wait { .. }
            | CallTarget::Read { .. } => Ok(("call", BTreeMap::new())),
        }
    }

    /// An operation's tool: the call fields the turn accepts, with the
    /// operation's own input schema when one connector serves it.
    fn operation_tool(
        &self,
        scope: &crate::kernel::connectors::ConnectorScope,
        allowed: &AllowedOperation,
    ) -> Result<TurnTool, RuntimeError> {
        let describing =
            |message: String| RuntimeError::effect_state("describing an operation", message);
        let declared = self
            .declared(allowed.requirement.as_str())?
            .ok_or_else(|| {
                describing(format!(
                    "requirement {} is not declared",
                    allowed.requirement
                ))
            })?;
        let declared = &declared;
        let connectors = if declared.choose.is_some() {
            declared
                .from
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>()
        } else {
            vec![allowed.requirement.as_str().to_owned()]
        };
        let mut described = Vec::new();
        for id in &connectors {
            // A pinned requirement reaches the version it pins.
            let connector = if declared.choose.is_none() {
                scope.requirement(id, declared)
            } else {
                scope.connector(id)
            }
            .map_err(|error| describing(error.to_string()))?;
            let operation = connector
                .manifest
                .operations
                .iter()
                .find(|(name, _)| name.as_str() == allowed.operation.as_str())
                .map(|(_, operation)| operation.clone())
                .ok_or_else(|| {
                    describing(format!("{id} has no operation {}", allowed.operation))
                })?;
            described.push((id.clone(), operation));
        }
        operation_descriptor(allowed, declared, &connectors, &described)
    }

    /// A listed child skill's tool, named for the skill.
    fn skill_tool(&self, target: CallTarget, name: String) -> Result<TurnTool, RuntimeError> {
        let call = self
            .call_step(0, &target, &JsonObject::new())
            .map_err(|reason| RuntimeError::effect_state("describing a child skill", reason))?;
        let loaded = load_step_skill(
            self.scope.workflow_dir,
            &call.step,
            StepSkillLoadOptions::of_run(&self.scope.runtime.options),
        )?;
        skill_descriptor(&name, loaded.description, &loaded.method, target)
    }

    /// The loads this turn completed, in order.
    fn loads(&self) -> Result<Vec<TurnLoad>, RuntimeError> {
        self.recorded()?
            .iter()
            .filter(|call| call.target == CallTarget::Load)
            .filter_map(|call| match &call.outcome {
                CallOutcome::Completed(run) if run.outcome.succeeded() => Some(run),
                _ => None,
            })
            .map(|run| {
                JsonValue::Object(run.contract.clone())
                    .deserialize_into::<TurnLoad>()
                    .map_err(|error| {
                        RuntimeError::effect_state("reading a load", error.to_string())
                    })
            })
            .collect()
    }

    /// Whether a load of this turn offers `target`.
    fn loaded_offers(&self, target: &CallTarget) -> Result<bool, RuntimeError> {
        Ok(self
            .loads()?
            .iter()
            .any(|load| load.tools.iter().any(|tool| &tool.target == target)))
    }

    /// The declaration that admits an operation call on `requirement`: the
    /// skill's own, else that of a connector this turn loaded, pinned to the
    /// manifest the load bound.
    fn declared(&self, requirement: &str) -> Result<Option<ConnectorRequirement>, RuntimeError> {
        if let Some(declared) = self.scope.connectors.get(requirement) {
            return Ok(Some(declared.clone()));
        }
        for load in self.loads()? {
            let LoadedPackage::Connector {
                connector,
                manifest_digest,
                ..
            } = &load.package
            else {
                continue;
            };
            if connector.as_str() != requirement {
                continue;
            }
            return Ok(Some(ConnectorRequirement {
                operations: load
                    .tools
                    .iter()
                    .filter_map(|tool| match &tool.target {
                        CallTarget::Operation { operation, .. } => {
                            OperationName::new(operation.as_str().to_owned())
                        }
                        _ => None,
                    })
                    .collect(),
                manifest_digest: runx_contracts::connector::Digest::new(
                    manifest_digest.as_str().to_owned(),
                ),
                choose: None,
                from: Vec::new(),
            }));
        }
        Ok(None)
    }

    /// The exact binding the latest load of connector `requirement` recorded,
    /// when the turn's skill does not declare it itself: the only connector
    /// its calls run against.
    fn loaded_binding(&self, requirement: &str) -> Result<Option<ConnectorBinding>, RuntimeError> {
        if self.scope.connectors.contains_key(requirement) {
            return Ok(None);
        }
        Ok(self.loads()?.into_iter().rev().find_map(|load| {
            let LoadedPackage::Connector { connector, .. } = &load.package else {
                return None;
            };
            if connector.as_str() != requirement {
                return None;
            }
            load.connectors
                .into_iter()
                .find(|binding| binding.connector.as_str() == requirement)
        }))
    }

    /// Each connector the turn already reaches, at the exact revision it
    /// binds: its skill's requirements, and what its loads bound.
    fn held(&self) -> Result<Vec<ConnectorBinding>, RuntimeError> {
        let mut held = Vec::new();
        let connectors = self.scope.connectors;
        if !connectors.is_empty() {
            let scope = self.connector_scope()?;
            for (key, requirement) in connectors {
                let references = if requirement.choose.is_some() {
                    requirement.from.clone()
                } else {
                    vec![key.clone()]
                };
                for reference in references {
                    let connector =
                        scope
                            .requirement(&reference, requirement)
                            .map_err(|error| {
                                RuntimeError::effect_state(
                                    "reading the skill's connectors",
                                    error.to_string(),
                                )
                            })?;
                    held.push(connector.binding());
                }
            }
        }
        for load in self.loads()? {
            held.extend(load.connectors);
        }
        Ok(held)
    }

    /// Each `project:<name>` the run already binds: those its own closure
    /// binds, and those the turn's loads bound. One directory is each for the
    /// whole run.
    fn held_projects(&self) -> Result<Vec<ProjectBinding>, RuntimeError> {
        let options = &self.scope.runtime.options;
        let paths = crate::paths::RunxPaths::resolve(&options.env, self.scope.workflow_dir);
        let mut held = options
            .packages
            .iter()
            .flat_map(|admitted| admitted.projects())
            .map(|(name, root)| crate::identity::project_binding(name, root, &paths))
            .collect::<Vec<_>>();
        for load in self.loads()? {
            held.extend(load.projects);
        }
        Ok(held)
    }

    /// The connectors the turn's skill may use, within its project.
    fn connector_scope(&self) -> Result<crate::kernel::connectors::ConnectorScope, RuntimeError> {
        crate::kernel::connectors::ConnectorScope::in_project(
            self.scope.workflow_dir,
            &crate::paths::RunxPaths::resolve(
                &self.scope.runtime.options.env,
                self.scope.workflow_dir,
            ),
        )
        .map_err(|error| {
            RuntimeError::effect_state("reading the skill's connectors", error.to_string())
        })
    }

    /// A catalog tool's tool: its description and inputs as the catalog
    /// declares them.
    fn catalog_tool(&self, tool: &NonEmptyString) -> Result<TurnTool, RuntimeError> {
        let inspected = crate::tool_catalogs::dispatch::inspect_catalog_tool(
            tool.as_str(),
            &self.scope.runtime.options.env,
            self.scope.workflow_dir,
            &self.scope.runtime.options.effects,
        )?;
        Ok(offered(
            tool.as_str(),
            inspected
                .description
                .unwrap_or_else(|| format!("Runx tool {tool}.")),
            runx_contracts::call_input_schema(&inspected.inputs).map_err(|error| {
                RuntimeError::effect_state("describing a tool", error.to_string())
            })?,
            CallTarget::Tool { tool: tool.clone() },
        ))
    }

    /// A tool the turn lists resolves.
    fn resolve_tool(&self, tool: &str) -> Result<(), String> {
        crate::tool_catalogs::dispatch::inspect_catalog_tool(
            tool,
            &self.scope.runtime.options.env,
            self.scope.workflow_dir,
            &self.scope.runtime.options.effects,
        )
        .map(|_| ())
        .map_err(|error| format!("tool {tool} could not be resolved: {error}"))
    }
}

impl<A> Turn for OpenTurn<'_, A>
where
    A: SkillAdapter,
{
    fn invocation(&self) -> &TurnInvocation {
        self.invocation
    }

    fn step_id(&self) -> &str {
        self.ledger.scope.step_id
    }

    fn tools(&self) -> Result<Vec<TurnTool>, RuntimeError> {
        let allowance = &self.ledger.scope.allowance;
        let mut tools = Vec::new();
        for tool in &allowance.tools {
            tools.push(self.ledger.catalog_tool(tool)?);
        }
        if !allowance.operations.is_empty() {
            let connectors = self.ledger.connector_scope()?;
            for allowed in &allowance.operations {
                tools.push(self.ledger.operation_tool(&connectors, allowed)?);
            }
        }
        for skill in allowance.named_skills() {
            tools.push(self.ledger.skill_tool(
                CallTarget::Skill {
                    skill: skill.clone(),
                    method: None,
                    bound: None,
                },
                format!("skill__{}", skill.as_str().trim_start_matches(['.', '/'])),
            )?);
        }
        if allowance.rules().next().is_some() {
            tools.push(offered(
                "load",
                "Load a skill or connector the turn's catalog rules admit: {skill, method?}, or {connector} by name. A skill's load answers with its manual (how to use it) and its methods, each with its purpose and inputs; it binds the method named, or the skill's only method, and with no method named a skill of several binds none, so read the manual and load again naming one. Each method or operation a load binds is a tool of this turn from then on. Binding a package below the rule's trust tier, or a project package that reaches a connector this turn does not, waits on the person's trust."
                    .to_owned(),
                load_schema()?,
                CallTarget::Load,
            ));
        }
        // What a load added, as it described it when it bound it: a package
        // that changed since is refused at its call, never re-read here.
        for load in self.ledger.loads()? {
            tools.extend(load.tools.into_iter().filter(|tool| {
                !matches!(&tool.target, CallTarget::Operation { requirement, operation }
                if allowance.operations.iter().any(|allowed| {
                    &allowed.requirement == requirement && &allowed.operation == operation
                }))
            }));
        }
        if allowance.asks {
            tools.push(offered(
                "ask_human",
                "Ask the human: a yes or no on your reason, or one to three questions, each with up to four options (the recommended first) and free text where it allows. The call stays pending until they answer."
                    .to_owned(),
                ask_schema()?,
                CallTarget::Ask,
            ));
        }
        distinct_names(&mut tools);
        Ok(tools)
    }

    fn calls(&self) -> Result<Vec<TurnCallResult>, RuntimeError> {
        self.ledger
            .recorded()?
            .iter()
            .map(|call| {
                self.ledger
                    .result(&Made::of(call), &call.outcome, &call.outputs)
            })
            .collect()
    }

    fn admit(
        &mut self,
        host: &mut dyn Host,
        call_id: Option<String>,
        target: CallTarget,
        input: JsonObject,
    ) -> Result<Admission, RuntimeError> {
        if let Some(call_id) = &call_id {
            if call_id.is_empty()
                || call_id.len() > CALL_ID_BYTES
                || !call_id.bytes().all(|byte| byte.is_ascii_graphic())
            {
                return Err(RuntimeError::ContinuationRejected {
                    reason: format!(
                        "call_id must be 1-{CALL_ID_BYTES} printable ASCII characters without spaces"
                    ),
                });
            }
            if let Some(known) = self
                .ledger
                .recorded()?
                .iter()
                .find(|known| known.call_id.as_deref() == Some(call_id.as_str()))
            {
                let digest = crate::execution::run_record::input_digest;
                if known.target != target || digest(&known.input)? != digest(&input)? {
                    return Err(RuntimeError::ContinuationRejected {
                        reason: format!(
                            "call_id {call_id} names call {} of the turn, made with another target or input",
                            known.index
                        ),
                    });
                }
                return self
                    .ledger
                    .result(&Made::of(known), &known.outcome, &known.outputs)
                    .map(|result| Admission::Settled(Box::new(result)));
            }
        }
        let index = self
            .ledger
            .recorded()?
            .iter()
            .map(|call| call.index + 1)
            .max()
            .unwrap_or(0);
        let made = Made {
            index,
            call_id,
            target,
            input,
            standing: None,
            cancelled: false,
        };
        self.ledger
            .admit_made(&mut AnsweringHost::new(host, None), made)
    }

    fn execute(
        &self,
        host: &mut dyn Host,
        admitted: AdmittedCall,
    ) -> Result<ExecutedCall, RuntimeError> {
        self.ledger
            .execute_admitted(&mut AnsweringHost::new(host, None), admitted)
    }

    fn conclude(&mut self, executed: ExecutedCall) -> Result<TurnCallResult, RuntimeError> {
        self.ledger.conclude_executed(executed)
    }

    fn settle(&mut self, host: &mut dyn Host, index: u32) -> Result<TurnCallResult, RuntimeError> {
        self.ledger.further(host, index)
    }
}

/// A recorded request as the host protocol carries it.
pub(super) fn resolution_request(
    request: &PendingRequest,
) -> Result<ResolutionRequest, RuntimeError> {
    serde_json::to_value(&request.value)
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeError::json("reading a pending request", source))
}

/// The person's acknowledgement that runs a claimed call or step again.
pub(super) fn retry_acknowledgement() -> JsonValue {
    JsonValue::Object(JsonObject::from([(
        "retry".to_owned(),
        JsonValue::Bool(true),
    )]))
}

fn reason(text: String) -> NonEmptyString {
    NonEmptyString::new(text).unwrap_or_else(|| "the call gave no reason".into())
}

fn label(target: &CallTarget) -> String {
    match target {
        CallTarget::Tool { tool } => format!("tool {tool}"),
        CallTarget::Operation {
            requirement,
            operation,
        } => format!("operation {requirement}/{operation}"),
        CallTarget::Skill {
            skill,
            method: Some(method),
            ..
        } => format!("skill {skill}#{method}"),
        CallTarget::Skill { skill, .. } => format!("skill {skill}"),
        CallTarget::Ask => "ask".to_owned(),
        CallTarget::Load => "load".to_owned(),
        CallTarget::Wait { .. } => "wait".to_owned(),
        CallTarget::Read { digest } => format!("read {digest}"),
    }
}

/// An operation's tool: the call fields the turn accepts, with the
/// operation's own input schema when one connector serves it. `described`
/// holds each connector the call may choose, with its declaration of the
/// operation.
pub(super) fn operation_descriptor(
    allowed: &AllowedOperation,
    declared: &ConnectorRequirement,
    connectors: &[String],
    described: &[(String, runx_contracts::connector::ConnectorOperation)],
) -> Result<TurnTool, RuntimeError> {
    let describing =
        |message: String| RuntimeError::effect_state("describing an operation", message);
    let writes = described
        .iter()
        .any(|(_, operation)| operation.access() == OperationAccess::Mutate);
    let amount = described
        .iter()
        .any(|(_, operation)| operation.amount.requires_amount());
    let input = match described {
        [(_, operation)] => operation.input_schema.clone().map_or_else(
            || JsonValue::Object(object_schema([], &[])),
            JsonValue::Object,
        ),
        _ => JsonValue::Object(JsonObject::from([(
            "type".to_owned(),
            JsonValue::String("object".to_owned()),
        )])),
    };
    let mut properties = vec![("input", input)];
    let mut required = vec!["input"];
    if writes {
        properties.push(("idempotency_key", string_schema()));
        required.push("idempotency_key");
    }
    if amount {
        properties.push((
            "amount",
            JsonValue::Object(object_schema(
                [
                    (
                        "units",
                        JsonValue::Object(JsonObject::from([
                            ("type".to_owned(), JsonValue::String("integer".to_owned())),
                            ("minimum".to_owned(), JsonValue::Number(0_u64.into())),
                        ])),
                    ),
                    ("unit", string_schema()),
                ],
                &["units", "unit"],
            )),
        ));
        required.push("amount");
    }
    // The act the call performs, as the agent states it: why it is made.
    let act = JsonValue::from_serialize(&runx_contracts::CallAct::json_schema())
        .map_err(|error| describing(error.to_string()))?;
    properties.push(("act", act));
    if declared.choose.is_some() {
        properties.push((
            "connector",
            JsonValue::Object(JsonObject::from([(
                "enum".to_owned(),
                JsonValue::Array(connectors.iter().cloned().map(JsonValue::String).collect()),
            )])),
        ));
        required.push("connector");
    }
    let mut input_schema = object_schema([], &required);
    input_schema.insert(
        "properties".to_owned(),
        JsonValue::Object(
            properties
                .into_iter()
                .map(|(name, schema)| (name.to_owned(), schema))
                .collect(),
        ),
    );
    let description = described
        .iter()
        .map(|(id, operation)| format!("{id}/{}: {}", allowed.operation, operation.description))
        .collect::<Vec<_>>()
        .join("; ");
    let description = if writes {
        format!(
            "{description} A write: name it with an idempotency_key you reuse when retrying it."
        )
    } else {
        description
    };
    Ok(offered(
        &format!("{}__{}", allowed.requirement, allowed.operation),
        description,
        input_schema,
        CallTarget::Operation {
            requirement: allowed.requirement.clone(),
            operation: allowed.operation.clone(),
        },
    ))
}

/// A child skill's tool: its method's purpose (else the skill's description)
/// and its method's inputs, beside the act a skill call states.
pub(super) fn skill_descriptor(
    name: &str,
    skill_description: Option<String>,
    method: &runx_parser::SkillMethodDefinition,
    target: CallTarget,
) -> Result<TurnTool, RuntimeError> {
    let describing =
        |message: String| RuntimeError::effect_state("describing a child skill", message);
    let description = method
        .description
        .clone()
        .or(skill_description)
        .unwrap_or_else(|| format!("Run the {} skill.", method.name));
    let mut input_schema = runx_contracts::call_input_schema(&method.inputs)
        .map_err(|error| describing(error.to_string()))?;
    if let Some(JsonValue::Object(properties)) = input_schema.get_mut("properties") {
        properties.insert(
            "act".to_owned(),
            JsonValue::from_serialize(&CallAct::json_schema())
                .map_err(|error| describing(error.to_string()))?,
        );
    }
    Ok(offered(name, description, input_schema, target))
}

/// What a load call names: `{skill, method?}` or `{connector}`.
fn load_target(input: &JsonObject) -> Result<LoadTarget, String> {
    let text = |field: &str| -> Result<Option<String>, String> {
        input
            .get(field)
            .map(|value| {
                value
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| format!("a load's {field} is a non-empty string"))
            })
            .transpose()
    };
    if let Some(field) = input
        .keys()
        .find(|field| !["skill", "method", "connector"].contains(&field.as_str()))
    {
        return Err(format!(
            "a load takes skill and method, or connector, not {field}"
        ));
    }
    match (text("skill")?, text("method")?, text("connector")?) {
        (Some(skill), method, None) => Ok(LoadTarget::Skill { skill, method }),
        (None, None, Some(name)) => Ok(LoadTarget::Connector { name }),
        _ => {
            Err("a load names a skill, with its method if it wants one, or a connector".to_owned())
        }
    }
}

/// A loaded method's tool name: its package, without its version, and its
/// method.
pub(super) fn loaded_tool_name(package: &str, method: &str) -> String {
    let name = package.split_once('@').map_or(package, |(name, _)| name);
    let name = name.strip_prefix("skills/").unwrap_or(name);
    format!("skill__{name}__{method}")
}

/// A load's input: a skill with an optional method, or a connector by
/// name.
fn load_schema() -> Result<JsonObject, RuntimeError> {
    let text = serde_json::json!({"type": "string", "minLength": 1});
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "skill": text,
            "method": text,
            "connector": text,
        },
        "oneOf": [
            {"required": ["skill"], "not": {"required": ["connector"]}},
            {"required": ["connector"], "not": {"anyOf": [{"required": ["skill"]}, {"required": ["method"]}]}},
        ],
    }))
    .map_err(|source| RuntimeError::json("describing a load", source))
}

/// One call a turn offers, named for MCP.
fn offered(
    name: &str,
    description: String,
    input_schema: JsonObject,
    target: CallTarget,
) -> TurnTool {
    TurnTool {
        name: mcp_name(name).into(),
        description: NonEmptyString::new(description).unwrap_or_else(|| label(&target).into()),
        target,
        input_schema,
    }
}

/// Give each tool a distinct name, clear of the host's controls: a repeat
/// takes the first free `_<n>` suffix, shortening the name to keep it within
/// the grammar.
fn distinct_names(tools: &mut [TurnTool]) {
    let mut seen = std::collections::BTreeSet::from([
        TURN_STATUS_TOOL.to_owned(),
        TURN_READ_TOOL.to_owned(),
        TURN_COMPLETE_TOOL.to_owned(),
    ]);
    for tool in tools {
        let base = tool.name.as_str().to_owned();
        let mut suffix = 1;
        while !seen.insert(tool.name.as_str().to_owned()) {
            suffix += 1;
            let tail = format!("_{suffix}");
            let head = base.get(..base.len().min(MCP_NAME_BYTES - tail.len()));
            tool.name = format!("{}{tail}", head.unwrap_or_default()).into();
        }
    }
}

/// The longest MCP tool name.
const MCP_NAME_BYTES: usize = 64;

/// A name in the MCP tool-name grammar: `[A-Za-z0-9_-]`, at most 64 bytes.
fn mcp_name(raw: &str) -> String {
    let name = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    name.get(..name.len().min(MCP_NAME_BYTES))
        .unwrap_or(&name)
        .to_owned()
}

/// An ask's input: its reason, or its questions.
fn ask_schema() -> Result<JsonObject, RuntimeError> {
    let text = serde_json::json!({"type": "string", "minLength": 1});
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "reason": text,
            "questions": {
                "type": "array",
                "minItems": 1,
                "maxItems": 3,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "prompt"],
                    "properties": {
                        "id": text,
                        "prompt": text,
                        "options": {
                            "type": "array",
                            "maxItems": 4,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["label"],
                                "properties": {"label": text, "detail": {"type": "string"}},
                            },
                        },
                        "free_text": {"type": "boolean"},
                        "required": {"type": "boolean"},
                    },
                },
            },
        },
        "oneOf": [{"required": ["reason"]}, {"required": ["questions"]}],
    }))
    .map_err(|source| RuntimeError::json("describing an ask", source))
}

fn string_schema() -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("type".to_owned(), JsonValue::String("string".to_owned())),
        ("minLength".to_owned(), JsonValue::Number(1_u64.into())),
    ]))
}

fn object_schema<const N: usize>(
    properties: [(&str, JsonValue); N],
    required: &[&str],
) -> JsonObject {
    JsonObject::from([
        ("type".to_owned(), JsonValue::String("object".to_owned())),
        ("additionalProperties".to_owned(), JsonValue::Bool(false)),
        (
            "properties".to_owned(),
            JsonValue::Object(
                properties
                    .into_iter()
                    .map(|(name, schema)| (name.to_owned(), schema))
                    .collect(),
            ),
        ),
        (
            "required".to_owned(),
            JsonValue::Array(
                required
                    .iter()
                    .map(|name| JsonValue::String((*name).to_owned()))
                    .collect(),
            ),
        ),
    ])
}

fn pending_request(request: &ResolutionRequest) -> Result<PendingRequest, RuntimeError> {
    let value = serde_json::to_value(request)
        .and_then(serde_json::from_value::<JsonValue>)
        .map_err(|source| RuntimeError::json("encoding a pending request", source))?;
    let id = match request {
        ResolutionRequest::Input { id, .. }
        | ResolutionRequest::Approval { id, .. }
        | ResolutionRequest::Turn { id, .. }
        | ResolutionRequest::Retry { id, .. }
        | ResolutionRequest::Purchase { id, .. }
        | ResolutionRequest::Wait { id, .. } => id.to_string(),
    };
    Ok(PendingRequest {
        id,
        value,
        digest: request_digest(request)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> TurnTool {
        offered(
            name,
            "a tool".to_owned(),
            JsonObject::new(),
            CallTarget::Ask,
        )
    }

    #[test]
    fn names_stay_distinct_and_within_the_grammar() {
        let long = "x".repeat(80);
        let mut tools = [
            tool(&long),
            tool(&long),
            tool(&long),
            tool("a.b"),
            tool("a_b"),
            tool("turn.status"),
        ];
        distinct_names(&mut tools);
        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names[0], "x".repeat(64));
        assert_eq!(names[1], format!("{}_2", "x".repeat(62)));
        assert_eq!(names[2], format!("{}_3", "x".repeat(62)));
        assert_eq!(&names[3..], ["a_b", "a_b_2", "turn_status_2"]);
        assert!(names.iter().all(|name| name.len() <= MCP_NAME_BYTES));
    }
}
