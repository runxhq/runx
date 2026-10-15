// The graph engine owns state transitions and step commits, and records each
// into the run's stream as it happens. Step behavior itself enters through
// dispatch.
use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{ExecutionEvent, JsonValue, Reference};
use runx_core::state_machine::{
    GraphStepStatus, SequentialGraphEvent, SequentialGraphPlan, SequentialGraphState,
    create_sequential_graph_state,
};
use runx_parser::{ExecutionGraph, GraphStep, Operand, Source, StepKind};

use super::super::graph::{LoadedStepSkill, StepSkillCache, StepSkillLoadOptions};
use super::super::graph_index::{ExecutionGraphIndex, PriorRunIndex};
use super::super::run_record::{
    Answer, Hold, PendingRequest, Replayed, RunRecord, Suspension, request_digest,
};
use super::dispatch::{LoadedStepExecutionRequest, run_step_with_loaded_skill_index};
use super::step_handlers::output_error;
use super::{GraphRun, Runtime, StepRun};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::host::Host;
use crate::lifecycle::LifecycleEvent;

pub(super) struct GraphExecution {
    graph_index: ExecutionGraphIndex,
    planning_cursor: usize,
    step_skill_cache: StepSkillCache,
    state: SequentialGraphState,
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
    graph_dir: &'a Path,
    graph: &'a ExecutionGraph,
    step: &'a GraphStep,
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
            | runx_contracts::ResolutionRequest::AgentAct { id, .. }
            | runx_contracts::ResolutionRequest::Retry { id, .. } => id.to_string(),
        };
        if self
            .held
            .is_some_and(|held| held.id == request_id && held.digest != request_digest)
        {
            return Err(RuntimeError::ContinuationRejected {
                reason: format!("{request_id} is not the request the run waited on"),
            });
        }
        let response = self.host.resolve(request)?;
        if let Some(response) = &response {
            self.answers.push(Answer {
                request_id,
                request_digest,
                value: response.payload.clone(),
                human: response.actor == runx_contracts::ResolutionResponseActor::Human,
            });
        }
        Ok(response)
    }

    fn log(&mut self, message: String) -> Result<(), RuntimeError> {
        self.host.log(message)
    }

    /// The turn's calls answer through the host itself: each call records
    /// the answers it consumed, never the step that holds the turn.
    fn drive_turn(&mut self, turn: &mut dyn super::turn::Turn) -> Result<(), RuntimeError> {
        self.host.drive_turn(turn)
    }
}

#[derive(Clone, Copy)]
pub(super) struct StepExecutionPlan<'a> {
    step_id: &'a str,
    attempt: u32,
}

struct StepExecutionContext<'a, A> {
    runtime: &'a Runtime<A>,
    graph_dir: &'a Path,
    graph: &'a ExecutionGraph,
    step: &'a GraphStep,
    host: &'a mut dyn Host,
    plan: StepExecutionPlan<'a>,
}

impl GraphExecution {
    pub(super) fn new(graph: &ExecutionGraph) -> Self {
        let definitions = super::super::graph::step_definitions(graph);
        let state = create_sequential_graph_state(graph.name.clone(), &definitions);
        let graph_index = ExecutionGraphIndex::new(graph, definitions);
        Self {
            graph_index,
            planning_cursor: 0,
            step_skill_cache: StepSkillCache::default(),
            state,
            runs: Vec::new(),
            run_positions: BTreeMap::new(),
            suspended_step: None,
            suspended_hold: None,
            claimed: None,
        }
    }

    fn apply_state_event(&mut self, event: SequentialGraphEvent) {
        self.graph_index.apply_event(&mut self.state, event);
    }

