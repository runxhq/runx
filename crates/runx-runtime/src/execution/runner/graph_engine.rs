// The graph engine owns state transitions, fanout scheduling, step commits,
// and checkpoint state. Step behavior itself enters through dispatch.
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use runx_contracts::{ExecutionEvent, FanoutReceiptSyncPoint, JsonValue, Reference};
use runx_core::state_machine::{
    FanoutBranchPlan, FanoutGroupPolicy, FanoutSyncDecision, FanoutSyncOutcome, GraphStepStatus,
    SequentialGraphEvent, SequentialGraphPlan, SequentialGraphState, create_sequential_graph_state,
};
use runx_parser::{ExecutionGraph, GraphRunTarget, GraphStep};

use super::super::fanout::fanout_policies;
use super::super::graph::{LoadedStepSkill, StepSkillCache, StepSkillLoadOptions};
use super::super::graph_index::{ExecutionGraphIndex, PriorRunIndex};
use super::dispatch::{
    LoadedStepExecutionRequest, StepFault, run_step_with_loaded_skill,
    run_step_with_loaded_skill_index,
};
use super::scheduler::{
    FanoutSchedule, FanoutScheduler, ParallelFanoutSchedule, ParallelWidth, Parallelism,
    parallel_safe_step_shape,
};
use super::step_handlers::{output_error, runtime_error_step_run};
use super::sync::fanout_sync_point;
use super::{GraphCheckpoint, GraphRun, Runtime, StepRun, SuspendedGraphStep};
use crate::RuntimeError;
use crate::adapter::{BorrowedSkillAdapter, SkillAdapter};
use crate::host::{Host, RejectingParallelHost};
use crate::journal::ExecutionJournal;
use crate::lifecycle::LifecycleEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StepFailureMode {
    Propagate,
    RecordAndContinue,
}

struct FanoutRunPlan {
    group_id: String,
    branches: Vec<FanoutBranchPlan>,
}

struct PlannedFanoutStep<'a> {
    attempt: u32,
    step: &'a GraphStep,
    loaded_skill: Result<Option<LoadedStepSkill>, RuntimeError>,
    lane: StepLane,
}

enum StepLane {
    Serial,
    Isolated {
        width: ParallelWidth,
        executor: Box<dyn SkillAdapter + Send + Sync>,
    },
}

impl StepLane {
    fn parallelism(&self) -> Parallelism {
        match self {
            Self::Serial => Parallelism::Serial,
            Self::Isolated { width, .. } => Parallelism::Isolated(*width),
        }
    }
}

pub(super) struct GraphExecution {
    graph_index: ExecutionGraphIndex,
    planning_cursor: usize,
    step_skill_cache: StepSkillCache,
    state: SequentialGraphState,
    pub(super) runs: Vec<StepRun>,
    run_positions: BTreeMap<String, usize>,
    pub(super) sync_points: Vec<FanoutReceiptSyncPoint>,
    journal: ExecutionJournal,
    suspended_step: Option<Box<SuspendedGraphStep>>,
}

struct ParallelFanoutJob<'a> {
    attempt: u32,
    step: &'a GraphStep,
    loaded_skill: Option<LoadedStepSkill>,
    policy_approval_refs: Vec<Reference>,
    executor: Box<dyn SkillAdapter + Send + Sync>,
}

struct ParallelFanoutContext<'a> {
    options: &'a Arc<super::RuntimeOptions>,
    javascript: &'a crate::adapters::javascript::JavaScriptAdapter,
    local_artifacts: &'a crate::services::LocalArtifactService,
    graph_dir: &'a Path,
    graph_name: &'a str,
    prior_run_index: &'a PriorRunIndex<'a>,
}

#[derive(Clone, Copy)]
pub(super) struct StepExecutionPlan<'a> {
    step_id: &'a str,
    attempt: u32,
    failure_mode: StepFailureMode,
}

struct StepExecutionContext<'a, A> {
    runtime: &'a Runtime<A>,
    graph_dir: &'a Path,
    graph: &'a ExecutionGraph,
    step: &'a GraphStep,
    host: &'a mut dyn Host,
    plan: StepExecutionPlan<'a>,
}

