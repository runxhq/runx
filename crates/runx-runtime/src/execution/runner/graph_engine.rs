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
use runx_parser::{ExecutionGraph, GraphStep, StepKind};

use super::super::graph::{LoadedStepSkill, StepSkillCache, StepSkillLoadOptions};
use super::super::graph_index::{ExecutionGraphIndex, PriorRunIndex};
use super::super::run_record::{Answer, Hold, Replayed, RunRecord, request_digest};
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
/// answer the step refuses is never recorded.
pub(super) struct AnsweringHost<'a> {
    pub(super) host: &'a mut dyn Host,
    pub(super) answers: Vec<Answer>,
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
        if let Some(suspension) = &record.suspension
            && !matches!(suspension.hold, Hold::Retry(_))
        {
            execution
                .graph_index
                .start_step(&mut execution.state, &suspension.step_id, at.clone());
            execution.suspended_step = Some(suspension.step_id.clone());
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
                    at: runtime.options.created_at.clone(),
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
        enforce_guards(context.graph, context.step, &self.runs)?;
        let policy_approval_refs =
            verified_approval_references(context.runtime, context.graph, context.step, &self.runs)?;
        if let Some(pending) = self.suspended_step.take() {
            if pending != context.plan.step_id {
                return Err(RuntimeError::EngineInvariant {
                    context: "resuming a graph step",
                    message: "the suspended step is not the selected step".to_owned(),
                });
            }
        } else {
            self.record_lifecycle(
                context.host,
                LifecycleEvent::step_started(context.plan.step_id),
            )?;
            self.start_step(context.runtime, context.plan.step_id);
        }
        let mut answering = AnsweringHost {
            host: &mut *context.host,
            answers: Vec::new(),
        };
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
                let log = &context.runtime.options.run;
                match &child {
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
            let status = run
                .outcome
                .metadata
                .get("http_status")
                .and_then(|value| value.as_str())
                .map(|status| format!("status {status}: "))
                .unwrap_or_default();
            let message = format!(
                "{status}{}",
                run.outcome
                    .failure_message()
                    .unwrap_or_else(|| "step failed with no diagnostic output".to_owned())
            );
            // The failed run is recorded even on terminal failure so the run
            // list agrees with the run record's step failure; a failed attempt
            // must never be silently absent from the execution record.
            let disposition = run.receipt.seal.disposition.clone();
            let reason_code = run.receipt.seal.reason_code.to_string();
            self.push_run(runtime, run, answers)?;
            if super::is_governed_step_closure(&disposition) {
                return Err(RuntimeError::StepClosed {
                    step_id: plan.step_id.to_owned(),
                    disposition,
                    reason_code,
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
            .start_step(&mut self.state, step_id, runtime.options.created_at.clone());
    }

    pub(super) fn succeed_step<A>(&mut self, runtime: &Runtime<A>, run: &StepRun) {
        self.graph_index.succeed_step(
            &mut self.state,
            runtime.options.created_at.clone(),
            run.admission_witness.clone(),
            Some(run.contract.clone()),
        );
    }

    pub(super) fn fail_step<A>(&mut self, runtime: &Runtime<A>, step_id: &str, run: &StepRun) {
        self.apply_state_event(SequentialGraphEvent::StepFailed {
            step_id: step_id.to_owned(),
            at: runtime.options.created_at.clone(),
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

pub(super) fn enforce_guards(
    graph: &ExecutionGraph,
    step: &GraphStep,
    runs: &[StepRun],
) -> Result<(), RuntimeError> {
    let Some(policy) = &graph.policy else {
        return Ok(());
    };
    for gate in policy.guards.iter().filter(|gate| gate.step == step.id) {
        let Some(value) = transition_field_value(&gate.field, runs) else {
            return Err(RuntimeError::GraphBlocked {
                step_id: step.id.clone(),
                reason: format!("guard '{}' is unresolved", gate.field),
            });
        };
        if let Some(expected) = &gate.equals
            && value != expected
        {
            if expected == &JsonValue::Bool(true)
                && value == &JsonValue::Bool(false)
                && gate.field.ends_with(".approval_decision.data.approved")
            {
                return Err(crate::RuntimeError::AuthorityDenied {
                    verb: runx_contracts::AuthorityVerb::Execute,
                    step_id: step.id.clone(),
                    reason: format!("approval guard '{}' was denied", gate.field),
                });
            }
            return Err(RuntimeError::GraphBlocked {
                step_id: step.id.clone(),
                reason: format!("guard '{}' expected {}", gate.field, display_json(expected)),
            });
        }
        if let Some(disallowed) = &gate.not_equals
            && value == disallowed
        {
            return Err(RuntimeError::GraphBlocked {
                step_id: step.id.clone(),
                reason: format!(
                    "guard '{}' must not equal {}",
                    gate.field,
                    display_json(disallowed)
                ),
            });
        }
        if gate.equals.is_none() && gate.not_equals.is_none() {
            return Err(RuntimeError::GraphBlocked {
                step_id: step.id.clone(),
                reason: format!("guard '{}' has no comparison", gate.field),
            });
        }
    }
    Ok(())
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
        .filter(|guard| {
            guard.equals == Some(JsonValue::Bool(true))
                && guard.not_equals.is_none()
                && guard.field.ends_with(".approval_decision.data.approved")
        })
    {
        let approval_step_id = guard.field.split('.').next().unwrap_or_default();
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
            || transition_field_value(&guard.field, runs) != Some(&JsonValue::Bool(true))
        {
            continue;
        }
        crate::receipts::tree::validate_runtime_receipt_tree_with_policy(
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
            true,
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

pub(super) fn transition_field_value<'a>(
    field: &str,
    runs: &'a [StepRun],
) -> Option<&'a JsonValue> {
    let mut segments = field.split('.');
    let step_id = segments.next()?;
    let run = runs.iter().rev().find(|run| run.step_id == step_id)?;
    let first = segments.next()?;
    // Guards and `when` conditions resolve against the same declared contract
    // as context edges. Runtime diagnostics live on `StepOutcome`, never in
    // this map, so there is no second implicit control-flow surface.
    let mut value = run.contract.get(first)?;
    for segment in segments {
        let JsonValue::Object(object) = value else {
            return None;
        };
        value = object.get(segment)?;
    }
    Some(value)
}

pub(super) fn display_json(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unprintable>".to_owned())
}

/// Resolve which steps a `when` condition selects out, given the runs so far.
/// A pending predicate source leaves the branch pending. A source already
/// selected out makes every branch depending on its absent output unreachable,
/// so selection propagates transitively instead of leaving the graph blocked.
pub(super) fn when_skipped_steps(
    graph: &ExecutionGraph,
    runs: &[StepRun],
    already_skipped: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let mut skipped = already_skipped.clone();
    loop {
        let previous_len = skipped.len();
        for step in &graph.steps {
            let Some(when) = &step.when else {
                continue;
            };
            let predicate_step = when.field.split('.').next().unwrap_or_default();
            if skipped.contains(predicate_step) {
                skipped.insert(step.id.clone());
                continue;
            }
            let Some(value) = transition_field_value(&when.field, runs) else {
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
    when: { field: source.decision, equals: ready }
    agent: { role: test, task: inspect }
    outputs: { decision: string }
  - id: reject
    when: { field: inspect.decision, equals: reject }
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
}
