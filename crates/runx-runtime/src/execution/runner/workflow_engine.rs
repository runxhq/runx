// The workflow engine owns state transitions and step commits, and records each
// into the run's stream as it happens. Step behavior itself enters through
// dispatch.
use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{ExecutionEvent, JsonValue, Reference};
use runx_core::state_machine::{
    SequentialWorkflow, SequentialWorkflowEvent, SequentialWorkflowPlan, WorkflowStepState,
    create_sequential_workflow,
};
use runx_parser::{Operand, Source, StepKind, Workflow, WorkflowStep};

use super::super::run_record::{
    Answer, Hold, PendingRequest, Replayed, RunRecord, Suspension, request_digest,
};
use super::super::workflow::{LoadedStepSkill, StepSkillCache, StepSkillLoadOptions};
use super::super::workflow_index::{ExecutionWorkflowIndex, PriorRunIndex};
use super::dispatch::{LoadedStepExecutionRequest, run_step_with_loaded_skill_index};
use super::step_handlers::output_error;
use super::{Runtime, StepRun, WorkflowRun};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;
use crate::lifecycle::LifecycleEvent;

pub(super) struct WorkflowExecution {
    workflow_index: ExecutionWorkflowIndex,
    planning_cursor: usize,
    step_skill_cache: StepSkillCache,
    progress: SequentialWorkflow,
    pub(super) runs: Vec<StepRun>,
    run_positions: BTreeMap<String, usize>,
    /// The step a host suspension holds; it resumes its attempt rather than
    /// starting a new one.
    suspended_step: Option<String>,
    /// What that step waited on. A step that suspends on it again still
    /// waits; the run records nothing new.
    suspended_hold: Option<Hold>,
    /// A step claimed and never settled, and its retry: it runs again only
    /// on the person's acknowledgement, which its new claim records.
    claimed: Option<(String, PendingRequest)>,
}

/// What one step runs against, apart from its host.
struct StepPlanTarget<'a, A> {
    runtime: &'a Runtime<A>,
    workflow_dir: &'a Path,
    workflow: &'a Workflow,
    step: &'a WorkflowStep,
    plan: StepExecutionPlan<'a>,
}

/// The host a step runs against. Every answer the run's host gives during the
/// step is kept, so the step is recorded with the answers it consumed and an
/// answer the step refuses is never recorded. A step resumed on a request
/// must ask it again unchanged.
pub(super) struct AnsweringHost<'a> {
    pub(super) host: &'a mut dyn Host,
    pub(super) answers: Vec<Answer>,
    /// The request the step waited on, when it resumes.
    held: Option<&'a PendingRequest>,
}

impl<'a> AnsweringHost<'a> {
    pub(super) fn new(host: &'a mut dyn Host, held: Option<&'a PendingRequest>) -> Self {
        Self {
            host,
            answers: Vec::new(),
            held,
        }
    }
}

impl Host for AnsweringHost<'_> {
    fn report(&mut self, event: ExecutionEvent) -> Result<(), RuntimeError> {
        self.host.report(event)
    }

    fn resolve(
        &mut self,
        request: runx_contracts::ResolutionRequest,
    ) -> Result<Option<runx_contracts::ResolutionResponse>, RuntimeError> {
        let request_digest = request_digest(&request)?;
        let request_id = match &request {
            runx_contracts::ResolutionRequest::Input { id, .. }
            | runx_contracts::ResolutionRequest::Approval { id, .. }
            | runx_contracts::ResolutionRequest::Turn { id, .. }
            | runx_contracts::ResolutionRequest::Retry { id, .. }
            | runx_contracts::ResolutionRequest::Purchase { id, .. }
            | runx_contracts::ResolutionRequest::Wait { id, .. } => id.to_string(),
        };
        if self
            .held
            .is_some_and(|held| held.id == request_id && held.digest != request_digest)
        {
            return Err(RuntimeError::ContinuationRejected {
                reason: format!("{request_id} is not the request the run waited on"),
            });
        }
        // The approval gate the answer decides, when it's an approval.
        let gate = match &request {
            runx_contracts::ResolutionRequest::Approval { gate, .. } => gate.gate_type.clone(),
            _ => None,
        };
        // What a turn asked, as reuse compares it (D32).
        let input_digest = match &request {
            runx_contracts::ResolutionRequest::Turn { invocation, .. } => {
                runx_contracts::input_digest::turn_input_digest(invocation).ok()
            }
            _ => None,
        };
        let response = self.host.resolve(request)?;
        if let Some(response) = &response {
            self.answers.push(Answer {
                request_id,
                request_digest,
                value: response.payload.clone(),
                human: response.actor == runx_contracts::ResolutionResponseActor::Human,
                input_digest,
                gate,
            });
        }
        Ok(response)
    }

    fn log(&mut self, message: String) -> Result<(), RuntimeError> {
        self.host.log(message)
    }

    fn detached(&self) -> Option<crate::host::DetachedHost> {
        self.host.detached()
    }

    /// The turn's calls answer through the host itself: each call records
    /// the answers it consumed, never the step that holds the turn.
    fn drive_turn(&mut self, turn: &mut dyn super::turn::Turn) -> Result<(), RuntimeError> {
        self.host.drive_turn(turn)
    }

    fn records(&self) -> bool {
        self.host.records()
    }

    fn reads_local_only(&self) -> bool {
        self.host.reads_local_only()
    }
}

#[derive(Clone, Copy)]
pub(super) struct StepExecutionPlan<'a> {
    step_id: &'a str,
    attempt: u32,
}

struct StepExecutionContext<'a, A> {
    runtime: &'a Runtime<A>,
    workflow_dir: &'a Path,
    workflow: &'a Workflow,
    step: &'a WorkflowStep,
    host: &'a mut dyn Host,
    plan: StepExecutionPlan<'a>,
}

impl WorkflowExecution {
    pub(super) fn new(workflow: &Workflow) -> Self {
        let definitions = super::super::workflow::step_definitions(workflow);
        let progress = create_sequential_workflow(workflow.name.clone(), &definitions);
        let workflow_index = ExecutionWorkflowIndex::new(workflow, definitions);
        Self {
            workflow_index,
            planning_cursor: 0,
            step_skill_cache: StepSkillCache::default(),
            progress,
            runs: Vec::new(),
            run_positions: BTreeMap::new(),
            suspended_step: None,
            suspended_hold: None,
            claimed: None,
        }
    }