const DISABLE_RUNTIME_INDEXES_ENV: &str = "RUNX_RUNTIME_DISABLE_INDEXES";

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
            sync_points: Vec::new(),
            journal: ExecutionJournal::default(),
            suspended_step: None,
        }
    }

    fn apply_state_event(&mut self, event: SequentialGraphEvent) {
        self.graph_index.apply_event(&mut self.state, event);
    }

    pub(super) fn from_checkpoint(
        graph: &ExecutionGraph,
        checkpoint: GraphCheckpoint,
    ) -> Result<Self, RuntimeError> {
        if checkpoint.graph_name != graph.name {
            return Err(RuntimeError::CheckpointGraphMismatch {
                checkpoint_graph: checkpoint.graph_name,
                graph: graph.name.clone(),
            });
        }
        let definitions = super::super::graph::step_definitions(graph);
        let graph_index = ExecutionGraphIndex::new(graph, definitions);
        let planning_cursor = checkpoint_planning_cursor(
            graph,
            &checkpoint.state,
            &checkpoint.sync_points,
            checkpoint
                .suspended_step
                .as_ref()
                .map(|pending| pending.step_id.as_str()),
        )?;
        let run_positions = run_positions(&checkpoint.steps);
        Ok(Self {
            graph_index,
            planning_cursor,
            step_skill_cache: StepSkillCache::default(),
            state: checkpoint.state,
            runs: checkpoint.steps,
            run_positions,
            sync_points: checkpoint.sync_points,
            journal: checkpoint.journal,
            suspended_step: checkpoint.suspended_step,
        })
    }

    pub(super) fn run<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        max_new_steps: Option<usize>,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        let fanout_policies = fanout_policies(graph);
        let initial_step_count = self.runs.len();
        if max_new_steps != Some(0)
            && let Some(pending) = &self.suspended_step
        {
            let step_id = pending.step_id.clone();
            let attempt = self
                .state
                .steps
                .iter()
                .find(|step| step.step_id == step_id)
                .ok_or_else(|| RuntimeError::StepMissing {
                    step_id: step_id.clone(),
                })?
                .attempts;
            let group_id = self.find_step(graph, &step_id)?.fanout_group.clone();
            self.run_one_step_with_mode(
                runtime,
                graph_dir,
                graph,
                host,
                StepExecutionPlan {
                    step_id: &step_id,
                    attempt,
                    failure_mode: if group_id.is_some() {
                        StepFailureMode::RecordAndContinue
                    } else {
                        StepFailureMode::Propagate
                    },
                },
            )?;
            if let Some(group_id) = group_id {
                self.record_proceeding_fanout_sync_point(graph, &fanout_policies, &group_id)?;
            }
        }
        loop {
            if reached_step_limit(initial_step_count, self.runs.len(), max_new_steps) {
                return Ok(());
            }
            self.mark_when_skipped_steps(graph, &runtime.options.created_at);
            self.advance_planning_cursor(graph);
            let plan = self.graph_index.plan_transition(
                &self.state,
                &fanout_policies,
                self.planning_cursor,
            );
            if self.apply_plan(runtime, graph_dir, graph, host, &fanout_policies, plan)? {
                break;
            }
        }
        Ok(())
    }

    fn advance_planning_cursor(&mut self, graph: &ExecutionGraph) {
        self.planning_cursor =
            terminal_prefix_cursor(graph, &self.state, &self.sync_points, self.planning_cursor);
    }

    /// Mark every step whose `when` condition the runtime has resolved to false
    /// as `Skipped`, so the planner walks past it and graph completion treats it
    /// as terminal. Evaluated against the runs so far, so a branch is only
    /// selected out once the step it reads from has produced its output.
    fn mark_when_skipped_steps(&mut self, graph: &ExecutionGraph, at: &str) {
        let already_skipped = self
            .state
            .steps
            .iter()
            .filter(|step| step.status == GraphStepStatus::Skipped)
            .map(|step| step.step_id.clone())
            .collect();
        for step_id in when_skipped_steps(graph, &self.runs, &already_skipped) {
            let is_pending = self
                .state
                .steps
                .iter()
                .any(|step| step.step_id == step_id && step.status == GraphStepStatus::Pending);
            if is_pending {
                self.apply_state_event(SequentialGraphEvent::StepSkipped {
                    step_id,
                    at: at.to_owned(),
                });
            }
        }
    }

    pub(super) fn apply_plan<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        fanout_policies: &BTreeMap<String, FanoutGroupPolicy>,
        plan: SequentialGraphPlan,
    ) -> Result<bool, RuntimeError>
    where
        A: SkillAdapter,
    {
        match plan {
            SequentialGraphPlan::RunStep {
                step_id, attempt, ..
            } => self.apply_step_plan(runtime, graph_dir, graph, host, &step_id, attempt),
            SequentialGraphPlan::RunFanout { group_id, branches } => {
                self.run_fanout_plan(
                    runtime,
                    graph_dir,
                    graph,
                    host,
                    fanout_policies,
                    FanoutRunPlan { group_id, branches },
                )?;
                Ok(false)
            }
            SequentialGraphPlan::Complete => Ok(self.complete_graph()),
            SequentialGraphPlan::Blocked {
                step_id,
                reason,
                sync_decision,
            } => self.block_graph(graph, step_id, reason, sync_decision),
            SequentialGraphPlan::Failed {
                step_id,
                reason,
                sync_decision,
            } => self.fail_graph(graph, step_id, reason, sync_decision),
            SequentialGraphPlan::Paused {
                step_id,
                reason,
                sync_decision,
            } => self.pause_for_sync(graph, step_id, reason, sync_decision),
            SequentialGraphPlan::Escalated {
                step_id,
                reason,
                sync_decision,
            } => self.escalate_for_sync(graph, step_id, reason, sync_decision),
        }
    }

    pub(super) fn apply_step_plan<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        step_id: &str,
        attempt: u32,
    ) -> Result<bool, RuntimeError>
    where
        A: SkillAdapter,
    {
        self.run_one_step(runtime, graph_dir, graph, step_id, attempt, host)?;
        Ok(false)
    }

    pub(super) fn complete_graph(&mut self) -> bool {
        self.apply_state_event(SequentialGraphEvent::Complete);
        true
    }

    fn run_fanout_plan<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        fanout_policies: &BTreeMap<String, FanoutGroupPolicy>,
        plan: FanoutRunPlan,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        if runtime
            .options
            .env
            .contains_key(DISABLE_RUNTIME_INDEXES_ENV)
        {
            self.run_serial_fanout_steps(runtime, graph_dir, graph, host, &plan.branches)?;
            return self.record_proceeding_fanout_sync_point(
                graph,
                fanout_policies,
                &plan.group_id,
            );
        }

        let scheduler = FanoutScheduler::from_env(&runtime.options.env);
        if !scheduler.can_parallelize(plan.branches.len()) {
            self.run_serial_fanout_steps(runtime, graph_dir, graph, host, &plan.branches)?;
            return self.record_proceeding_fanout_sync_point(
                graph,
                fanout_policies,
                &plan.group_id,
            );
        }

        let steps = self.plan_fanout_steps(runtime, graph_dir, graph, &plan.branches)?;
        match scheduler.schedule(steps, |step| step.lane.parallelism()) {
            FanoutSchedule::Serial(steps) => {
                self.run_planned_fanout_steps(runtime, graph_dir, graph, host, steps)?;
            }
            FanoutSchedule::Parallel(schedule) => {
                self.run_parallel_fanout_steps(runtime, graph_dir, graph, host, schedule)?;
            }
        }
        self.record_proceeding_fanout_sync_point(graph, fanout_policies, &plan.group_id)
    }

    fn run_serial_fanout_steps<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        branches: &[FanoutBranchPlan],
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        for branch in branches {
            self.run_one_step_with_mode(
                runtime,
                graph_dir,
                graph,
                host,
                StepExecutionPlan {
                    step_id: &branch.step_id,
                    attempt: branch.attempt,
                    failure_mode: StepFailureMode::RecordAndContinue,
                },
            )?;
        }
        Ok(())
    }

    fn plan_fanout_steps<'a, A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &'a ExecutionGraph,
        branches: &[FanoutBranchPlan],
    ) -> Result<Vec<PlannedFanoutStep<'a>>, RuntimeError>
    where
        A: SkillAdapter,
    {
        branches
            .iter()
            .map(|branch| {
                let step = self.find_step(graph, &branch.step_id)?;
                // Loading is speculative only for lane selection. Preserve the
                // exact result so a failed load follows the serial branch path
                // and is sealed under RecordAndContinue instead of being
                // erased into an absent skill.
                let loaded_skill = self.cached_step_skill(runtime, graph_dir, step);
                let lane = self.plan_step_lane(
                    runtime,
                    step,
                    loaded_skill.as_ref().ok().and_then(Option::as_ref),
                );
                Ok(PlannedFanoutStep {
                    attempt: branch.attempt,
                    step,
                    loaded_skill,
                    lane,
                })
            })
            .collect()
    }

    fn plan_step_lane<A>(
        &self,
        runtime: &Runtime<A>,
        step: &GraphStep,
        loaded_skill: Option<&LoadedStepSkill>,
    ) -> StepLane
    where
        A: SkillAdapter,
    {
        if !parallel_safe_step_shape(step, &runtime.options().effects) {
            return StepLane::Serial;
        }
        let Some(skill) = loaded_skill else {
            return StepLane::Serial;
        };
        if skill.runner.source.source_type == runx_parser::SourceKind::JavaScript {
            let Some(width) = NonZeroUsize::new(runtime.javascript.max_concurrency()) else {
                return StepLane::Serial;
            };
            return StepLane::Isolated {
                width: ParallelWidth::Bounded(width),
                executor: Box::new(runtime.javascript.clone()),
            };
        }
        runtime
            .configured_adapter
            .isolated_fanout_adapter(&skill.runner.source)
            .map_or(StepLane::Serial, |executor| StepLane::Isolated {
                width: ParallelWidth::Unbounded,
                executor,
            })
    }

    fn run_planned_fanout_steps<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        steps: Vec<PlannedFanoutStep<'_>>,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        for planned in steps {
            self.run_loaded_step_with_mode(
                StepExecutionContext {
                    runtime,
                    graph_dir,
                    graph,
                    step: planned.step,
                    host,
                    plan: StepExecutionPlan {
                        step_id: &planned.step.id,
                        attempt: planned.attempt,
                        failure_mode: StepFailureMode::RecordAndContinue,
                    },
                },
                planned.loaded_skill,
            )?;
        }
        Ok(())
    }

    fn run_parallel_fanout_steps<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        schedule: ParallelFanoutSchedule<PlannedFanoutStep<'_>>,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        for planned in &schedule.steps {
            enforce_guards(graph, planned.step, &self.runs)?;
        }
        for planned in &schedule.steps {
            self.record_lifecycle(host, LifecycleEvent::step_started(&planned.step.id))?;
            self.start_step(runtime, &planned.step.id);
        }

        let commit_plans = schedule
            .steps
            .iter()
            .map(|planned| StepExecutionPlan {
                step_id: planned.step.id.as_str(),
                attempt: planned.attempt,
                failure_mode: StepFailureMode::RecordAndContinue,
            })
            .collect::<Vec<_>>();
        let results = self.execute_parallel_fanout_steps(
            runtime,
            graph_dir,
            graph,
            schedule.steps,
            schedule.max_concurrency,
        )?;
        for (result, plan) in results.into_iter().zip(commit_plans) {
            self.commit_step_run(runtime, host, plan, result, false)?;
        }
        Ok(())
    }

    fn execute_parallel_fanout_steps<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        steps: Vec<PlannedFanoutStep<'_>>,
        max_concurrency: usize,
    ) -> Result<Vec<StepRun>, RuntimeError>
    where
        A: SkillAdapter,
    {
        let jobs = steps
            .into_iter()
            .map(|planned| {
                let StepLane::Isolated { executor, .. } = planned.lane else {
                    return Err(fanout_worker_error(format!(
                        "step {} reached the parallel lane without an isolated executor",
                        planned.step.id
                    )));
                };
                let loaded_skill = planned.loaded_skill.map_err(|error| {
                    RuntimeError::engine(
                        "parallel fanout admitted a step whose skill failed to load",
                        error,
                    )
                })?;
                let policy_approval_refs =
                    verified_approval_references(runtime, graph, planned.step, &self.runs)?;
                Ok(Mutex::new(Some(ParallelFanoutJob {
                    attempt: planned.attempt,
                    step: planned.step,
                    loaded_skill,
                    policy_approval_refs,
                    executor,
                })))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        let worker_count = max_concurrency.max(1).min(jobs.len());
        let cursor = AtomicUsize::new(0);
        let outcomes = (0..jobs.len())
            .map(|_| OnceLock::new())
            .collect::<Vec<OnceLock<Result<StepRun, RuntimeError>>>>();
        let prior_run_index = PriorRunIndex::from_positions(&self.runs, &self.run_positions);
        let context = ParallelFanoutContext {
            options: &runtime.options,
            javascript: &runtime.javascript,
            local_artifacts: &runtime.local_artifacts,
            graph_dir,
            graph_name: &graph.name,
            prior_run_index: &prior_run_index,
        };
        thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                let jobs = &jobs;
                let cursor = &cursor;
                let outcomes = &outcomes;
                let context = &context;
                handles.push(scope.spawn(move || {
                    loop {
                        let index = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(job_slot) = jobs.get(index) else {
                            return Ok::<(), RuntimeError>(());
                        };
                        let job = job_slot
                            .lock()
                            .map_err(|_| fanout_worker_error(format!("job {index} was poisoned")))?
                            .take()
                            .ok_or_else(|| {
                                fanout_worker_error(format!("job {index} was claimed twice"))
                            })?;
                        let result = execute_parallel_fanout_job(job, context);
                        outcomes[index].set(result).map_err(|_| {
                            fanout_worker_error(format!("job {index} completed twice"))
                        })?;
                    }
                }));
            }
            join_parallel_fanout_workers(handles)?;
            Ok::<(), RuntimeError>(())
        })?;
        outcomes
            .into_iter()
            .enumerate()
            .map(|(index, outcome)| {
                outcome
                    .into_inner()
                    .ok_or_else(|| fanout_worker_error(format!("job {index} produced no result")))?
            })
            .collect()
    }

    pub(super) fn block_graph(
        &mut self,
        graph: &ExecutionGraph,
        step_id: String,
        reason: String,
        sync_decision: Option<FanoutSyncDecision>,
    ) -> Result<bool, RuntimeError> {
        if let Some(sync_decision) = sync_decision {
            self.push_sync_point(graph, &sync_decision)?;
        }
        Err(RuntimeError::GraphBlocked { step_id, reason })
    }

    pub(super) fn fail_graph(
        &mut self,
        graph: &ExecutionGraph,
        step_id: String,
        reason: String,
        sync_decision: Option<FanoutSyncDecision>,
    ) -> Result<bool, RuntimeError> {
        if let Some(sync_decision) = sync_decision {
            self.push_sync_point(graph, &sync_decision)?;
        }
        self.apply_state_event(SequentialGraphEvent::FailGraph {
            error: reason.clone(),
        });
        Err(RuntimeError::GraphPlanningFailed { step_id, reason })
    }

    pub(super) fn pause_graph(
        &mut self,
        step_id: String,
        reason: String,
        sync_decision: runx_core::state_machine::FanoutSyncDecision,
    ) -> Result<bool, RuntimeError> {
        self.apply_state_event(SequentialGraphEvent::PauseGraph {
            reason: reason.clone(),
        });
        Err(RuntimeError::GraphPaused {
            step_id,
            reason,
            sync_decision: Box::new(sync_decision),
        })
    }

    pub(super) fn pause_for_sync(
        &mut self,
        graph: &ExecutionGraph,
        step_id: String,
        reason: String,
        sync_decision: FanoutSyncDecision,
    ) -> Result<bool, RuntimeError> {
        self.push_sync_point(graph, &sync_decision)?;
        self.pause_graph(step_id, reason, sync_decision)
    }

    pub(super) fn escalate_graph(
        &mut self,
        step_id: String,
        reason: String,
        sync_decision: runx_core::state_machine::FanoutSyncDecision,
    ) -> Result<bool, RuntimeError> {
        self.apply_state_event(SequentialGraphEvent::EscalateGraph {
            reason: reason.clone(),
        });
        Err(RuntimeError::GraphEscalated {
            step_id,
            reason,
            sync_decision: Box::new(sync_decision),
        })
    }

    pub(super) fn escalate_for_sync(
        &mut self,
        graph: &ExecutionGraph,
        step_id: String,
        reason: String,
        sync_decision: FanoutSyncDecision,
    ) -> Result<bool, RuntimeError> {
        self.push_sync_point(graph, &sync_decision)?;
        self.escalate_graph(step_id, reason, sync_decision)
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
        self.run_one_step_with_mode(
            runtime,
            graph_dir,
            graph,
            host,
            StepExecutionPlan {
                step_id,
                attempt,
                failure_mode: StepFailureMode::Propagate,
            },
        )
    }

    pub(super) fn run_one_step_with_mode<A>(
        &mut self,
        runtime: &Runtime<A>,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        host: &mut dyn Host,
        plan: StepExecutionPlan<'_>,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        let step = self.find_step(graph, plan.step_id)?;
        let loaded_skill = self.cached_step_skill(runtime, graph_dir, step);
        self.run_loaded_step_with_mode(
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

    fn run_loaded_step_with_mode<A>(
        &mut self,
        mut context: StepExecutionContext<'_, A>,
        loaded_skill: Result<Option<LoadedStepSkill>, RuntimeError>,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        enforce_guards(context.graph, context.step, &self.runs)?;
        let policy_approval_refs =
            verified_approval_references(context.runtime, context.graph, context.step, &self.runs)?;
        let retry_remaining = retry_budget_remaining(context.step, context.plan.attempt);
        let child_checkpoint = if let Some(pending) = self.suspended_step.take() {
            if pending.step_id != context.plan.step_id {
                return Err(RuntimeError::EngineInvariant {
                    context: "resuming a graph step",
                    message: "pending checkpoint does not belong to the selected step".to_owned(),
                });
            }
            pending.child
        } else {
            self.record_lifecycle(
                context.host,
                LifecycleEvent::step_started(context.plan.step_id),
            )?;
            self.start_step(context.runtime, context.plan.step_id);
            None
        };
        let run = match self.execute_step_plan(
            &mut context,
            loaded_skill,
            policy_approval_refs,
            child_checkpoint,
        ) {
            Err(RuntimeError::ResolutionPending {
                step_id,
                reason,
                checkpoint,
            }) => {
                self.suspended_step = Some(Box::new(SuspendedGraphStep {
                    step_id: context.plan.step_id.to_owned(),
                    child: checkpoint,
                }));
                return Err(RuntimeError::ResolutionPending {
                    step_id,
                    reason,
                    checkpoint: None,
                });
            }
            result => result?,
        };
        self.commit_step_run(
            context.runtime,
            context.host,
            context.plan,
            run,
            retry_remaining,
        )
    }

    fn execute_step_plan<A>(
        &mut self,
        context: &mut StepExecutionContext<'_, A>,
        loaded_skill: Result<Option<LoadedStepSkill>, RuntimeError>,
        policy_approval_refs: Vec<Reference>,
        child_checkpoint: Option<Box<GraphCheckpoint>>,
    ) -> Result<StepRun, RuntimeError>
    where
        A: SkillAdapter,
    {
        let run_result = loaded_skill
            .map_err(StepFault::from)
            .and_then(|loaded_skill| {
                if context
                    .runtime
                    .options
                    .env
                    .contains_key(DISABLE_RUNTIME_INDEXES_ENV)
                {
                    self.execute_step_without_index(
                        context,
                        loaded_skill,
                        policy_approval_refs,
                        child_checkpoint,
                    )
                } else {
                    self.execute_step_with_index(
                        context,
                        loaded_skill,
                        policy_approval_refs,
                        child_checkpoint,
                    )
                }
            });
        let run_result = run_result.map_err(|fault| fault.at_graph_step(&context.step.id));
        Ok(match run_result {
            Ok(run) => run,
            Err(StepFault::Sealable(error @ RuntimeError::ResolutionPending { .. })) => {
                return Err(error);
            }
            Err(StepFault::Sealable(error))
                if context.plan.failure_mode == StepFailureMode::RecordAndContinue =>
            {
                runtime_error_step_run(
                    context.runtime,
                    &context.graph.name,
                    context.step,
                    context.plan.attempt,
                    error,
                )?
            }
            Err(fault) => return Err(fault.into_runtime_error()),
        })
    }

    fn execute_step_without_index<A>(
        &mut self,
        context: &mut StepExecutionContext<'_, A>,
        loaded_skill: Option<LoadedStepSkill>,
        policy_approval_refs: Vec<Reference>,
        child_checkpoint: Option<Box<GraphCheckpoint>>,
    ) -> Result<StepRun, StepFault>
    where
        A: SkillAdapter,
    {
        run_step_with_loaded_skill(
            LoadedStepExecutionRequest {
                runtime: context.runtime,
                graph_dir: context.graph_dir,
                graph_name: &context.graph.name,
                step: context.step,
                attempt: context.plan.attempt,
                loaded_skill,
                policy_approval_refs,
                child_checkpoint,
                host: context.host,
            },
            &self.runs,
        )
    }

    fn execute_step_with_index<A>(
        &mut self,
        context: &mut StepExecutionContext<'_, A>,
        loaded_skill: Option<LoadedStepSkill>,
        policy_approval_refs: Vec<Reference>,
        child_checkpoint: Option<Box<GraphCheckpoint>>,
    ) -> Result<StepRun, StepFault>
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
                loaded_skill,
                policy_approval_refs,
                child_checkpoint,
                host: context.host,
            },
            &prior_run_index,
        )
    }

    fn commit_step_run<A>(
        &mut self,
        runtime: &Runtime<A>,
        host: &mut dyn Host,
        plan: StepExecutionPlan<'_>,
        run: StepRun,
        retry_remaining: bool,
    ) -> Result<(), RuntimeError>
    where
        A: SkillAdapter,
    {
        if run.outcome.succeeded() {
            self.succeed_step(runtime, &run);
            self.push_run(run);
            self.record_lifecycle(host, LifecycleEvent::step_completed(plan.step_id))
        } else {
            self.fail_step(runtime, plan.step_id, &run);
            host.log(format!("step {} failed", plan.step_id))?;
            self.record_lifecycle(host, LifecycleEvent::step_failed(plan.step_id))?;
            let terminal =
                plan.failure_mode != StepFailureMode::RecordAndContinue && !retry_remaining;
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
            // list agrees with the journal's StepFailed event; a failed attempt
            // must never be silently absent from the execution record.
            self.push_run(run);
            if terminal {
                Err(RuntimeError::SkillFailed {
                    skill_name: plan.step_id.to_owned(),
                    message,
                })
            } else {
                Ok(())
            }
        }
    }

    fn push_run(&mut self, run: StepRun) {
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
        if self
            .suspended_step
            .as_ref()
            .is_some_and(|suspended| suspended.step_id == step_id)
        {
            self.suspended_step = None;
        } else {
            self.record_lifecycle(host, LifecycleEvent::step_started(step_id))?;
            self.start_step(runtime, step_id);
        }
        self.fail_step(runtime, step_id, &run);
        self.push_run(run);
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
        self.journal.push(event.clone());
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
            sync_points: self.sync_points,
            receipt,
            journal: self.journal,
        }
    }

    pub(super) fn checkpoint(self, graph_name: String) -> GraphCheckpoint {
        GraphCheckpoint {
            graph_name,
            state: self.state,
            steps: self.runs,
            sync_points: self.sync_points,
            journal: self.journal,
            suspended_step: self.suspended_step,
        }
    }

    pub(super) fn checkpoint_error(self, graph_name: &str, error: RuntimeError) -> RuntimeError {
        match error {
            RuntimeError::ResolutionPending {
                step_id, reason, ..
            } => RuntimeError::ResolutionPending {
                step_id,
                reason,
                checkpoint: Some(Box::new(self.checkpoint(graph_name.to_owned()))),
            },
            error => error,
        }
    }

    pub(super) fn record_proceeding_fanout_sync_point(
        &mut self,
        graph: &ExecutionGraph,
        fanout_policies: &BTreeMap<String, FanoutGroupPolicy>,
        group_id: &str,
    ) -> Result<(), RuntimeError> {
        let follow_up =
            self.graph_index
                .plan_transition(&self.state, fanout_policies, self.planning_cursor);
        if matches!(
            follow_up,
            SequentialGraphPlan::RunFanout {
                group_id: ref next_group_id,
                ..
            } if next_group_id == group_id
        ) {
            return Ok(());
        }

        let Some(policy) = fanout_policies.get(group_id) else {
            return Ok(());
        };
        let decision = self.graph_index.fanout_decision(&self.state, policy);
        if decision.decision == FanoutSyncOutcome::Proceed {
            self.push_sync_point(graph, &decision)?;
        }
        Ok(())
    }

    pub(super) fn push_sync_point(
        &mut self,
        graph: &ExecutionGraph,
        decision: &FanoutSyncDecision,
    ) -> Result<(), RuntimeError> {
        let sync_point = fanout_sync_point(
            decision,
            &self.graph_index.fanout_receipt_ids(
                graph,
                &self.runs,
                &self.run_positions,
                &decision.group_id,
            ),
        );
        let already_recorded = self.sync_points.iter().any(|existing| {
            existing.group_id == sync_point.group_id
                && existing.rule_fired == sync_point.rule_fired
                && existing.decision == sync_point.decision
        });
        if !already_recorded {
            self.sync_points.push(sync_point);
        }
        Ok(())
    }

    fn cached_step_skill(
        &mut self,
        runtime: &Runtime<impl SkillAdapter>,
        graph_dir: &Path,
        step: &GraphStep,
    ) -> Result<Option<LoadedStepSkill>, RuntimeError> {
        if step.skill.is_none() {
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
        // / `from_checkpoint`), which is immutable for the run, so the index position
        // map is always in sync with `graph.steps`. The index's `StepMissing` is the
        // authoritative answer for a genuinely-missing step; a linear re-scan over the
        // same `graph.steps` could never find a step the index legitimately missed, it
        // would only silently paper over an index/graph desync. Return the index result
        // directly so such a desync surfaces instead of being absorbed by an O(n) scan.
        self.graph_index.find_step(graph, step_id)
    }
}

