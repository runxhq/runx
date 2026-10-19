//! Agent turns. An agent step whose act is unanswered holds its turn open,
//! and its host works the turn: the agent calls back into the run within the
//! act's allowance. A call runs as one step under the agent step's own
//! runtime, at whatever depth the step runs, so it is admitted, executed and
//! sealed exactly as the step kind it names would be, and it is recorded in
//! the run's stream before its outcome is returned. A skill call runs its
//! child skill as a child run of its own, whose turns are open turns too. The
//! turn ends when the host answers the act.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::{AmountRule, OperationAccess, OperationName};
use runx_contracts::schema::{NonEmptyString, RunxSchema};
use runx_contracts::{
    AgentActInvocation, AllowedOperation, BoundClosure, CallAct, InputDefinition, JsonObject,
    JsonValue, LoadedPackage, ProjectArtifactRef, ProvenanceEntry, Reference, ReferenceType,
    ResolutionRequest, RunCallOutput, TurnCallOutcome, TurnCallResult, TurnLoad, TurnStatus,
    TurnTarget, TurnTool,
};
use runx_parser::{
    AgentTask, AskGate, ConnectorRequirement, GraphStep, LoadStep, LoadTarget, OperationStepTarget,
    SkillTarget, StepKind, ToolTarget,
};

use super::super::graph::{
    LoadedStepSkill, LoadedStepSkillPackage, StepSkillLoadOptions, load_step_skill, step_skill,
    turn_skill_target,
};
use super::super::run_record::{
    Answer, CallOutcome, CallRecord, PendingRequest, RecordedCall, call_step_id, request_digest,
    waiting_requests,
};
use super::graph_engine::AnsweringHost;
use super::step_handlers::{
    StepRunRequest, output_error, run_step_with_inputs, run_step_with_loaded_skill_inputs,
};
use super::{Runtime, StepRun};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;

/// The turn an unanswered agent act holds open, as its host works it.
pub trait Turn {
    /// The act the turn answers; its id is the turn's address.
    fn invocation(&self) -> &AgentActInvocation;

    /// The agent step that holds the turn.
    fn step_id(&self) -> &str;

    /// The calls the turn offers its agent: one per allowance entry, named
    /// for MCP.
    fn tools(&self) -> Result<Vec<TurnTool>, RuntimeError>;

    /// How each call the turn made so far ended.
    fn calls(&self) -> Result<Vec<TurnCallResult>, RuntimeError>;

    /// The turn as its agent reads it.
    fn status(&self) -> Result<TurnStatus, RuntimeError> {
        Ok(TurnStatus {
            invocation: self.invocation().clone(),
            tools: self.tools()?,
            calls: self.calls()?,
        })
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
        target: TurnTarget,
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
        target: TurnTarget,
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
fn offered_target(turn: &dyn Turn, tool: &str) -> Result<TurnTarget, RuntimeError> {
    turn.tools()?
        .into_iter()
        .find(|offered| offered.name.as_str() == tool)
        .map(|offered| offered.target)
        .ok_or_else(|| RuntimeError::ContinuationRejected {
            reason: format!(
                "turn {} offers no tool {tool}",
                turn.invocation().id.as_str()
            ),
        })
}

/// The controls a host presents beside a turn's own tools; a turn's tools
/// never take these names.
pub(crate) const TURN_STATUS_TOOL: &str = "turn_status";
pub(crate) const TURN_COMPLETE_TOOL: &str = "turn_complete";

const OPERATION_CALL_FIELDS: &[&str] = &["connector", "input", "idempotency_key", "amount", "act"];

/// The longest call id a host may pin.
const CALL_ID_BYTES: usize = 200;

/// Where a turn runs: the agent step, its attempt, and its runtime.
pub(super) struct TurnScope<'a, A> {
    pub(super) runtime: &'a Runtime<A>,
    pub(super) graph_dir: &'a Path,
    pub(super) graph_name: &'a str,
    pub(super) step_id: &'a str,
    pub(super) attempt: u32,
    pub(super) agent: &'a AgentTask,
}

/// The open turn of one agent step: where it runs, the act it answers, and
/// the calls made in it so far.
pub(super) struct OpenTurn<'a, A> {
    scope: TurnScope<'a, A>,
    invocation: &'a AgentActInvocation,
    /// The act as the run records it.
    request: PendingRequest,
    /// The calls the run records for this turn, read when first needed.
    calls: OnceCell<Vec<RecordedCall>>,
}