    fn apply_state_event(&mut self, event: SequentialWorkflowEvent) {
        self.workflow_index.apply_event(&mut self.progress, event);
    }

    /// Read the receipts the steps replayed from its record hold by digest,
    /// as the run seals: a resume that doesn't seal reads none of them.
    pub(super) fn read_recorded_receipts(
        &mut self,
        store: &crate::kernel::store::KernelStore,
    ) -> Result<(), RuntimeError> {
        for run in &mut self.runs {
            run.read_recorded_nested(store)
                .map_err(crate::execution::run_record::RunRecordError::from)?;
        }
        Ok(())
    }

    /// Rebuild a run's execution state from its record, applying each
    /// recorded fact as the engine applied it.
    pub(super) fn from_record(
        workflow: &Workflow,
        record: &RunRecord,
    ) -> Result<Self, RuntimeError> {
        let at = &record.created_at;
        let mut execution = Self::new(workflow);
        for fact in &record.facts {
            match fact {
                Replayed::Step(run) => {
                    execution.workflow_index.start_step(
                        &mut execution.progress,
                        &run.step_id,
                        at.clone(),
                    );
                    if run.outcome.succeeded() {
                        execution.workflow_index.succeed_step(
                            &mut execution.progress,
                            at.clone(),
                            run.admission_witness.clone(),
                            Some(run.contract.clone()),
                        );
                    } else {
                        execution.apply_state_event(SequentialWorkflowEvent::StepFailed {
                            step_id: run.step_id.clone(),
                            at: at.clone(),
                            error: output_error(run),
                        });
                    }
                    execution.remember_run(run.as_ref().clone());
                }
                Replayed::Skipped(step_ids) => {
                    for step_id in step_ids {
                        execution.apply_state_event(SequentialWorkflowEvent::StepSkipped {
                            step_id: step_id.clone(),
                            at: at.clone(),
                        });
                    }
                }
            }
        }
        if let Some(Suspension {
            step_id,
            hold: Hold::Claimed(retry),
            ..
        }) = &record.suspension
        {
            execution.claimed = Some((step_id.clone(), retry.clone()));
        }
        if let Some(suspension) = &record.suspension
            && !matches!(suspension.hold, Hold::Retry { .. } | Hold::Claimed(_))
        {
            execution.workflow_index.start_step(
                &mut execution.progress,
                &suspension.step_id,
                at.clone(),
            );
            execution.suspended_step = Some(suspension.step_id.clone());
            execution.suspended_hold = Some(suspension.hold.clone());
        }
        execution.planning_cursor =
            replay_planning_cursor(&execution.progress, execution.suspended_step.as_deref())?;
        Ok(execution)
    }

    /// The final state of a sealed run: a run sealed `completed` completed
    /// its workflow.
    pub(super) fn seal_state(&mut self, outcome: runx_contracts::RunOutcome) {
        if outcome == runx_contracts::RunOutcome::Completed {
            self.complete_workflow();
        }
    }

    pub(super) fn run<A>(
        &mut self,
        runtime: &Runtime<A>,
        workflow_dir: &Path,
        workflow: &Workflow,
        host: &mut dyn Host,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        if let Some(step_id) = self.suspended_step.clone() {
            let attempt = self
                .progress
                .steps
                .iter()
                .find(|step| step.step_id == step_id)
                .ok_or_else(|| RuntimeError::StepMissing {
                    step_id: step_id.clone(),
                })?
                .attempts;
            self.run_one_step(runtime, workflow_dir, workflow, &step_id, attempt, host)?;
        }
        loop {
            self.mark_when_skipped_steps(runtime, workflow)?;
            self.advance_planning_cursor();
            let plan = self
                .workflow_index
                .plan_transition(&self.progress, self.planning_cursor);
            if self.apply_plan(runtime, workflow_dir, workflow, host, plan)? {
                break;
            }
        }
        Ok(())
    }

    fn advance_planning_cursor(&mut self) {
        self.planning_cursor = terminal_prefix_cursor(&self.progress, self.planning_cursor);
    }

    /// Mark every step whose `when` condition the runtime has resolved to false
    /// as `Skipped`, so the planner walks past it and workflow completion treats it
    /// as terminal. Evaluated against the runs so far, so a branch is only
    /// selected out once the step it reads from has produced its output.
    fn mark_when_skipped_steps<A>(
        &mut self,
        runtime: &Runtime<A>,
        workflow: &Workflow,
    ) -> Result<(), RuntimeError> {
        let already_skipped = self
            .progress
            .steps
            .iter()
            .filter(|step| step.state == WorkflowStepState::Skipped)
            .map(|step| step.step_id.clone())
            .collect();
        let mut skipped = Vec::new();
        for step_id in when_skipped_steps(workflow, &self.runs, &already_skipped) {
            let is_pending =
                self.progress.steps.iter().any(|step| {
                    step.step_id == step_id && step.state == WorkflowStepState::Pending
                });
            if is_pending {
                self.apply_state_event(SequentialWorkflowEvent::StepSkipped {
                    step_id: step_id.clone(),
                    at: runtime.options.clock.now(),
                });
                skipped.push(step_id);
            }
        }
        runtime.options.run.record_skipped(skipped)?;
        Ok(())
    }

    pub(super) fn apply_plan<A>(
        &mut self,
        runtime: &Runtime<A>,
        workflow_dir: &Path,
        workflow: &Workflow,
        host: &mut dyn Host,
        plan: SequentialWorkflowPlan,
    ) -> Result<bool, RuntimeError>
    where
        A: SkillAdapter,
    {
        match plan {
            SequentialWorkflowPlan::RunStep {
                step_id, attempt, ..
            } => {
                self.run_one_step(runtime, workflow_dir, workflow, &step_id, attempt, host)?;
                Ok(false)
            }
            // A child run cancelled by its caller's host ends killed even
            // when no step is left: it never completes.
            SequentialWorkflowPlan::Complete if crate::interrupt::caller_cancelled() => {
                Err(RuntimeError::Cancelled {
                    step_id: self
                        .runs
                        .last()
                        .map(|run| run.step_id.clone())
                        .or_else(|| workflow.steps.last().map(|step| step.id.clone()))
                        .unwrap_or_default(),
                })
            }
            SequentialWorkflowPlan::Complete => Ok(self.complete_workflow()),
            SequentialWorkflowPlan::Blocked { step_id, reason } => {
                Err(RuntimeError::WorkflowBlocked { step_id, reason })
            }
            SequentialWorkflowPlan::Failed { step_id, reason } => {
                self.fail_workflow(step_id, reason)
            }
        }
    }