fn execute_parallel_fanout_job(
    job: ParallelFanoutJob<'_>,
    context: &ParallelFanoutContext<'_>,
) -> Result<StepRun, RuntimeError> {
    let adapter = BorrowedSkillAdapter::new(job.executor.as_ref());
    let runtime = Runtime::with_native_services(
        adapter,
        context.options.clone(),
        context.javascript.clone(),
        context.local_artifacts.clone(),
    );
    let mut host = RejectingParallelHost;
    match run_step_with_loaded_skill_index(
        LoadedStepExecutionRequest {
            runtime: &runtime,
            graph_dir: context.graph_dir,
            graph_name: context.graph_name,
            step: job.step,
            attempt: job.attempt,
            loaded_skill: job.loaded_skill,
            policy_approval_refs: job.policy_approval_refs,
            child_checkpoint: None,
            host: &mut host,
        },
        context.prior_run_index,
    ) {
        Ok(run) => Ok(run),
        Err(StepFault::Sealable(error)) => {
            runtime_error_step_run(&runtime, context.graph_name, job.step, job.attempt, error)
        }
        Err(StepFault::Fatal(error)) => Err(error),
    }
}

fn join_parallel_fanout_workers(
    handles: Vec<thread::ScopedJoinHandle<'_, Result<(), RuntimeError>>>,
) -> Result<(), RuntimeError> {
    let mut first_error = None;
    for handle in handles {
        let result = handle
            .join()
            .map_err(|_| fanout_worker_error("worker panicked"))
            .and_then(|result| result);
        if first_error.is_none() {
            first_error = result.err();
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}

fn fanout_worker_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::EngineInvariant {
        context: "executing parallel fanout",
        message: message.into(),
    }
}

fn checkpoint_planning_cursor(
    graph: &ExecutionGraph,
    state: &SequentialGraphState,
    sync_points: &[FanoutReceiptSyncPoint],
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
                reason: "checkpoint contains a running step without one matching host suspension"
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
            reason: "pending checkpoint does not identify a running step".to_owned(),
        });
    }
    Ok(terminal_prefix_cursor(graph, state, sync_points, 0))
}