    /// Rebuild a run's execution state from its record, applying each
    /// recorded fact as the engine applied it.
    pub(super) fn from_record(
        graph: &ExecutionGraph,
        record: &RunRecord,
    ) -> Result<Self, RuntimeError> {
        let at = &record.created_at;
        let mut execution = Self::new(graph);
        for fact in &record.facts {
            match fact {
                Replayed::Step(run) => {
                    execution.graph_index.start_step(
                        &mut execution.state,
                        &run.step_id,
                        at.clone(),
                    );
                    if run.outcome.succeeded() {
                        execution.graph_index.succeed_step(
                            &mut execution.state,
                            at.clone(),
                            run.admission_witness.clone(),
                            Some(run.contract.clone()),
                        );
                    } else {
                        execution.apply_state_event(SequentialGraphEvent::StepFailed {
                            step_id: run.step_id.clone(),
                            at: at.clone(),
                            error: output_error(run),
                        });
                    }
                    execution.remember_run(run.as_ref().clone());
                }
                Replayed::Skipped(step_ids) => {
                    for step_id in step_ids {
                        execution.apply_state_event(SequentialGraphEvent::StepSkipped {
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
            execution
                .graph_index
                .start_step(&mut execution.state, &suspension.step_id, at.clone());
            execution.suspended_step = Some(suspension.step_id.clone());
            execution.suspended_hold = Some(suspension.hold.clone());
        }
        execution.planning_cursor =
            replay_planning_cursor(&execution.state, execution.suspended_step.as_deref())?;
        Ok(execution)
    }

    /// The final state of a sealed run: a run sealed `closed` completed.
    pub(super) fn seal_state(&mut self, disposition: &runx_contracts::ClosureDisposition) {
        if *disposition == runx_contracts::ClosureDisposition::Closed {
            self.complete_graph();
        }
    }

    pub(super) fn run<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        if let Some(step_id) = self.suspended_step.clone() {
            let attempt = self
                .state
                .steps
                .iter()
                .find(|step| step.step_id == step_id)
                .ok_or_else(|| RuntimeError::StepMissing {
                    step_id: step_id.clone(),
                })?
                .attempts;
            self.run_one_step(runtime, graph_dir, graph, &step_id, attempt, host)?;
        }
        loop {
            self.mark_when_skipped_steps(runtime, graph)?;
            self.advance_planning_cursor();
            let plan = self
                .graph_index
                .plan_transition(&self.state, self.planning_cursor);
            if self.apply_plan(runtime, graph_dir, graph, host, plan)? {
                break;
            }
        }
        Ok(())
    }

    fn advance_planning_cursor(&mut self) {
        self.planning_cursor = terminal_prefix_cursor(&self.state, self.planning_cursor);
    }

    /// Mark every step whose `when` condition the runtime has resolved to false
    /// as `Skipped`, so the planner walks past it and graph completion treats it
    /// as terminal. Evaluated against the runs so far, so a branch is only
    /// selected out once the step it reads from has produced its output.
    fn mark_when_skipped_steps<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph: &ExecutionGraph,
    ) -> Result<(), RuntimeError> {
        let already_skipped = self
            .state
            .steps
            .iter()
            .filter(|step| step.status == GraphStepStatus::Skipped)
            .map(|step| step.step_id.clone())
            .collect();
        let mut skipped = Vec::new();
        for step_id in when_skipped_steps(graph, &self.runs, &already_skipped) {
            let is_pending = self
                .state
                .steps
                .iter()
                .any(|step| step.step_id == step_id && step.status == GraphStepStatus::Pending);
            if is_pending {
                self.apply_state_event(SequentialGraphEvent::StepSkipped {
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
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        plan: SequentialGraphPlan,
    ) -> Result<bool, RuntimeError>
    where
        A: SkillAdapter,
    {
        match plan {
            SequentialGraphPlan::RunStep {
                step_id, attempt, ..
            } => {
                self.run_one_step(runtime, graph_dir, graph, &step_id, attempt, host)?;
                Ok(false)
            }
            SequentialGraphPlan::Complete => Ok(self.complete_graph()),
            SequentialGraphPlan::Blocked { step_id, reason } => {
                Err(RuntimeError::GraphBlocked { step_id, reason })
            }
            SequentialGraphPlan::Failed { step_id, reason } => self.fail_graph(step_id, reason),
        }
    }

    pub(super) fn complete_graph(&mut self) -> bool {
        self.apply_state_event(SequentialGraphEvent::Complete);
        true
    }

    pub(super) fn fail_graph(
        &mut self,
        step_id: String,
        reason: String,
    ) -> Result<bool, RuntimeError> {
        self.apply_state_event(SequentialGraphEvent::FailGraph {
            error: reason.clone(),
        });
        Err(RuntimeError::GraphPlanningFailed { step_id, reason })
    }

    pub(super) fn run_one_step<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        step_id: &str,
        attempt: u32,
        host: &mut dyn Host,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        // A driver asked to stop admits no further step; the run stays where
        // it is.
        if crate::interrupt::stop_requested() {
            return Err(RuntimeError::Stopped {
                run_id: runtime.options.run.run_id().to_owned(),
                step_id: format!("step {step_id}"),
            });
        }
        let plan = StepExecutionPlan { step_id, attempt };
        let step = self.find_step(graph, plan.step_id)?;
        let loaded_skill = self.cached_step_skill(runtime, graph_dir, step);
        self.run_loaded_step(
            StepExecutionContext {
                runtime,
                graph_dir,
                graph,
                step,
                host,
                plan,
            },
            loaded_skill,
        )
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
        enforce_guards(context.graph, context.step, &self.runs)?;
        let policy_approval_refs =
            verified_approval_references(context.runtime, context.graph, context.step, &self.runs)?;
        let resumed_hold = if let Some(pending) = self.suspended_step.take() {
            if pending != context.plan.step_id {
                return Err(RuntimeError::EngineInvariant {
                    context: "resuming a graph step",
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
        let claimed = super::reaches_world(&context.step.kind);
        if claimed {
            let retry = log.address(&format!("{}/retry", context.plan.step_id));
            log.record_claim(
                context.plan.step_id,
                context.plan.attempt,
                &super::unsettled_retry(
                    retry,
                    &format!("step {}", context.plan.step_id),
                    super::resend_of(
                        context.step,
                        context.graph_dir,
                        &context.runtime.options.env,
                    ),
                ),
                &std::mem::take(&mut answering.answers),
            )?;
        }
        let result = self.execute_step_plan(
            StepPlanTarget {
                runtime: context.runtime,
                graph_dir: context.graph_dir,
                graph: context.graph,
                step: context.step,
                plan: context.plan,
            },
            &mut answering,
            loaded_skill,
            policy_approval_refs,
        );
        let answers = answering.answers;
        let run = match result {
            Err(RuntimeError::ResolutionPending {
                step_id,
                reason,
                request,
                child,
            }) => {
                // A claim recorded since the hold was read is the run's
                // latest event, so the hold is recorded again after it.
                let still_waiting = !claimed
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
        self.commit_step_run(context.runtime, context.host, context.plan, run, &answers)
    }

    fn execute_step_plan<A>(
        &mut self,
        context: StepPlanTarget<'_, A>,
        host: &mut dyn Host,
        loaded_skill: Result<Option<LoadedStepSkill>, RuntimeError>,
        policy_approval_refs: Vec<Reference>,
    ) -> Result<StepRun, RuntimeError>
    where
        A: SkillAdapter,
    {
        let prior_run_index = PriorRunIndex::from_positions(&self.runs, &self.run_positions);
        run_step_with_loaded_skill_index(
            LoadedStepExecutionRequest {
                runtime: context.runtime,
                graph_dir: context.graph_dir,
                graph_name: &context.graph.name,
                step: context.step,
                attempt: context.plan.attempt,
                loaded_skill: loaded_skill?,
                policy_approval_refs,
                host,
            },
            &prior_run_index,
        )
        .map_err(|error| error.at_graph_step(&context.step.id))
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
            let disposition = run.receipt.seal.disposition.clone();
            let reason = run.receipt.seal.reason;
            self.push_run(runtime, run, answers)?;
            if super::is_governed_step_closure(&disposition) {
                return Err(RuntimeError::StepClosed {
                    step_id: plan.step_id.to_owned(),
                    disposition,
                    reason,
                    summary: message,
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
        self.graph_index
            .start_step(&mut self.state, step_id, runtime.options.clock.now());
    }

    pub(super) fn succeed_step<A>(&mut self, runtime: &Runtime<A>, run: &StepRun) {
        self.graph_index.succeed_step(
            &mut self.state,
            runtime.options.clock.now(),
            run.admission_witness.clone(),
            Some(run.contract.clone()),
        );
    }

    pub(super) fn fail_step<A>(&mut self, runtime: &Runtime<A>, step_id: &str, run: &StepRun) {
        self.apply_state_event(SequentialGraphEvent::StepFailed {
            step_id: step_id.to_owned(),
            at: runtime.options.clock.now(),
            error: output_error(run),
        });
    }

    /// Fail the graph in place at `step_id`. A step whose dispatch faulted
    /// has no run yet; `fault_run` records it so the sealed trace holds it.
    pub(super) fn fail_graph_at<A>(
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
        self.apply_state_event(SequentialGraphEvent::FailGraph {
            error: format!("step {step_id} failed"),
        });
        Ok(())
    }

    pub(super) fn has_run(&self, step_id: &str) -> bool {
        self.run_positions.contains_key(step_id)
    }

    /// Whether `step_id`'s latest run is a failure it already sealed.
    pub(super) fn has_failed_run(&self, step_id: &str) -> bool {
        self.run_positions
            .get(step_id)
            .and_then(|index| self.runs.get(*index))
            .is_some_and(|run| !run.outcome.succeeded())
    }

    pub(super) fn attempt(&self, step_id: &str) -> u32 {
        self.state
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
        self.apply_state_event(SequentialGraphEvent::FailGraph {
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
        graph: ExecutionGraph,
        receipt: runx_contracts::Receipt,
    ) -> GraphRun {
        GraphRun {
            graph,
            state: self.state,
            steps: self.runs,
            receipt,
            stop: None,
        }
    }

    /// A run a governed stop sealed: it keeps why it stopped.
    pub(super) fn finish_stopped(
        self,
        graph: ExecutionGraph,
        receipt: runx_contracts::Receipt,
        stop: String,
    ) -> GraphRun {
        GraphRun {
            stop: Some(stop),
            ..self.finish(graph, receipt)
        }
    }

    /// The attempt a terminal failure of `step_id` is recorded under: the
    /// suspended attempt it held, or the next one.
    pub(super) fn failure_attempt(&self, step_id: &str) -> u32 {
        self.state
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .map_or(1, |step| {
                if step.status == GraphStepStatus::Running {
                    step.attempts
                } else {
                    step.attempts.saturating_add(1).max(1)
                }
            })
    }

    fn cached_step_skill(
        &mut self,
        runtime: &Runtime<impl SkillAdapter>,
        graph_dir: &Path,
        step: &GraphStep,
    ) -> Result<Option<LoadedStepSkill>, RuntimeError> {
        if step.skill().is_none() {
            return Ok(None);
        }
        self.step_skill_cache
            .load(
                graph_dir,
                step,
                StepSkillLoadOptions {
                    env: &runtime.options().env,
                },
            )
            .map(Some)
    }

    fn find_step<'a>(
        &self,
        graph: &'a ExecutionGraph,
        step_id: &str,
    ) -> Result<&'a GraphStep, RuntimeError> {
        // `graph_index` is built from exactly this `graph` (see `GraphExecution::new`
        // / `from_record`), which is immutable for the run, so the index position
        // map is always in sync with `graph.steps`. The index's `StepMissing` is the
        // authoritative answer for a genuinely-missing step; a linear re-scan over the
        // same `graph.steps` could never find a step the index legitimately missed, it
        // would only silently paper over an index/graph desync. Return the index result
        // directly so such a desync surfaces instead of being absorbed by an O(n) scan.
        self.graph_index.find_step(graph, step_id)
    }
}

fn replay_planning_cursor(
    state: &SequentialGraphState,
    suspended_step_id: Option<&str>,
) -> Result<usize, RuntimeError> {
    let mut found_pending = false;
    for step in state
        .steps
        .iter()
        .filter(|step| step.status == GraphStepStatus::Running)
    {
        if found_pending || suspended_step_id != Some(step.step_id.as_str()) || step.attempts == 0 {
            return Err(RuntimeError::GraphPlanningFailed {
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
        return Err(RuntimeError::GraphPlanningFailed {
            step_id: step_id.to_owned(),
            reason: "the recorded suspension does not identify a running step".to_owned(),
        });
    }
    Ok(terminal_prefix_cursor(state, 0))
}

fn terminal_prefix_cursor(state: &SequentialGraphState, start: usize) -> usize {
    let mut cursor = start.min(state.steps.len());
    while state.steps.get(cursor).is_some_and(|step| {
        matches!(
            step.status,
            GraphStepStatus::Succeeded | GraphStepStatus::Skipped
        )
    }) {
        cursor += 1;
    }
    cursor
}

/// A step runs only on a condition that holds. One that reads nothing (a
/// field the earlier step's result does not have) never lets the step run.
fn enforce_condition(step: &GraphStep, runs: &[StepRun]) -> Result<(), RuntimeError> {
    let Some(when) = &step.when else {
        return Ok(());
    };
    let shown = operand_text(&when.value);
    let value = operand_value(&when.value, runs).ok_or_else(|| RuntimeError::InvalidRunStep {
        step_id: step.id.clone(),
        reason: format!("condition '{shown}' resolves to nothing"),
    })?;
    let holds = match (&when.equals, &when.not_equals) {
        (Some(expected), _) => value == expected,
        (_, Some(disallowed)) => value != disallowed,
        _ => true,
    };
    if holds {
        Ok(())
    } else {
        Err(RuntimeError::EngineInvariant {
            context: "running a graph step",
            message: format!(
                "step {} was selected although its condition '{shown}' does not hold",
                step.id
            ),
        })
    }
}

pub(super) fn enforce_guards(
    graph: &ExecutionGraph,
    step: &GraphStep,
    runs: &[StepRun],
) -> Result<(), RuntimeError> {
    let Some(policy) = &graph.policy else {
        return Ok(());
    };
    for gate in policy.guards.iter().filter(|gate| gate.step == step.id) {
        let shown = operand_text(&gate.value);
        let Some(value) = operand_value(&gate.value, runs) else {
            return Err(RuntimeError::GraphBlocked {
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
            return Err(RuntimeError::GraphBlocked {
                step_id: step.id.clone(),
                reason: format!("guard '{shown}' expected {}", display_json(expected)),
            });
        }
        if let Some(disallowed) = &gate.not_equals
            && value == disallowed
        {
            return Err(RuntimeError::GraphBlocked {
                step_id: step.id.clone(),
                reason: format!(
                    "guard '{shown}' must not equal {}",
                    display_json(disallowed)
                ),
            });
        }
        if gate.equals.is_none() && gate.not_equals.is_none() {
            return Err(RuntimeError::GraphBlocked {
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
    graph: &ExecutionGraph,
    step: &GraphStep,
    runs: &[StepRun],
) -> Result<Vec<Reference>, RuntimeError>
where
    A: SkillAdapter,
{
    let mut references = Vec::new();
    for guard in graph
        .policy
        .iter()
        .flat_map(|policy| &policy.guards)
        .filter(|guard| guard.step == step.id)
        .filter(|guard| guard.equals == Some(JsonValue::Bool(true)) && guard.not_equals.is_none())
    {
        let Some(approval_step_id) = approval_decision_of(&guard.value) else {
            continue;
        };
        let Some(approval_step) = graph
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
            || approval_run.receipt.seal.disposition != runx_contracts::ClosureDisposition::Closed
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
pub(super) fn operand_value<'a>(
    operand: &'a Operand,
    runs: &'a [StepRun],
) -> Option<&'a JsonValue> {
    match operand {
        Operand::Literal(value) => Some(value),
        Operand::Reference(Source::Step { step, path }) => {
            let run = runs.iter().rev().find(|run| run.step_id == *step)?;
            let (first, rest) = path.split_first()?;
            crate::execution::graph::value_at(run.output(first)?, rest)
        }
        // The run's input is bound before the graph runs, so an input field
        // still unbound is absent; no condition or guard reads the invoker's
        // act or the run's id.
        Operand::Reference(Source::Input { .. } | Source::Act { .. } | Source::RunId) => None,
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
/// graph blocked.
pub(super) fn when_skipped_steps(
    graph: &ExecutionGraph,
    runs: &[StepRun],
    already_skipped: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let mut skipped = already_skipped.clone();
    loop {
        let previous_len = skipped.len();
        for step in &graph.steps {
            let condition_source = step.when.as_ref().and_then(|when| when.value.step());
            if step
                .reads()
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
            let Some(value) = operand_value(&when.value, runs) else {
                continue;
            };
            let satisfied = match (&when.equals, &when.not_equals) {
                (Some(expected), _) => value == expected,
                (_, Some(disallowed)) => value != disallowed,
                _ => true,
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

    use runx_core::state_machine::{
        GraphStepStatus, SequentialGraphStepDefinition, create_sequential_graph_state,
    };
    use runx_parser::{parse_graph_yaml, validate_graph};

    use super::{replay_planning_cursor, when_skipped_steps};

    fn replay_state(
        statuses: &[GraphStepStatus],
    ) -> runx_core::state_machine::SequentialGraphState {
        let definitions = statuses
            .iter()
            .enumerate()
            .map(|(index, _)| SequentialGraphStepDefinition {
                id: format!("step_{index}"),
                context_from: None,
            })
            .collect::<Vec<_>>();
        let mut state = create_sequential_graph_state("graph", &definitions);
        for (step, status) in state.steps.iter_mut().zip(statuses) {
            step.status = status.clone();
        }
        state
    }

    #[test]
    fn replay_cursor_starts_at_the_first_non_terminal_step() {
        let state = replay_state(&[
            GraphStepStatus::Succeeded,
            GraphStepStatus::Skipped,
            GraphStepStatus::Failed,
            GraphStepStatus::Pending,
        ]);

        assert_eq!(
            replay_planning_cursor(&state, None).expect("valid record"),
            2
        );
    }

    #[test]
    fn replay_cursor_rejects_running_state_anywhere() {
        let state = replay_state(&[
            GraphStepStatus::Succeeded,
            GraphStepStatus::Pending,
            GraphStepStatus::Running,
        ]);

        let error = replay_planning_cursor(&state, None).expect_err("a running step must fail");
        assert!(error.to_string().contains("holds a running step"));
    }

    #[test]
    fn skipped_branch_predicates_propagate_to_unreachable_descendants() {
        let graph = validate_graph(
            parse_graph_yaml(
                r#"
name: conditional-propagation
steps:
  - id: source
    agent: { role: test, task: source }
    outputs: { decision: string }
  - id: inspect
    when: { value: $steps.source.decision, equals: ready }
    agent: { role: test, task: inspect }
    outputs: { decision: string }
  - id: reject
    when: { value: $steps.inspect.decision, equals: reject }
    agent: { role: test, task: reject }
    outputs: { decision: string }
"#,
            )
            .expect("fixture graph should parse"),
        )
        .expect("fixture graph should validate");
        let skipped = when_skipped_steps(&graph, &[], &BTreeSet::from(["inspect".to_owned()]));

        assert!(skipped.contains("reject"));
    }

    #[test]
    fn a_step_that_reads_a_skipped_step_is_skipped_whatever_it_reads_it_through() {
        let graph = validate_graph(
            parse_graph_yaml(
                r#"
name: read-propagation
steps:
  - id: inspect
    when: { value: $input.inspect, equals: true }
    agent: { role: test, task: inspect }
    outputs: { report: object }
  - id: publish
    when: { value: $input.publish, equals: true }
    agent: { role: test, task: publish }
    inputs: { report: $steps.inspect.report }
"#,
            )
            .expect("fixture graph should parse"),
        )
        .expect("fixture graph should validate");
        let skipped = when_skipped_steps(&graph, &[], &BTreeSet::from(["inspect".to_owned()]));

        assert!(skipped.contains("publish"));
    }

    #[test]
    fn a_condition_that_reads_nothing_never_lets_its_step_run() {
        let graph = validate_graph(
            parse_graph_yaml(
                r#"
name: misspelled-condition
steps:
  - id: review
    agent: { role: test, task: review }
    outputs: { verdict: string }
  - id: send
    when: { value: $steps.review.verdcit, equals: approved }
    agent: { role: test, task: send }
"#,
            )
            .expect("fixture graph should parse"),
        )
        .expect("fixture graph should validate");
        let send = &graph.steps[1];
        let error = super::enforce_condition(send, &[]).expect_err("unresolved");
        assert!(error.to_string().contains("resolves to nothing"), "{error}");
    }

    #[test]
    fn a_branch_on_the_run_input_is_decided_before_any_step_runs() {
        let graph = validate_graph(
            parse_graph_yaml(
                r#"
name: input-branch
steps:
  - id: apply
    when: { value: $input.mode, equals: apply }
    agent: { role: test, task: apply }
  - id: preview
    when: { value: $input.mode, not_equals: apply }
    agent: { role: test, task: preview }
"#,
            )
            .expect("fixture graph should parse"),
        )
        .expect("fixture graph should validate");
        let inputs = runx_contracts::JsonObject::from([(
            "mode".to_owned(),
            runx_contracts::JsonValue::String("preview".to_owned()),
        )]);
        let graph = crate::execution::graph::bind_run_inputs(graph, &inputs, None, "run")
            .expect("the run input binds");
        let skipped = when_skipped_steps(&graph, &[], &BTreeSet::new());

        assert_eq!(skipped, BTreeSet::from(["apply".to_owned()]));
    }

    #[test]
    fn an_absent_input_satisfies_no_condition_and_passes_no_guard() {
        let graph = validate_graph(
            parse_graph_yaml(
                r#"
name: absent-input
steps:
  - id: preview
    when: { value: $input.mode, not_equals: apply }
    agent: { role: test, task: preview }
  - id: send
    agent: { role: test, task: send }
policy:
  guards:
    - { step: send, value: $input.approved, not_equals: false }
"#,
            )
            .expect("fixture graph should parse"),
        )
        .expect("fixture graph should validate");
        let graph = crate::execution::graph::bind_run_inputs(
            graph,
            &runx_contracts::JsonObject::new(),
            None,
            "run",
        )
        .expect("the run input binds");

        assert!(
            when_skipped_steps(&graph, &[], &BTreeSet::new()).is_empty(),
            "an absent input decides no branch"
        );
        let error = super::enforce_condition(&graph.steps[0], &[]).expect_err("absent");
        assert!(error.to_string().contains("resolves to nothing"), "{error}");
        let error = super::enforce_guards(&graph, &graph.steps[1], &[]).expect_err("absent");
        assert!(
            error
                .to_string()
                .contains("guard '$input.approved' is unresolved"),
            "{error}"
        );
    }
}