    pub(super) fn complete_workflow(&mut self) -> bool {
        self.apply_state_event(SequentialWorkflowEvent::Complete);
        true
    }

    pub(super) fn fail_workflow(
        &mut self,
        step_id: String,
        reason: String,
    ) -> Result<bool, RuntimeError> {
        self.apply_state_event(SequentialWorkflowEvent::FailWorkflow {
            error: reason.clone(),
        });
        Err(RuntimeError::WorkflowPlanningFailed { step_id, reason })
    }

    pub(super) fn run_one_step<A>(
        &mut self,
        runtime: &Runtime<A>,
        workflow_dir: &Path,
        workflow: &Workflow,
        step_id: &str,
        attempt: u32,
        host: &mut dyn Host,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        // A driver asked to stop admits no further step; the run stays where
        // it is. A pass that only records what a call did still re-enters the
        // step it is suspended at, to record it.
        let recording = host.records() && self.suspended_step.as_deref() == Some(step_id);
        if crate::interrupt::stop_requested() && !recording {
            return Err(RuntimeError::Stopped {
                run_id: runtime.options.run.run_id().to_owned(),
                step_id: format!("step {step_id}"),
            });
        }
        // A child run whose caller's host cancelled the call that ran it
        // starts no further step.
        if crate::interrupt::caller_cancelled() && !recording {
            return Err(RuntimeError::Cancelled {
                step_id: step_id.to_owned(),
            });
        }
        let plan = StepExecutionPlan { step_id, attempt };
        let step = self.find_step(workflow, plan.step_id)?;
        // The step's receipt is counted against the run's tree before
        // anything of the step runs: a step the run waits at, as a fact, and
        // a new one from what's left. A step a governed stop ends seals none,
        // and gives it back.
        let key = super::receipt_budget::step_key(runtime.options.run.run_id(), step_id, attempt);
        if self.suspended_step.as_deref() == Some(step_id) {
            runtime.options.receipts.count(key.clone(), 1);
        } else {
            runtime.options.take_receipt(key.clone(), step_id)?;
        }
        let ran = self.runs.len();
        let loaded_skill = self.cached_step_skill(runtime, workflow_dir, step);
        let result = self.run_loaded_step(
            StepExecutionContext {
                runtime,
                workflow_dir,
                workflow,
                step,
                host,
                plan,
            },
            loaded_skill,
        );
        if let Err(error) = &result
            && self.runs.len() == ran
            && !super::step_may_seal(workflow, error)
        {
            runtime.options.receipts.give_back(&key, 1);
        }
        result
    }

    fn run_loaded_step<A>(
        &mut self,
        context: StepExecutionContext<'_, A>,
        loaded_skill: Result<Option<LoadedStepSkill>, RuntimeError>,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        enforce_condition(context.step, &self.runs)?;
        enforce_guards(context.step, &self.runs)?;
        let admission_refs = verified_approval_references(
            context.runtime,
            context.workflow,
            context.step,
            &self.runs,
        )?;
        let resumed_hold = if let Some(pending) = self.suspended_step.take() {
            if pending != context.plan.step_id {
                return Err(RuntimeError::EngineInvariant {
                    context: "resuming a workflow step",
                    message: "the suspended step is not the selected step".to_owned(),
                });
            }
            self.suspended_hold.take()
        } else {
            self.record_lifecycle(
                context.host,
                LifecycleEvent::step_started(context.plan.step_id),
            )?;
            self.start_step(context.runtime, context.plan.step_id);
            None
        };
        let log = &context.runtime.options.run;
        let held = match &resumed_hold {
            Some(Hold::Request(held)) => Some(held),
            _ => None,
        };
        let mut answering = AnsweringHost::new(&mut *context.host, held);
        // A step claimed before runs again only on the person's exact
        // acknowledgement, which its new claim consumes.
        if let Some((_, retry)) = self
            .claimed
            .take_if(|(step_id, _)| step_id == context.plan.step_id)
        {
            let acknowledged = answering
                .resolve(super::turn::resolution_request(&retry)?)?
                .is_some_and(|response| response.payload == super::turn::retry_acknowledgement());
            if !acknowledged {
                return Err(RuntimeError::ContinuationRejected {
                    reason: format!(
                        "step {} runs again only on the acknowledgement {{\"retry\": true}}",
                        context.plan.step_id
                    ),
                });
            }
        }
        // The floor decides before the claim, so a step waiting on the
        // person is held on its gate, never "may have run".
        let floor = self.floor(
            context.runtime,
            context.workflow_dir,
            context.step,
            &mut answering,
        );
        let (workflow_dir, env) = (context.workflow_dir, &context.runtime.options.env);
        let reaches = floor.is_ok() && super::reaches_world(&context.step.kind);
        let reads = reaches && super::only_reads(context.step, workflow_dir, env);
        // A safe read isn't claimed: a crash that leaves it unsettled runs it
        // again with nobody asked, as it does worker compute (D33).
        let claimed = reaches && !super::safe_read(context.step, workflow_dir, env);
        let result = match floor {
            Err(error) => Err(error),
            Ok(admitted) => {
                if claimed {
                    let retry = log.address(&format!("{}/retry", context.plan.step_id));
                    log.record_claim(
                        context.plan.step_id,
                        context.plan.attempt,
                        &super::unsettled_retry(
                            retry,
                            &format!("step {}", context.plan.step_id),
                            super::resend_of(context.step, workflow_dir, env),
                        ),
                        &std::mem::take(&mut answering.answers),
                    )?;
                    if !reads {
                        crate::interrupt::note_effect();
                    }
                }
                #[cfg(feature = "crash-points")]
                if reaches {
                    crate::crash::at("after_admission", &log.address(context.plan.step_id));
                }
                let mut admission_refs = admission_refs;
                admission_refs.extend(admitted.reference()?);
                let held_by_effects = admitted == super::floor::Admitted::Held;
                let target = StepPlanTarget {
                    runtime: context.runtime,
                    workflow_dir: context.workflow_dir,
                    workflow: context.workflow,
                    step: context.step,
                    plan: context.plan,
                };
                // A read runs under the stops of the calls whose child run
                // this is, so their cancel ends it, and it then ends the run
                // where it stands.
                if reads {
                    let read = crate::interrupt::while_reading(None, || {
                        self.execute_step_plan(
                            target,
                            &mut answering,
                            loaded_skill,
                            admission_refs,
                            held_by_effects,
                        )
                    });
                    if crate::interrupt::caller_cancelled() {
                        return Err(RuntimeError::Cancelled {
                            step_id: context.plan.step_id.to_owned(),
                        });
                    }
                    read
                } else {
                    self.execute_step_plan(
                        target,
                        &mut answering,
                        loaded_skill,
                        admission_refs,
                        held_by_effects,
                    )
                }
            }
        };
        let answers = answering.answers;
        let run = match result {
            Err(RuntimeError::ResolutionPending {
                step_id,
                reason,
                request,
                child,
            }) => {
                // What the step appended while it ran, a claim or a script's
                // call, may be the run's latest event now, so a hold it still
                // waits on is recorded again after it.
                let still_waiting = log.stands_waiting()?
                    && match (&child, &resumed_hold) {
                        (Some(child), Some(Hold::Child(held))) => child == held,
                        (None, Some(Hold::Request(held))) => {
                            request_digest(&request)? == held.digest
                        }
                        _ => false,
                    };
                match &child {
                    _ if still_waiting => {}
                    Some(child) => {
                        log.record_child_hold(context.plan.step_id, context.plan.attempt, child)?;
                    }
                    None => {
                        log.record_request(context.plan.step_id, context.plan.attempt, &request)?;
                    }
                }
                self.suspended_step = Some(context.plan.step_id.to_owned());
                return Err(RuntimeError::ResolutionPending {
                    step_id,
                    reason,
                    request,
                    child,
                });
            }
            result => result?,
        };
        #[cfg(feature = "crash-points")]
        crate::crash::at("after_execution", &log.address(context.plan.step_id));
        self.commit_step_run(context.runtime, context.host, context.plan, run, &answers)
    }