/// A call as its host made it: its place in the turn, the id its host
/// pinned on it, what it names and its input.
#[derive(Clone)]
struct Made {
    index: u32,
    call_id: Option<String>,
    target: TurnTarget,
    input: JsonObject,
    /// The standing approval that admitted it on the floor, if one did.
    standing: Option<runx_contracts::StandingApproval>,
}

impl Made {
    fn of(call: &RecordedCall) -> Self {
        Self {
            index: call.index,
            call_id: call.call_id.clone(),
            target: call.target.clone(),
            input: call.input.clone(),
            standing: call.standing.clone(),
        }
    }
}

/// The step a call runs as, with what rides beside its inputs.
struct CallStep {
    step: GraphStep,
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
}

impl AdmittedCall {
    /// The call's place in its turn.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.made.index
    }
}

/// What an admitted call's execution came to, to record.
pub struct ExecutedCall {
    made: Made,
    claim: CallOutcome,
    ran: Ran,
    /// The answers its execution consumed.
    answers: Vec<Answer>,
}

/// How a call's step went.
enum Ran {
    Settled(CallOutcome),
    /// The step waits on `request`: its own, or one its child run raised.
    Waiting(Box<ResolutionRequest>),
    /// The step's effect may have happened and is not confirmed.
    Unconfirmed,
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
        let ResolutionRequest::AgentAct { invocation, .. } = request else {
            return Err(RuntimeError::EngineInvariant {
                context: "opening an agent turn",
                message: "an agent step asked for something other than its act".to_owned(),
            });
        };
        Ok(Self {
            scope,
            invocation,
            request: pending_request(request)?,
            calls: OnceCell::new(),
        })
    }

    /// The turn's calls as the run records them, in order.
    fn recorded(&self) -> Result<&[RecordedCall], RuntimeError> {
        if let Some(calls) = self.calls.get() {
            return Ok(calls);
        }
        let read = self
            .scope
            .runtime
            .options
            .run
            .calls(self.scope.step_id, self.scope.attempt)?;
        Ok(self.calls.get_or_init(|| read))
    }

    fn recorded_mut(&mut self) -> Result<&mut Vec<RecordedCall>, RuntimeError> {
        self.recorded()?;
        self.calls
            .get_mut()
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
                CallOutcome::Completed(_) | CallOutcome::Refused(_) | CallOutcome::Failed(_) => {
                    None
                }
            }))
    }

    /// Why the turn's allowance refuses `target`, if it does.
    fn refusal(&self, target: &TurnTarget) -> Result<Option<String>, RuntimeError> {
        let allowance = &self.scope.agent.allowance;
        let allowed =
            match target {
                TurnTarget::Tool { tool } => allowance.tools.contains(tool),
                TurnTarget::Operation {
                    requirement,
                    operation,
                } => {
                    allowance.operations.iter().any(|item| {
                        &item.requirement == requirement && &item.operation == operation
                    }) || self.loaded_offers(target)?
                }
                TurnTarget::Skill {
                    skill,
                    runner: None,
                    bound: None,
                } => allowance.skill_refs().any(|listed| listed == skill),
                TurnTarget::Skill { .. } => self.loaded_offers(target)?,
                TurnTarget::Ask => allowance.asks,
                TurnTarget::Load => allowance.rules().next().is_some(),
            };
        if !allowed {
            return Ok(Some(format!(
                "{} is not in the turn's allowance",
                label(target)
            )));
        }
        if matches!(target, TurnTarget::Skill { .. }) {
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
            if !matches!(call.target, TurnTarget::Skill { .. }) {
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
        match self.admit_made(&mut host, made)? {
            Admission::Settled(result) => Ok(*result),
            Admission::Admitted(admitted) => {
                let executed = self.execute_admitted(&mut host, *admitted)?;
                self.conclude_executed(executed)
            }
        }
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
            self.scope.graph_dir,
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
        let claim = match &made.target {
            _ if super::reaches_world(&call.step.kind) => {
                CallOutcome::Claimed(self.retry(made.index, &call.step)?)
            }
            TurnTarget::Skill { .. } => {
                CallOutcome::Child(self.scope.runtime.options.run.child_id(&call.step.id))
            }
            // A call that reaches nothing holds no claim: it runs now, under
            // the driver.
            TurnTarget::Tool { .. }
            | TurnTarget::Operation { .. }
            | TurnTarget::Ask
            | TurnTarget::Load => {
                let ran = self.run_call_step(&call, &made.target, admitted_by, host)?;
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
                    Ran::Unconfirmed => Err(RuntimeError::EngineInvariant {
                        context: "running a turn call",
                        message: "an unclaimed call has no effect to leave unconfirmed".to_owned(),
                    }),
                };
            }
        };
        let authorized = std::mem::take(&mut host.answers);
        self.record(&made, claim.clone(), &authorized)?;
        Ok(Admission::Admitted(Box::new(AdmittedCall {
            made,
            call,
            claim,
            admitted_by,
        })))
    }

    /// Execute an admitted call through `host`; what it consumed rides with
    /// what it came to.
    fn execute_admitted(
        &self,
        host: &mut AnsweringHost<'_>,
        admitted: AdmittedCall,
    ) -> Result<ExecutedCall, RuntimeError> {
        let AdmittedCall {
            made,
            call,
            claim,
            admitted_by,
        } = admitted;
        let ran = self.run_call_step(&call, &made.target, admitted_by, host)?;
        Ok(ExecutedCall {
            made,
            claim,
            ran,
            answers: std::mem::take(&mut host.answers),
        })
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
        } = executed;
        match (ran, claim) {
            (Ran::Settled(outcome), _) => self.settled(made, outcome, &answers),
            (Ran::Waiting(_) | Ran::Unconfirmed, claim @ CallOutcome::Child(_)) => {
                self.result(&made, &claim, &[])
            }
            (Ran::Waiting(request), _) => {
                let pending = CallOutcome::Pending(pending_request(&request)?);
                self.settled(made, pending, &answers)
            }
            (Ran::Unconfirmed, claim) => self.result(&made, &claim, &[]),
        }
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
        match self.run_call_step(&step, &made.target, Vec::new(), &mut answering)? {
            Ran::Settled(outcome) => {
                let answers = answering.answers;
                self.settled(made, outcome, &answers)
            }
            Ran::Waiting(_) | Ran::Unconfirmed => self.result(&made, &call.outcome, &call.outputs),
        }
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
    fn retry(&self, index: u32, step: &GraphStep) -> Result<PendingRequest, RuntimeError> {
        pending_request(&super::unsettled_retry(
            self.retry_id(index),
            &format!("call {index}"),
            super::resend_of(step, self.scope.graph_dir, &self.scope.runtime.options.env),
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
        let outputs = match &outcome {
            CallOutcome::Completed(run) => self.output_artifacts(made.index, run)?,
            _ => Vec::new(),
        };
        self.scope.runtime.options.run.record_call(&CallRecord {
            step_id: self.scope.step_id,
            attempt: self.scope.attempt,
            turn: &self.request,
            index: made.index,
            call_id: made.call_id.as_deref(),
            target: &made.target,
            input: &made.input,
            outcome: &outcome,
            outputs: &outputs,
            standing: made.standing.as_ref(),
            answers,
        })?;
        let call = RecordedCall {
            step_id: self.scope.step_id.to_owned(),
            attempt: self.scope.attempt,
            index: made.index,
            call_id: made.call_id.clone(),
            target: made.target.clone(),
            input: made.input.clone(),
            standing: made.standing.clone(),
            outcome,
            outputs: outputs.clone(),
        };
        let calls = self.recorded_mut()?;
        match calls.iter_mut().find(|known| known.index == call.index) {
            Some(known) => *known = call,
            None => calls.push(call),
        }
        Ok(outputs)
    }

    /// A completed call's output as project artifacts of its run: one per
    /// packet output, holding the packet's data, and one for the rest. Each
    /// is content-addressed, so a host reads the call's outcome from the
    /// run's record alone.
    fn output_artifacts(
        &self,
        index: u32,
        run: &StepRun,
    ) -> Result<Vec<RunCallOutput>, RuntimeError> {
        let options = &self.scope.runtime.options;
        let artifacts =
            crate::paths::RunxPaths::resolve(&options.env, self.scope.graph_dir).artifacts();
        let label = call_step_id(self.scope.step_id, index);
        let persist = |label: &str, value: &JsonValue| {
            crate::services::persist_json_artifact(
                &artifacts,
                options.run.run_id(),
                "calls",
                label,
                value,
            )
            .and_then(|reference| {
                serde_json::to_value(reference)
                    .and_then(serde_json::from_value::<ProjectArtifactRef>)
                    .map_err(|error| error.to_string())
            })
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

    /// How a call stands, as its agent reads it. A skill call waits on what
    /// its child run waits on.
    fn result(
        &self,
        made: &Made,
        outcome: &CallOutcome,
        outputs: &[RunCallOutput],
    ) -> Result<TurnCallResult, RuntimeError> {
        let outcome = match outcome {
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
        };
        Ok(TurnCallResult {
            index: made.index,
            target: made.target.clone(),
            input_digest: crate::execution::run_record::input_digest(&made.input)?.into(),
            call_id: made.call_id.clone().map(Into::into),
            outcome,
        })
    }

    /// Run a call as its step; a skill call's act becomes its child run's.
    /// What admitted it on the floor, and the artifacts its inputs were given
    /// by, ride in its receipt; where those inputs came from rides in its
    /// provenance.
    fn run_call_step(
        &self,
        call: &CallStep,
        target: &TurnTarget,
        admitted_by: Vec<Reference>,
        host: &mut dyn Host,
    ) -> Result<Ran, RuntimeError> {
        let scope = &self.scope;
        let step = &call.step;
        let mut admission_refs = admitted_by;
        admission_refs.extend(call.fed.iter().map(|fed| fed.artifact.clone()));
        let request = StepRunRequest {
            runtime: scope.runtime,
            graph_dir: scope.graph_dir,
            graph_name: scope.graph_name,
            step,
            attempt: 1,
            inputs: step.inputs.clone(),
            child_act: call.act.clone(),
            provenance: call.fed.iter().filter_map(|fed| fed.edge.clone()).collect(),
            admission_refs,
            host,
        };
        let result = match target {
            TurnTarget::Skill {
                bound: Some(bound), ..
            } => self
                .bound_skill(step, bound)
                .and_then(|skill| run_step_with_loaded_skill_inputs(request, skill)),
            TurnTarget::Skill { .. } => load_step_skill(
                scope.graph_dir,
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

    /// The step a call runs as, a skill call's act, and the inputs it gave
    /// by reference. A malformed call is refused before it runs.
    fn call_step(
        &self,
        index: u32,
        target: &TurnTarget,
        input: &JsonObject,
    ) -> Result<CallStep, String> {
        let step_id = call_step_id(self.scope.step_id, index);
        let mut inputs = input.clone();
        let mut act = None;
        let (kind, scopes) = match target {
            TurnTarget::Tool { tool } => (
                StepKind::Tool(ToolTarget::Catalog(tool.as_str().to_owned())),
                self.declared_scopes(tool.as_str())?,
            ),
            TurnTarget::Operation {
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
                let target = OperationStepTarget {
                    requirement: requirement.as_str().to_owned(),
                    operation: Some(
                        OperationName::new(operation.as_str().to_owned())
                            .ok_or_else(|| format!("{operation} is not an operation name"))?,
                    ),
                    declared: self
                        .declared(requirement.as_str())
                        .map_err(|error| error.to_string())?,
                };
                (StepKind::Operation(target), Vec::new())
            }
            TurnTarget::Skill { skill, runner, .. } => {
                // `act` is never a skill input: it is why the skill is run.
                act = inputs
                    .remove("act")
                    .map(|value| CallAct::parse(&value))
                    .transpose()?;
                let target = match runner {
                    Some(runner) => SkillTarget {
                        reference: skill.as_str().to_owned(),
                        runner: Some(runner.as_str().to_owned()),
                    },
                    None => turn_skill_target(skill.as_str()),
                };
                (StepKind::Skill(target), Vec::new())
            }
            TurnTarget::Load => (
                StepKind::Load(LoadStep {
                    target: load_target(input)?,
                    rules: self.scope.agent.allowance.rules().copied().collect(),
                    held: self.held().map_err(|error| error.to_string())?,
                }),
                Vec::new(),
            ),
            // An ask is a yes or no on its reason, or questions the person
            // answers.
            TurnTarget::Ask if input.len() == 1 && input.contains_key("questions") => {
                let questions = super::step_handlers::ask_questions(&input["questions"])?;
                (
                    StepKind::Ask(AskGate {
                        gate: step_id.clone(),
                        reason: questions
                            .iter()
                            .map(|question| question.prompt.as_str())
                            .collect::<Vec<_>>()
                            .join(" "),
                        gate_type: Some(super::step_handlers::QUESTIONS_GATE.to_owned()),
                    }),
                    Vec::new(),
                )
            }
            TurnTarget::Ask => {
                let reason = input
                    .get("reason")
                    .and_then(JsonValue::as_str)
                    .filter(|reason| !reason.trim().is_empty())
                    .ok_or("an ask names its reason, or its questions")?;
                if input.len() > 1 {
                    return Err("an ask takes its reason, or its questions, alone".to_owned());
                }
                (
                    StepKind::Ask(AskGate {
                        gate: step_id.clone(),
                        reason: reason.to_owned(),
                        gate_type: None,
                    }),
                    Vec::new(),
                )
            }
        };
        let mut step = GraphStep {
            id: step_id,
            label: Some(label(target)),
            kind,
            artifacts: None,
            outputs: None,
            // An agent's call carries values, never graph bindings. An
            // operation call's act is one of its fields.
            inputs,
            bindings: Vec::new(),
            act: None,
            context_skills: Vec::new(),
            scopes,
            when: None,
            duplicates: None,
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
        step: &mut GraphStep,
        target: &TurnTarget,
    ) -> Result<Vec<Fed>, String> {
        let given = step
            .inputs
            .iter()
            .filter(|(_, value)| is_artifact_ref(value))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        if given.is_empty() {
            return Ok(Vec::new());
        }
        let (owner, declared) = self.declared_inputs(step, target)?;
        let options = &self.scope.runtime.options;
        let artifacts =
            crate::paths::RunxPaths::resolve(&options.env, self.scope.graph_dir).artifacts();
        let recorded = self.recorded().map_err(|error| error.to_string())?;
        let mut fed = Vec::new();
        for name in given {
            // Only an input of a packet type is given by reference; any other
            // takes the value as it is.
            let Some(input) = declared.get(&name).filter(|input| input.packet.is_some()) else {
                continue;
            };
            let named = step.inputs[&name]
                .clone()
                .deserialize_into::<ProjectArtifactRef>()
                .map_err(|error| {
                    format!("input {name} is not a project artifact reference: {error}")
                })?;
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
            let reference = produced.map_or(&named, |(_, _, _, artifact)| artifact);
            let packet = JsonValue::from_serialize(reference)
                .map_err(|error| error.to_string())
                .and_then(|recorded| crate::services::read_json_artifact(&artifacts, &recorded))
                .map_err(|error| format!("input {name}: {error}"))?;
            // An artifact input takes the packet in its envelope, as a step
            // emits it.
            let value = if input.artifact == Some(true) {
                JsonValue::Object(JsonObject::from([("data".to_owned(), packet)]))
            } else {
                packet
            };
            crate::input_contract::admit_input_value(owner, &name, input, &value)
                .map_err(|error| error.to_string())?;
            step.inputs.insert(name.clone(), value);
            fed.push(Fed {
                edge: produced.map(|(call, run, output, _)| ProvenanceEntry {
                    input: name.clone().into(),
                    output: output.clone(),
                    from_step: Some(call_step_id(self.scope.step_id, call.index)),
                    artifact_id: Some(reference.digest.clone()),
                    receipt_id: Some(run.receipt.id.to_string()),
                }),
                artifact: Reference {
                    provider: Some("runx".into()),
                    locator: Some(reference.digest.clone().into()),
                    label: Some(name.into()),
                    ..Reference::with_uri(ReferenceType::Artifact, reference.r#ref.clone())
                },
            });
        }
        Ok(fed)
    }

    /// The inputs a call's target declares, and whose they are: a catalog
    /// tool's, or a child skill's runner's. An operation's input and an ask
    /// declare none.
    fn declared_inputs(
        &self,
        step: &GraphStep,
        target: &TurnTarget,
    ) -> Result<(&'static str, BTreeMap<String, InputDefinition>), String> {
        let options = &self.scope.runtime.options;
        match target {
            TurnTarget::Tool { tool } => crate::tool_catalogs::dispatch::inspect_catalog_tool(
                tool.as_str(),
                &options.env,
                self.scope.graph_dir,
                &options.effects,
            )
            .map(|inspected| ("tool", inspected.inputs))
            .map_err(|error| format!("tool {tool} could not be resolved: {error}")),
            TurnTarget::Skill {
                skill,
                bound: Some(bound),
                ..
            } => self
                .bound_skill(step, bound)
                .map(|loaded| ("runner", loaded.runner.inputs))
                .map_err(|error| format!("skill {skill} could not be loaded: {error}")),
            TurnTarget::Skill { skill, .. } => load_step_skill(
                self.scope.graph_dir,
                step,
                StepSkillLoadOptions::of_run(options),
            )
            .map(|loaded| ("runner", loaded.runner.inputs))
            .map_err(|error| format!("skill {skill} could not be loaded: {error}")),
            TurnTarget::Operation { .. } | TurnTarget::Ask | TurnTarget::Load => {
                Ok(("call", BTreeMap::new()))
            }
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
        let connectors = if declared.connector_from.is_some() {
            declared
                .allowed_connectors
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>()
        } else {
            vec![allowed.requirement.as_str().to_owned()]
        };
        let mut described = Vec::new();
        for id in &connectors {
            // A pinned requirement reaches the version it pins.
            let connector = if declared.connector_from.is_none() {
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

    /// A listed child skill's tool, named for its ref.
    fn skill_tool(&self, target: TurnTarget, name: String) -> Result<TurnTool, RuntimeError> {
        let call = self
            .call_step(0, &target, &JsonObject::new())
            .map_err(|reason| RuntimeError::effect_state("describing a child skill", reason))?;
        let loaded = load_step_skill(
            self.scope.graph_dir,
            &call.step,
            StepSkillLoadOptions::of_run(&self.scope.runtime.options),
        )?;
        skill_descriptor(&name, loaded.description, &loaded.runner, target)
    }

    /// The loads this turn completed, in order.
    fn loads(&self) -> Result<Vec<TurnLoad>, RuntimeError> {
        self.recorded()?
            .iter()
            .filter(|call| call.target == TurnTarget::Load)
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
    fn loaded_offers(&self, target: &TurnTarget) -> Result<bool, RuntimeError> {
        Ok(self
            .loads()?
            .iter()
            .any(|load| load.tools.iter().any(|tool| &tool.target == target)))
    }

    /// The declaration that admits an operation call on `requirement`: the
    /// skill's own, else that of a connector this turn loaded, pinned to the
    /// manifest the load bound.
    fn declared(&self, requirement: &str) -> Result<Option<ConnectorRequirement>, RuntimeError> {
        if let Some(declared) = self.scope.agent.connectors.get(requirement) {
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
                        TurnTarget::Operation { operation, .. } => {
                            OperationName::new(operation.as_str().to_owned())
                        }
                        _ => None,
                    })
                    .collect(),
                manifest_digest: runx_contracts::connector::Digest::new(
                    manifest_digest.as_str().to_owned(),
                ),
                connector_from: None,
                allowed_connectors: Vec::new(),
            }));
        }
        Ok(None)
    }

    /// Each connector the turn already reaches, by name, at the manifest it
    /// binds: its skill's requirements, and what its loads bound.
    fn held(&self) -> Result<BTreeMap<String, String>, RuntimeError> {
        let mut held = BTreeMap::new();
        let connectors = &self.scope.agent.connectors;
        if !connectors.is_empty() {
            let scope = self.connector_scope()?;
            for (key, requirement) in connectors {
                let references = if requirement.connector_from.is_some() {
                    requirement.allowed_connectors.clone()
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
                    held.insert(
                        connector.name.as_str().to_owned(),
                        connector.manifest_digest.as_str().to_owned(),
                    );
                }
            }
        }
        for load in self.loads()? {
            for (name, digest) in load.connectors {
                held.insert(name, digest.as_str().to_owned());
            }
        }
        Ok(held)
    }

    /// The connectors the turn's skill may use, within its project.
    fn connector_scope(&self) -> Result<crate::kernel::connectors::ConnectorScope, RuntimeError> {
        crate::kernel::connectors::ConnectorScope::in_project(
            self.scope.graph_dir,
            &crate::paths::RunxPaths::resolve(
                &self.scope.runtime.options.env,
                self.scope.graph_dir,
            ),
        )
        .map_err(|error| {
            RuntimeError::effect_state("reading the skill's connectors", error.to_string())
        })
    }

    /// The skill a loaded runner's call runs: the package its load bound,
    /// read from the project again and refused if it changed since, with the
    /// packages its closure binds for its child run to execute.
    fn bound_skill(
        &self,
        step: &GraphStep,
        bound: &BoundClosure,
    ) -> Result<LoadedStepSkill, RuntimeError> {
        let options = &self.scope.runtime.options;
        let target = step.skill().ok_or_else(|| RuntimeError::StepMissingSkill {
            step_id: step.id.clone(),
        })?;
        let runner = target
            .runner
            .as_deref()
            .ok_or_else(|| RuntimeError::InvalidRunStep {
                step_id: step.id.clone(),
                reason: "a loaded skill's call names its runner".to_owned(),
            })?;
        let directory = crate::paths::RunxPaths::resolve(&options.env, self.scope.graph_dir)
            .workspace()
            .join(&target.reference);
        let changed = |reason: String| RuntimeError::AuthorityDenied {
            verb: runx_contracts::AuthorityVerb::Execute,
            step_id: step.id.clone(),
            reason: format!(
                "skill {} is not what the turn loaded: {reason}",
                target.reference
            ),
        };
        let package = std::sync::Arc::new(
            crate::load_validated_skill_package(&directory)
                .map_err(|error| changed(error.to_string()))?,
        );
        let binding = crate::skill_package::verify_loaded_execution_binding(
            std::sync::Arc::clone(&package),
            runner,
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

    /// A catalog tool's tool: its description and inputs as the catalog
    /// declares them.
    fn catalog_tool(&self, tool: &NonEmptyString) -> Result<TurnTool, RuntimeError> {
        let inspected = crate::tool_catalogs::dispatch::inspect_catalog_tool(
            tool.as_str(),
            &self.scope.runtime.options.env,
            self.scope.graph_dir,
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
            TurnTarget::Tool { tool: tool.clone() },
        ))
    }

    /// Listing a tool grants it the scopes it declares.
    fn declared_scopes(&self, tool: &str) -> Result<Vec<String>, String> {
        crate::tool_catalogs::dispatch::inspect_catalog_tool(
            tool,
            &self.scope.runtime.options.env,
            self.scope.graph_dir,
            &self.scope.runtime.options.effects,
        )
        .map(|inspected| inspected.scopes)
        .map_err(|error| format!("tool {tool} could not be resolved: {error}"))
    }
}

impl<A> Turn for OpenTurn<'_, A>
where
    A: SkillAdapter,
{
    fn invocation(&self) -> &AgentActInvocation {
        self.invocation
    }

    fn step_id(&self) -> &str {
        self.scope.step_id
    }

    fn tools(&self) -> Result<Vec<TurnTool>, RuntimeError> {
        let allowance = &self.scope.agent.allowance;
        let mut tools = Vec::new();
        for tool in &allowance.tools {
            tools.push(self.catalog_tool(tool)?);
        }
        if !allowance.operations.is_empty() {
            let connectors = self.connector_scope()?;
            for allowed in &allowance.operations {
                tools.push(self.operation_tool(&connectors, allowed)?);
            }
        }
        for skill in allowance.skill_refs() {
            tools.push(self.skill_tool(
                TurnTarget::Skill {
                    skill: skill.clone(),
                    runner: None,
                    bound: None,
                },
                format!("skill__{}", skill.as_str().trim_start_matches(['.', '/'])),
            )?);
        }
        if allowance.rules().next().is_some() {
            tools.push(offered(
                "load",
                "Load a skill or connector the turn's catalog rules admit: {skill, runner?} by ref, or {connector} by name. Each runner or operation it binds is a tool of this turn from then on. A package below the rule's trust tier waits on the person's trust."
                    .to_owned(),
                load_schema()?,
                TurnTarget::Load,
            ));
        }
        // What a load added, as it described it when it bound it: a package
        // that changed since is refused at its call, never re-read here.
        for load in self.loads()? {
            tools.extend(load.tools.into_iter().filter(|tool| {
                !matches!(&tool.target, TurnTarget::Operation { requirement, operation }
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
                TurnTarget::Ask,
            ));
        }
        distinct_names(&mut tools);
        Ok(tools)
    }

    fn calls(&self) -> Result<Vec<TurnCallResult>, RuntimeError> {
        self.recorded()?
            .iter()
            .map(|call| self.result(&Made::of(call), &call.outcome, &call.outputs))
            .collect()
    }

    fn admit(
        &mut self,
        host: &mut dyn Host,
        call_id: Option<String>,
        target: TurnTarget,
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
                    .result(&Made::of(known), &known.outcome, &known.outputs)
                    .map(|result| Admission::Settled(Box::new(result)));
            }
        }
        let index = self
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
        };
        self.admit_made(&mut AnsweringHost::new(host, None), made)
    }

    fn execute(
        &self,
        host: &mut dyn Host,
        admitted: AdmittedCall,
    ) -> Result<ExecutedCall, RuntimeError> {
        self.execute_admitted(&mut AnsweringHost::new(host, None), admitted)
    }

    fn conclude(&mut self, executed: ExecutedCall) -> Result<TurnCallResult, RuntimeError> {
        self.conclude_executed(executed)
    }

    fn settle(&mut self, host: &mut dyn Host, index: u32) -> Result<TurnCallResult, RuntimeError> {
        let Some(call) = self
            .recorded()?
            .iter()
            .find(|call| call.index == index)
            .cloned()
        else {
            return Err(RuntimeError::ContinuationRejected {
                reason: format!("the turn has no call {index}"),
            });
        };
        match call.outcome.clone() {
            CallOutcome::Child(_) => self.enter(host, call),
            // The call asks again what it waits on, unchanged.
            CallOutcome::Pending(waiting) => {
                self.run(AnsweringHost::new(host, Some(&waiting)), Made::of(&call))
            }
            // A claimed call runs again only on the host's exact
            // acknowledgement, which its new claim records.
            CallOutcome::Claimed(retry) => {
                let mut answering = AnsweringHost::new(host, Some(&retry));
                let acknowledged = answering
                    .resolve(resolution_request(&retry)?)?
                    .is_some_and(|response| response.payload == retry_acknowledgement());
                if !acknowledged {
                    return Err(RuntimeError::ContinuationRejected {
                        reason: format!(
                            "call {index} runs again only on the acknowledgement {{\"retry\": true}}"
                        ),
                    });
                }
                self.run(answering, Made::of(&call))
            }
            settled => self.result(&Made::of(&call), &settled, &call.outputs),
        }
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

/// Whether a value is given as a project artifact reference.
fn is_artifact_ref(value: &JsonValue) -> bool {
    value
        .as_object()
        .and_then(|object| object.get("schema"))
        .and_then(JsonValue::as_str)
        == Some("runx.project_artifact_ref.v1")
}

fn reason(text: String) -> NonEmptyString {
    NonEmptyString::new(text).unwrap_or_else(|| "the call gave no reason".into())
}

fn label(target: &TurnTarget) -> String {
    match target {
        TurnTarget::Tool { tool } => format!("tool {tool}"),
        TurnTarget::Operation {
            requirement,
            operation,
        } => format!("operation {requirement}/{operation}"),
        TurnTarget::Skill {
            skill,
            runner: Some(runner),
            ..
        } => format!("skill {skill}#{runner}"),
        TurnTarget::Skill { skill, .. } => format!("skill {skill}"),
        TurnTarget::Ask => "ask".to_owned(),
        TurnTarget::Load => "load".to_owned(),
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
        .any(|(_, operation)| operation.amount == AmountRule::Required);
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
    if declared.connector_from.is_some() {
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
        TurnTarget::Operation {
            requirement: allowed.requirement.clone(),
            operation: allowed.operation.clone(),
        },
    ))
}

/// A child skill's tool: its description and its runner's inputs, beside
/// the act a skill call states.
pub(super) fn skill_descriptor(
    name: &str,
    description: Option<String>,
    runner: &runx_parser::SkillRunnerDefinition,
    target: TurnTarget,
) -> Result<TurnTool, RuntimeError> {
    let describing =
        |message: String| RuntimeError::effect_state("describing a child skill", message);
    let description = description.unwrap_or_else(|| format!("Run the {} skill.", runner.name));
    let mut input_schema = runx_contracts::call_input_schema(&runner.inputs)
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

/// What a load call names: `{skill, runner?}` or `{connector}`.
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
        .find(|field| !["skill", "runner", "connector"].contains(&field.as_str()))
    {
        return Err(format!(
            "a load takes skill and runner, or connector, not {field}"
        ));
    }
    match (text("skill")?, text("runner")?, text("connector")?) {
        (Some(reference), runner, None) => Ok(LoadTarget::Skill { reference, runner }),
        (None, None, Some(name)) => Ok(LoadTarget::Connector { name }),
        _ => {
            Err("a load names a skill, with its runner if it wants one, or a connector".to_owned())
        }
    }
}

/// A loaded runner's tool name: its package, without its version, and its
/// runner.
pub(super) fn loaded_tool_name(package: &str, runner: &str) -> String {
    let name = package.split_once('@').map_or(package, |(name, _)| name);
    let name = name.strip_prefix("skills/").unwrap_or(name);
    format!("skill__{name}__{runner}")
}

/// A load's input: a skill by ref with an optional runner, or a connector by
/// name.
fn load_schema() -> Result<JsonObject, RuntimeError> {
    let text = serde_json::json!({"type": "string", "minLength": 1});
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "skill": text,
            "runner": text,
            "connector": text,
        },
        "oneOf": [
            {"required": ["skill"], "not": {"required": ["connector"]}},
            {"required": ["connector"], "not": {"anyOf": [{"required": ["skill"]}, {"required": ["runner"]}]}},
        ],
    }))
    .map_err(|source| RuntimeError::json("describing a load", source))
}

/// One call a turn offers, named for MCP.
fn offered(
    name: &str,
    description: String,
    input_schema: JsonObject,
    target: TurnTarget,
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
        | ResolutionRequest::AgentAct { id, .. }
        | ResolutionRequest::Retry { id, .. }
        | ResolutionRequest::Purchase { id, .. } => id.to_string(),
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
            TurnTarget::Ask,
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