fn terminal_prefix_cursor(
    graph: &ExecutionGraph,
    state: &SequentialGraphState,
    sync_points: &[FanoutReceiptSyncPoint],
    start: usize,
) -> usize {
    let mut cursor = start.min(state.steps.len());
    while let Some(step_state) = state.steps.get(cursor) {
        if !matches!(
            step_state.status,
            GraphStepStatus::Succeeded | GraphStepStatus::Skipped
        ) {
            break;
        }
        let Some(graph_step) = graph
            .steps
            .get(cursor)
            .filter(|step| step.id == step_state.step_id)
        else {
            break;
        };
        if let Some(group_id) = graph_step.fanout_group.as_deref()
            && !sync_points.iter().any(|sync| {
                sync.group_id.as_ref() == group_id
                    && sync.decision == runx_contracts::FanoutReceiptDecision::Proceed
            })
        {
            break;
        }
        cursor += 1;
    }
    cursor
}

fn run_positions(runs: &[StepRun]) -> BTreeMap<String, usize> {
    let mut positions = BTreeMap::new();
    for (index, run) in runs.iter().enumerate() {
        positions.insert(run.step_id.clone(), index);
    }
    positions
}

fn retry_budget_remaining(step: &GraphStep, attempt: u32) -> bool {
    let max_attempts = step.retry.as_ref().map_or(1, |retry| {
        u32::try_from(retry.max_attempts).unwrap_or(u32::MAX)
    });
    attempt < max_attempts
}