    /// The floor's decision for a tool or operation step, under the run's
    /// `effects`, over the inputs it would run with. Its failure is the
    /// step's, scoped as the step's execution is, so a step whose effect
    /// can't even be described seals failed there.
    fn floor<A>(
        &self,
        runtime: &Runtime<A>,
        workflow_dir: &Path,
        step: &WorkflowStep,
        host: &mut dyn Host,
    ) -> Result<super::floor::Admitted, RuntimeError>
    where
        A: SkillAdapter,
    {
        let effects =
            crate::kernel::authority::step_effects(runtime.options.run_authority.as_ref(), step);
        if effects == runx_contracts::Effects::Allow
            || !matches!(step.kind, StepKind::Tool(_) | StepKind::Operation(_))
        {
            return Ok(super::floor::Admitted::Free);
        }
        super::super::workflow::materialize_step_invocation_inputs_with_index(
            step,
            &PriorRunIndex::from_positions(&self.runs, &self.run_positions),
        )
        .and_then(|inputs| super::floor::hold(runtime, workflow_dir, step, &inputs, host))
        .map_err(|error| error.at_workflow_step(&step.id))
    }

    fn execute_step_plan<A>(
        &mut self,
        context: StepPlanTarget<'_, A>,
        host: &mut dyn Host,
        loaded_skill: Result<Option<LoadedStepSkill>, RuntimeError>,
        admission_refs: Vec<Reference>,
        held_by_effects: bool,
    ) -> Result<StepRun, RuntimeError>
    where
        A: SkillAdapter,
    {
        let prior_run_index = PriorRunIndex::from_positions(&self.runs, &self.run_positions);
        run_step_with_loaded_skill_index(
            LoadedStepExecutionRequest {
                runtime: context.runtime,
                workflow_dir: context.workflow_dir,
                workflow_name: &context.workflow.name,
                step: context.step,
                attempt: context.plan.attempt,
                loaded_skill: loaded_skill?,
                admission_refs,
                held_by_effects,
                host,
            },
            &prior_run_index,
        )
        .map_err(|error| error.at_workflow_step(&context.step.id))
    }

    fn commit_step_run<A>(
        &mut self,
        runtime: &Runtime<A>,
        host: &mut dyn Host,
        plan: StepExecutionPlan<'_>,
        run: StepRun,
        answers: &[Answer],
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        if run.outcome.succeeded() {
            self.succeed_step(runtime, &run);
            self.push_run(runtime, run, answers)?;
            self.record_lifecycle(host, LifecycleEvent::step_completed(plan.step_id))
        } else {
            self.fail_step(runtime, plan.step_id, &run);
            host.log(format!("step {} failed", plan.step_id))?;
            self.record_lifecycle(host, LifecycleEvent::step_failed(plan.step_id))?;
            // Every invocation kind owns an honest diagnostic variant. A
            // failure message no longer depends on fabricated process fields.
            let message = run
                .outcome
                .failure_message()
                .unwrap_or_else(|| "step failed with no diagnostic output".to_owned());
            // The failed run is recorded even on terminal failure so the run
            // list agrees with the run record's step failure; a failed attempt
            // must never be silently absent from the execution record.
            let outcome = run.receipt.seal.outcome;
            let reason = run.receipt.seal.reason;
            let refusal = run.refusal.clone().map(Box::new);
            self.push_run(runtime, run, answers)?;
            // A child run killed by its caller's cancel ends this run where it
            // stands, by the same rule.
            if outcome == runx_contracts::RunOutcome::Killed
                && reason == Some(runx_contracts::SealReason::Cancelled)
            {
                return Err(RuntimeError::Cancelled {
                    step_id: plan.step_id.to_owned(),
                });
            }
            if super::is_governed_step_closure(&outcome) {
                return Err(RuntimeError::StepClosed {
                    step_id: plan.step_id.to_owned(),
                    outcome,
                    reason,
                    summary: message,
                    refusal,
                });
            }
            Err(RuntimeError::SkillFailed {
                skill_name: plan.step_id.to_owned(),
                message,
            })
        }
    }

    /// Record a step's run, with the answers it consumed, into the run's
    /// stream, then hold it in memory.
    fn push_run<A>(
        &mut self,
        runtime: &Runtime<A>,
        run: StepRun,
        answers: &[Answer],
    ) -> Result<(), RuntimeError> {
        runtime.options.run.record_step(&run, answers)?;
        self.remember_run(run);
        Ok(())
    }