pub(super) fn reached_step_limit(
    initial: usize,
    current: usize,
    max_new_steps: Option<usize>,
) -> bool {
    max_new_steps.is_some_and(|max| current.saturating_sub(initial) >= max)
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
        if !matches!(approval_step.run, Some(GraphRunTarget::Approval)) {
            continue;
        }
        let Some(approval_run) = runs
            .iter()
            .rev()
            .find(|run| run.step_id == approval_step_id)
        else {
            continue;
        };
        if approval_run.skill != "run:approval"
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

    use runx_contracts::{FanoutReceiptDecision, FanoutReceiptStrategy, FanoutReceiptSyncPoint};
    use runx_core::state_machine::{
        GraphStepStatus, SequentialGraphStepDefinition, create_sequential_graph_state,
    };
    use runx_parser::{ExecutionGraph, parse_graph_yaml, validate_graph};

    use super::{checkpoint_planning_cursor, terminal_prefix_cursor, when_skipped_steps};

    fn checkpoint_state(
        statuses: &[GraphStepStatus],
    ) -> (
        ExecutionGraph,
        runx_core::state_machine::SequentialGraphState,
    ) {
        let definitions = statuses
            .iter()
            .enumerate()
            .map(|(index, _)| SequentialGraphStepDefinition {
                id: format!("step_{index}"),
                context_from: None,
                retry: None,
                fanout_group: None,
            })
            .collect::<Vec<_>>();
        let mut state = create_sequential_graph_state("graph", &definitions);
        for (step, status) in state.steps.iter_mut().zip(statuses) {
            step.status = status.clone();
        }
        let steps = statuses
            .iter()
            .enumerate()
            .map(|(index, _)| format!("  - id: step_{index}\n    skill: ./noop\n"))
            .collect::<String>();
        let graph = validate_graph(
            parse_graph_yaml(&format!("name: graph\nsteps:\n{steps}"))
                .expect("checkpoint graph should parse"),
        )
        .expect("checkpoint graph should validate");
        (graph, state)
    }

    #[test]
    fn checkpoint_cursor_starts_at_the_first_non_terminal_step() {
        let (graph, state) = checkpoint_state(&[
            GraphStepStatus::Succeeded,
            GraphStepStatus::Skipped,
            GraphStepStatus::Failed,
            GraphStepStatus::Pending,
        ]);

        assert_eq!(
            checkpoint_planning_cursor(&graph, &state, &[], None).expect("valid checkpoint"),
            2
        );
    }

    #[test]
    fn checkpoint_cursor_rejects_running_state_anywhere() {
        let (graph, state) = checkpoint_state(&[
            GraphStepStatus::Succeeded,
            GraphStepStatus::Pending,
            GraphStepStatus::Running,
        ]);

        let error = checkpoint_planning_cursor(&graph, &state, &[], None)
            .expect_err("running checkpoint must fail");
        assert!(
            error
                .to_string()
                .contains("checkpoint contains a running step")
        );
    }

    #[test]
    fn terminal_fanout_stays_at_sync_boundary_until_proceed_is_recorded() {
        let graph = validate_graph(
            parse_graph_yaml(
                r#"
name: checkpoint-fanout
fanout:
  groups:
    workers:
      strategy: all
      on_branch_failure: halt
steps:
  - id: first
    mode: fanout
    fanout_group: workers
    skill: ./noop
  - id: second
    mode: fanout
    fanout_group: workers
    skill: ./noop
  - id: finish
    skill: ./noop
"#,
            )
            .expect("fanout graph should parse"),
        )
        .expect("fanout graph should validate");
        let definitions = graph
            .steps
            .iter()
            .map(|step| SequentialGraphStepDefinition {
                id: step.id.clone(),
                context_from: None,
                retry: None,
                fanout_group: step.fanout_group.clone(),
            })
            .collect::<Vec<_>>();
        let mut state = create_sequential_graph_state(&graph.name, &definitions);
        state.steps[0].status = GraphStepStatus::Succeeded;
        state.steps[1].status = GraphStepStatus::Succeeded;

        assert_eq!(terminal_prefix_cursor(&graph, &state, &[], 0), 0);

        let sync = FanoutReceiptSyncPoint {
            group_id: "workers".into(),
            strategy: FanoutReceiptStrategy::All,
            decision: FanoutReceiptDecision::Proceed,
            rule_fired: "all_succeeded".into(),
            reason: "all branches succeeded".into(),
            branch_count: 2,
            success_count: 2,
            failure_count: 0,
            required_successes: 2,
            branch_receipts: Vec::new(),
            gate: None,
        };
        assert_eq!(terminal_prefix_cursor(&graph, &state, &[sync], 0), 2);
    }

    #[test]
    fn skipped_branch_predicates_propagate_to_unreachable_descendants() {
        let graph = validate_graph(
            parse_graph_yaml(
                r#"
name: conditional-propagation
steps:
  - id: source
    run:
      type: agent-task
      agent: test
      task: source
      outputs: { decision: string }
  - id: inspect
    when: { field: source.decision, equals: ready }
    run:
      type: agent-task
      agent: test
      task: inspect
      outputs: { decision: string }
  - id: reject
    when: { field: inspect.decision, equals: reject }
    run:
      type: agent-task
      agent: test
      task: reject
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