    fn remember_run(&mut self, run: StepRun) {
        let index = self.runs.len();
        self.run_positions.insert(run.step_id.clone(), index);
        self.runs.push(run);
    }

    pub(super) fn start_step<A>(&mut self, runtime: &Runtime<A>, step_id: &str) {
        self.workflow_index
            .start_step(&mut self.progress, step_id, runtime.options.clock.now());
    }

    pub(super) fn succeed_step<A>(&mut self, runtime: &Runtime<A>, run: &StepRun) {
        self.workflow_index.succeed_step(
            &mut self.progress,
            runtime.options.clock.now(),
            run.admission_witness.clone(),
            Some(run.contract.clone()),
        );
    }

    pub(super) fn fail_step<A>(&mut self, runtime: &Runtime<A>, step_id: &str, run: &StepRun) {
        self.apply_state_event(SequentialWorkflowEvent::StepFailed {
            step_id: step_id.to_owned(),
            at: runtime.options.clock.now(),
            error: output_error(run),
        });
    }

    /// Fail the workflow in place at `step_id`. A step whose dispatch faulted
    /// has no run yet; `fault_run` records it so the sealed trace holds it.
    pub(super) fn fail_workflow_at<A>(
        &mut self,
        runtime: &Runtime<A>,
        host: &mut dyn Host,
        step_id: &str,
        fault_run: Option<StepRun>,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        if let Some(run) = fault_run {
            self.fail_step(runtime, step_id, &run);
            self.push_run(runtime, run, &[])?;
            self.record_lifecycle(host, LifecycleEvent::step_failed(step_id))?;
        }
        self.apply_state_event(SequentialWorkflowEvent::FailWorkflow {
            error: format!("step {step_id} failed"),
        });
        Ok(())
    }

    pub(super) fn has_run(&self, step_id: &str) -> bool {
        self.run_positions.contains_key(step_id)
    }

    /// The reason `step_id`'s latest run sealed with, when it names one.
    pub(super) fn sealed_reason(&self, step_id: &str) -> Option<runx_contracts::SealReason> {
        self.run_positions
            .get(step_id)
            .and_then(|index| self.runs.get(*index))
            .and_then(|run| run.receipt.seal.reason)
    }

    /// Whether `step_id`'s latest run is a failure it already sealed.
    pub(super) fn has_failed_run(&self, step_id: &str) -> bool {
        self.run_positions
            .get(step_id)
            .and_then(|index| self.runs.get(*index))
            .is_some_and(|run| !run.outcome.succeeded())
    }

    pub(super) fn attempt(&self, step_id: &str) -> u32 {
        self.progress
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .map_or(1, |step| step.attempts.max(1))
    }

    pub(super) fn record_terminal_step_failure<A>(
        &mut self,
        runtime: &Runtime<A>,
        host: &mut dyn Host,
        step_id: &str,
        run: StepRun,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        if self.suspended_step.as_deref() == Some(step_id) {
            self.suspended_step = None;
        } else {
            self.record_lifecycle(host, LifecycleEvent::step_started(step_id))?;
            self.start_step(runtime, step_id);
        }
        self.fail_step(runtime, step_id, &run);
        self.push_run(runtime, run, &[])?;
        self.apply_state_event(SequentialWorkflowEvent::FailWorkflow {
            error: format!("step {step_id} failed"),
        });
        self.record_lifecycle(host, LifecycleEvent::step_failed(step_id))
    }

    pub(super) fn record(
        &mut self,
        host: &mut dyn Host,
        event: ExecutionEvent,
    ) -> Result<(), RuntimeError> {
        host.report(event)
    }

    pub(super) fn record_lifecycle(
        &mut self,
        host: &mut dyn Host,
        event: LifecycleEvent,
    ) -> Result<(), RuntimeError> {
        self.record(host, event.into_execution_event())
    }

    pub(super) fn finish(
        self,
        workflow: Workflow,
        receipt: runx_contracts::Receipt,
    ) -> WorkflowRun {
        WorkflowRun {
            workflow,
            progress: self.progress,
            steps: self.runs,
            receipt,
            stop: None,
            refusal: None,
        }
    }

    /// A run a governed stop sealed: it keeps why it stopped.
    pub(super) fn finish_stopped(
        self,
        workflow: Workflow,
        receipt: runx_contracts::Receipt,
        stop: String,
    ) -> WorkflowRun {
        WorkflowRun {
            stop: Some(stop),
            ..self.finish(workflow, receipt)
        }
    }

    /// The attempt a terminal failure of `step_id` is recorded under: the
    /// suspended attempt it held, or the next one.
    pub(super) fn failure_attempt(&self, step_id: &str) -> u32 {
        self.progress
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .map_or(1, |step| {
                if step.state == WorkflowStepState::Running {
                    step.attempts
                } else {
                    step.attempts.saturating_add(1).max(1)
                }
            })
    }

    fn cached_step_skill(
        &mut self,
        runtime: &Runtime<impl SkillAdapter>,
        workflow_dir: &Path,
        step: &WorkflowStep,
    ) -> Result<Option<LoadedStepSkill>, RuntimeError> {
        if step.skill().is_none() {
            return Ok(None);
        }
        self.step_skill_cache
            .load(
                workflow_dir,
                step,
                StepSkillLoadOptions::of_run(runtime.options()),
            )
            .map(Some)
    }

    fn find_step<'a>(
        &self,
        workflow: &'a Workflow,
        step_id: &str,
    ) -> Result<&'a WorkflowStep, RuntimeError> {
        // `workflow_index` is built from exactly this `workflow` (see `WorkflowExecution::new`
        // / `from_record`), which is immutable for the run, so the index position
        // map is always in sync with `workflow.steps`. The index's `StepMissing` is the
        // authoritative answer for a genuinely-missing step; a linear re-scan over the
        // same `workflow.steps` could never find a step the index legitimately missed, it
        // would only silently paper over an index/workflow desync. Return the index result
        // directly so such a desync surfaces instead of being absorbed by an O(n) scan.
        self.workflow_index.find_step(workflow, step_id)
    }
}

fn replay_planning_cursor(
    state: &SequentialWorkflow,
    suspended_step_id: Option<&str>,
) -> Result<usize, RuntimeError> {
    let mut found_pending = false;
    for step in state
        .steps
        .iter()
        .filter(|step| step.state == WorkflowStepState::Running)
    {
        if found_pending || suspended_step_id != Some(step.step_id.as_str()) || step.attempts == 0 {
            return Err(RuntimeError::WorkflowPlanningFailed {
                step_id: step.step_id.clone(),
                reason: "the run record holds a running step without a matching suspension"
                    .to_owned(),
            });
        }
        found_pending = true;
    }
    if let Some(step_id) = suspended_step_id
        && !found_pending
    {
        return Err(RuntimeError::WorkflowPlanningFailed {
            step_id: step_id.to_owned(),
            reason: "the recorded suspension does not identify a running step".to_owned(),
        });
    }
    Ok(terminal_prefix_cursor(state, 0))
}

fn terminal_prefix_cursor(state: &SequentialWorkflow, start: usize) -> usize {
    let mut cursor = start.min(state.steps.len());
    while state.steps.get(cursor).is_some_and(|step| {
        matches!(
            step.state,
            WorkflowStepState::Succeeded | WorkflowStepState::Skipped
        )
    }) {
        cursor += 1;
    }
    cursor
}

/// A step runs only on a condition that holds. One that reads nothing (a
/// field the earlier step's result does not have) never lets the step run.
fn enforce_condition(step: &WorkflowStep, runs: &[StepRun]) -> Result<(), RuntimeError> {
    let Some(when) = &step.when else {
        return Ok(());
    };
    let shown = operand_text(&when.value);
    let holds = condition_met(when, runs).ok_or_else(|| RuntimeError::StepFailed {
        step_id: step.id.clone(),
        reason: format!("condition '{shown}' resolves to nothing"),
    })?;
    if holds {
        Ok(())
    } else {
        Err(RuntimeError::EngineInvariant {
            context: "running a workflow step",
            message: format!(
                "step {} was selected although its condition '{shown}' does not hold",
                step.id
            ),
        })
    }
}

pub(super) fn enforce_guards(step: &WorkflowStep, runs: &[StepRun]) -> Result<(), RuntimeError> {
    for gate in &step.guard {
        let shown = operand_text(&gate.value);
        let Some(value) = operand_value(&gate.value, runs) else {
            return Err(RuntimeError::WorkflowBlocked {
                step_id: step.id.clone(),
                reason: format!("guard '{shown}' is unresolved"),
            });
        };
        if let Some(expected) = &gate.equals
            && value != expected
        {
            if expected == &JsonValue::Bool(true)
                && value == &JsonValue::Bool(false)
                && approval_decision_of(&gate.value).is_some()
            {
                return Err(crate::RuntimeError::AuthorityDenied {
                    verb: runx_contracts::AuthorityVerb::Execute,
                    step_id: step.id.clone(),
                    reason: format!("approval guard '{shown}' was denied"),
                });
            }
            return Err(RuntimeError::WorkflowBlocked {
                step_id: step.id.clone(),
                reason: format!("guard '{shown}' expected {}", display_json(expected)),
            });
        }
        if let Some(disallowed) = &gate.not_equals
            && value == disallowed
        {
            return Err(RuntimeError::WorkflowBlocked {
                step_id: step.id.clone(),
                reason: format!(
                    "guard '{shown}' must not equal {}",
                    display_json(disallowed)
                ),
            });
        }
        if gate.equals.is_none() && gate.not_equals.is_none() {
            return Err(RuntimeError::WorkflowBlocked {
                step_id: step.id.clone(),
                reason: format!("guard '{shown}' has no comparison"),
            });
        }
    }
    Ok(())
}

/// The step whose recorded approval decision `operand` reads, when it reads
/// one: `$steps.<ask>.approval_decision.approved`.
fn approval_decision_of(operand: &Operand) -> Option<&str> {
    match operand {
        Operand::Reference(Source::Step { step, path })
            if path == &["approval_decision", "approved"] =>
        {
            Some(step)
        }
        _ => None,
    }
}

fn verified_approval_references<A>(
    runtime: &Runtime<A>,
    workflow: &Workflow,
    step: &WorkflowStep,
    runs: &[StepRun],
) -> Result<Vec<Reference>, RuntimeError>
where
    A: SkillAdapter,
{
    let mut references = Vec::new();
    for guard in step
        .guard
        .iter()
        .filter(|guard| guard.equals == Some(JsonValue::Bool(true)) && guard.not_equals.is_none())
    {
        let Some(approval_step_id) = approval_decision_of(&guard.value) else {
            continue;
        };
        let Some(approval_step) = workflow
            .steps
            .iter()
            .find(|candidate| candidate.id == approval_step_id)
        else {
            continue;
        };
        if !matches!(approval_step.kind, StepKind::Ask(_)) {
            continue;
        }
        let Some(approval_run) = runs
            .iter()
            .rev()
            .find(|run| run.step_id == approval_step_id)
        else {
            continue;
        };
        if approval_run.skill != "ask"
            || !approval_run.outcome.succeeded()
            || approval_run.receipt.seal.outcome != runx_contracts::RunOutcome::Completed
            || operand_value(&guard.value, runs) != Some(&JsonValue::Bool(true))
        {
            continue;
        }
        crate::receipts::tree::validate_runtime_receipt_tree(
            &approval_run.receipt,
            std::iter::empty::<runx_contracts::Receipt>(),
            runx_receipts::ReceiptTreeConfig::default(),
            runtime.options.signature_policy(),
        )
        .map_err(|verification| RuntimeError::ReceiptInvalid {
            message: format!(
                "policy approval receipt {} failed verification: {:?}",
                approval_run.receipt.id, verification.findings
            ),
        })?;
        crate::receipts::seal::validate_step_receipt_claim(
            &approval_run.receipt,
            &approval_run.contract,
        )?;
        references.push(crate::receipts::seal::child_receipt_reference(
            &approval_run.receipt,
        ));
    }
    references.sort_by(|left, right| left.uri.cmp(&right.uri));
    references.dedup();
    Ok(references)
}

/// The value an operand holds now: a literal, or a field of a step's
/// declared result once that step has run. Guards and conditions read the
/// same declared result as step values do; runtime diagnostics live on
/// `StepOutcome`, never in it, so there is no second control-flow surface.
/// Whether `when` holds over `runs`. An input the run wasn't given is
/// absent, which equals nothing, so `equals` doesn't hold and `not_equals`
/// does. A step's output it reads that isn't there decides nothing.
fn condition_met(when: &runx_parser::WorkflowWhen, runs: &[StepRun]) -> Option<bool> {
    let holds = |value: Option<&JsonValue>| match (&when.equals, &when.not_equals) {
        (Some(expected), _) => value == Some(expected),
        (_, Some(disallowed)) => value != Some(disallowed),
        _ => true,
    };
    match operand_value(&when.value, runs) {
        Some(value) => Some(holds(Some(value))),
        None if matches!(when.value, Operand::Reference(Source::Input { .. })) => Some(holds(None)),
        None => None,
    }
}

pub(super) fn operand_value<'a>(
    operand: &'a Operand,
    runs: &'a [StepRun],
) -> Option<&'a JsonValue> {
    match operand {
        Operand::Literal(value) => Some(value),
        Operand::Reference(Source::Step { step, path }) => {
            let run = runs.iter().rev().find(|run| run.step_id == *step)?;
            let (first, rest) = path.split_first()?;
            crate::execution::workflow::value_at(run.output(first)?, rest)
        }
        // The run's input and workspace id are bound before the workflow runs,
        // so an input field still unbound is absent; no condition or guard
        // reads the invoker's act or the run's id.
        Operand::Reference(
            Source::Input { .. } | Source::Act { .. } | Source::RunId | Source::WorkspaceId,
        ) => None,
    }
}

fn operand_text(operand: &Operand) -> String {
    match operand {
        Operand::Literal(value) => display_json(value),
        Operand::Reference(source) => source.reference(),
    }
}

pub(super) fn display_json(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unprintable>".to_owned())
}

/// Resolve which steps a `when` condition selects out, given the runs so far.
/// A pending predicate source leaves the branch pending. A step selected out
/// makes every step that reads it (its condition, its values or its act)
/// unreachable, so selection propagates transitively instead of leaving the
/// workflow blocked.
pub(super) fn when_skipped_steps(
    workflow: &Workflow,
    runs: &[StepRun],
    already_skipped: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let mut skipped = already_skipped.clone();
    loop {
        let previous_len = skipped.len();
        for step in &workflow.steps {
            let condition_source = step.when.as_ref().and_then(|when| when.value.step());
            if step
                .needs()
                .into_iter()
                .chain(condition_source)
                .any(|source| skipped.contains(source))
            {
                skipped.insert(step.id.clone());
                continue;
            }
            let Some(when) = &step.when else {
                continue;
            };
            let Some(satisfied) = condition_met(when, runs) else {
                continue;
            };
            if !satisfied {
                skipped.insert(step.id.clone());
            }
        }
        if skipped.len() == previous_len {
            return skipped;
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::collections::BTreeSet;

    use runx_contracts::{
        ExecutionEvent, JsonValue, ResolutionRequest, ResolutionResponse, ResolutionResponseActor,
    };

    use super::AnsweringHost;
    use crate::{Host, RuntimeError};

    /// An agent that answers every turn alike.
    struct Agent;

    impl Host for Agent {
        fn report(&mut self, _event: ExecutionEvent) -> Result<(), RuntimeError> {
            Ok(())
        }

        fn resolve(
            &mut self,
            _request: ResolutionRequest,
        ) -> Result<Option<ResolutionResponse>, RuntimeError> {
            Ok(Some(ResolutionResponse {
                actor: ResolutionResponseActor::Agent,
                payload: JsonValue::Object(runx_contracts::JsonObject::new()),
            }))
        }

        fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    /// A turn of run `run` at `address`, asked with `offer`.
    fn turn(run: &str, address: &str, offer: &str) -> ResolutionRequest {
        serde_json::from_value(serde_json::json!({
            "kind": "turn",
            "id": address,
            "invocation": {
                "id": address,
                "task": "draft",
                "allowance": {},
                "envelope": {
                    "run_id": run,
                    "step_id": "draft",
                    "skill": "cold-outreach",
                    "instructions": "Write it.",
                    "instructions_digest": "sha256:1",
                    "inputs": {"offer": offer},
                    "requirements": {"execution_boundary": {"kind": "remote_provider"}},
                    "current_context": [],
                    "historical_context": [],
                    "provenance": [],
                    "trust_boundary": "test",
                },
            },
        }))
        .expect("a turn request")
    }

    /// An answer to an approval is kept with the type of the gate it
    /// decided, so a person's approval to spend on a read is told from any
    /// other answer.
    #[test]
    fn an_approvals_answer_keeps_its_gates_type() {
        let mut agent = Agent;
        let mut host = AnsweringHost::new(&mut agent, None);
        let approval: ResolutionRequest = serde_json::from_value(serde_json::json!({
            "kind": "approval",
            "id": "read",
            "gate": {"id": "read", "reason": "Spend on this read.", "type": "provider_usage"},
        }))
        .expect("an approval request");
        host.resolve(approval).expect("answered");
        assert_eq!(host.answers[0].gate.as_deref(), Some("provider_usage"));
    }

    /// A turn's answer is kept with what the turn asked: the same request
    /// from another run, at another address, asked the same, and another
    /// offer asked something else.
    #[test]
    fn a_turns_answer_keeps_what_the_turn_asked() {
        let mut agent = Agent;
        let mut host = AnsweringHost::new(&mut agent, None);
        for request in [
            turn("run-a", "prepare/3#draft", "A review."),
            turn("run-b", "prepare/7#draft", "A review."),
            turn("run-a", "prepare/3#draft", "A call."),
        ] {
            host.resolve(request).expect("answered");
        }
        let asked = host
            .answers
            .iter()
            .map(|answer| answer.input_digest.clone().expect("a turn's input digest"))
            .collect::<Vec<_>>();
        assert_eq!(asked[0], asked[1]);
        assert_ne!(asked[0], asked[2]);
    }

    use super::{replay_planning_cursor, when_skipped_steps};
    use runx_core::state_machine::{
        SequentialWorkflowStepDefinition, WorkflowStepState, create_sequential_workflow,
    };

    fn replay_state(
        statuses: &[WorkflowStepState],
    ) -> runx_core::state_machine::SequentialWorkflow {
        let definitions = statuses
            .iter()
            .enumerate()
            .map(|(index, _)| SequentialWorkflowStepDefinition {
                id: format!("step_{index}"),
                context_from: None,
            })
            .collect::<Vec<_>>();
        let mut state = create_sequential_workflow("graph", &definitions);
        for (step, status) in state.steps.iter_mut().zip(statuses) {
            step.state = status.clone();
        }
        state
    }

    #[test]
    fn replay_cursor_starts_at_the_first_non_terminal_step() {
        let state = replay_state(&[
            WorkflowStepState::Succeeded,
            WorkflowStepState::Skipped,
            WorkflowStepState::Failed,
            WorkflowStepState::Pending,
        ]);

        assert_eq!(
            replay_planning_cursor(&state, None).expect("valid record"),
            2
        );
    }

    #[test]
    fn replay_cursor_rejects_running_state_anywhere() {
        let state = replay_state(&[
            WorkflowStepState::Succeeded,
            WorkflowStepState::Pending,
            WorkflowStepState::Running,
        ]);

        let error = replay_planning_cursor(&state, None).expect_err("a running step must fail");
        assert!(error.to_string().contains("holds a running step"));
    }

    #[test]
    fn skipped_branch_predicates_propagate_to_unreachable_descendants() {
        let workflow = crate::execution::workflow::testing::method_workflow(
            "conditional-propagation",
            r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: source
        agent: source
        outputs: {decision: {type: string}}
      - id: inspect
        when: {value: $steps.source.decision, equals: ready}
        agent: inspect
        outputs: {decision: {type: string}}
      - id: reject
        when: {value: $steps.inspect.decision, equals: reject}
        agent: reject
        outputs: {decision: {type: string}}
"#,
        )
        .expect("fixture method should validate");
        let skipped = when_skipped_steps(&workflow, &[], &BTreeSet::from(["inspect".to_owned()]));

        assert!(skipped.contains("reject"));
    }

    #[test]
    fn a_step_that_reads_a_skipped_step_is_skipped_whatever_it_reads_it_through() {
        let workflow = crate::execution::workflow::testing::method_workflow(
            "read-propagation",
            r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: inspect
        when: {value: $input.inspect, equals: true}
        agent: inspect
        outputs: {report: {type: object}}
      - id: publish
        when: {value: $input.publish, equals: true}
        agent: publish
        inputs: {report: $steps.inspect.report}
"#,
        )
        .expect("fixture method should validate");
        let skipped = when_skipped_steps(&workflow, &[], &BTreeSet::from(["inspect".to_owned()]));

        assert!(skipped.contains("publish"));
    }

    #[test]
    fn a_condition_that_reads_nothing_never_lets_its_step_run() {
        let workflow = crate::execution::workflow::testing::method_workflow(
            "misspelled-condition",
            r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: review
        agent: review
        outputs: {verdict: {type: string}}
      - id: send
        when: {value: $steps.review.verdcit, equals: approved}
        agent: send
"#,
        )
        .expect("fixture method should validate");
        let send = &workflow.steps[1];
        let error = super::enforce_condition(send, &[]).expect_err("unresolved");
        assert!(error.to_string().contains("resolves to nothing"), "{error}");
    }

    #[test]
    fn a_branch_on_the_run_input_is_decided_before_any_step_runs() {
        let workflow = crate::execution::workflow::testing::method_workflow(
            "input-branch",
            r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: apply
        when: {value: $input.mode, equals: apply}
        agent: apply
      - id: preview
        when: {value: $input.mode, not_equals: apply}
        agent: preview
"#,
        )
        .expect("fixture method should validate");
        let inputs = runx_contracts::JsonObject::from([(
            "mode".to_owned(),
            runx_contracts::JsonValue::String("preview".to_owned()),
        )]);
        let workflow =
            crate::execution::workflow::bind_run_inputs(workflow, &inputs, None, "run", None)
                .expect("the run input binds");
        let skipped = when_skipped_steps(&workflow, &[], &BTreeSet::new());

        assert_eq!(skipped, BTreeSet::from(["apply".to_owned()]));
    }

    /// An input the run wasn't given is absent, which equals nothing: a step
    /// that runs only when it equals a value is skipped, one that runs when
    /// it doesn't is run, and neither fails the run (plans 390a69f). A guard
    /// still requires its value.
    #[test]
    fn an_absent_input_equals_nothing_and_passes_no_guard() {
        let workflow = crate::execution::workflow::testing::method_workflow(
            "absent-input",
            r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: preview
        when: {value: $input.mode, not_equals: apply}
        agent: preview
      - id: apply
        when: {value: $input.mode, equals: apply}
        agent: apply
      - id: send
        guard:
          - {value: $input.approved, not_equals: false}
        agent: send
"#,
        )
        .expect("fixture method should validate");
        let workflow = crate::execution::workflow::bind_run_inputs(
            workflow,
            &runx_contracts::JsonObject::new(),
            None,
            "run",
            None,
        )
        .expect("the run input binds");

        assert_eq!(
            when_skipped_steps(&workflow, &[], &BTreeSet::new()),
            BTreeSet::from(["apply".to_owned()]),
            "an absent input equals nothing"
        );
        super::enforce_condition(&workflow.steps[0], &[]).expect("it doesn't equal apply");
        let error = super::enforce_guards(&workflow.steps[2], &[]).expect_err("absent");
        assert!(
            error
                .to_string()
                .contains("guard '$input.approved' is unresolved"),
            "{error}"
        );
    }

    /// A turn whose context reads a step that was skipped runs with none
    /// of that step's entries, rather than being skipped with it.
    #[test]
    fn a_turn_runs_without_the_entries_of_a_skipped_step() {
        let workflow = crate::execution::workflow::testing::method_workflow(
            "skipped-context",
            r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: recall
        when: {value: $input.mode, equals: recall}
        tool: data.digest
        inputs: {value: $input.levels}
      - id: answer
        agent: answer
        context:
          entries: [$steps.recall.entries]
"#,
        )
        .expect("fixture method should validate");
        let workflow = crate::execution::workflow::bind_run_inputs(
            workflow,
            &runx_contracts::JsonObject::new(),
            None,
            "run",
            None,
        )
        .expect("the run input binds");

        assert_eq!(
            when_skipped_steps(&workflow, &[], &BTreeSet::new()),
            BTreeSet::from(["recall".to_owned()])
        );
        let positions = std::collections::BTreeMap::new();
        let entries = crate::execution::workflow::materialize_step_context_entries_with_index(
            &workflow.steps[1],
            &super::PriorRunIndex::from_positions(&[], &positions),
        )
        .expect("a skipped step gives no entries");
        assert!(entries.is_empty());
    }
}
