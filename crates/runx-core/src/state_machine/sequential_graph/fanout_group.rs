use std::collections::BTreeSet;

use super::super::types::{
    FanoutBranchFailurePolicy, FanoutBranchPlan, FanoutGroupPolicy, FanoutSyncDecision,
    FanoutSyncOutcome, FanoutSyncStrategy, GraphStepStatus, SequentialGraphPlan,
    SequentialGraphState, SequentialGraphStepDefinition,
};
use super::fanout_sync::{
    evaluate_sequential_fanout_sync, sequential_fanout_proceeds_without_gates,
};
use super::index::SequentialGraphStepIndex;
use super::step_readiness::{missing_context_at, retry_budget_exhausted};

pub(super) enum FanoutGroupPlan {
    Proceed,
    Plan(Box<SequentialGraphPlan>),
}

enum FanoutCandidatePlan {
    Plan(Box<SequentialGraphPlan>),
    ProceedToSync,
}

enum NonProceedFanoutDecision {
    Halt(FanoutSyncDecision),
    Pause(FanoutSyncDecision),
    Escalate(FanoutSyncDecision),
}

pub(super) fn plan_fanout_group(
    state: &SequentialGraphState,
    step_index: &SequentialGraphStepIndex,
    start_index: usize,
    definitions: &[SequentialGraphStepDefinition],
    group_steps: &[SequentialGraphStepDefinition],
    policy: Option<&FanoutGroupPolicy>,
    resolved_fanout_gate_keys: Option<&BTreeSet<String>>,
) -> FanoutGroupPlan {
    let Some(first_step) = group_steps.first() else {
        return FanoutGroupPlan::Plan(Box::new(SequentialGraphPlan::Failed {
            step_id: "unknown".to_owned(),
            reason: "fanout group is empty".to_owned(),
            sync_decision: None,
        }));
    };
    let Some(group_id) = fanout_group_id(first_step) else {
        return FanoutGroupPlan::Plan(Box::new(SequentialGraphPlan::Failed {
            step_id: first_step.id.clone(),
            reason: "fanout group is empty".to_owned(),
            sync_decision: None,
        }));
    };

    match plan_fanout_candidates(state, step_index, start_index, group_steps, group_id) {
        FanoutCandidatePlan::Plan(plan) => return FanoutGroupPlan::Plan(plan),
        FanoutCandidatePlan::ProceedToSync => {}
    }

    let default_policy;
    let fanout_policy = match policy {
        Some(policy) => policy,
        None => {
            default_policy = default_fanout_policy(group_id);
            &default_policy
        }
    };
    if sequential_fanout_proceeds_without_gates(state, step_index, fanout_policy) == Some(true) {
        return FanoutGroupPlan::Proceed;
    }
    let decision = evaluate_sequential_fanout_sync(
        state,
        definitions,
        step_index,
        fanout_policy,
        resolved_fanout_gate_keys,
    );
    let Some(non_proceed_decision) = non_proceed_fanout_decision(decision) else {
        return FanoutGroupPlan::Proceed;
    };

    FanoutGroupPlan::Plan(Box::new(sync_decision_plan(
        first_step,
        non_proceed_decision,
    )))
}

fn plan_fanout_candidates(
    state: &SequentialGraphState,
    step_index: &SequentialGraphStepIndex,
    start_index: usize,
    group_steps: &[SequentialGraphStepDefinition],
    group_id: &str,
) -> FanoutCandidatePlan {
    let mut branches = Vec::with_capacity(group_steps.len());

    for (offset, step_definition) in group_steps.iter().enumerate() {
        let definition_index = start_index + offset;
        let Some(step_state) = step_index.state_at(state, definition_index, &step_definition.id)
        else {
            return FanoutCandidatePlan::Plan(Box::new(SequentialGraphPlan::Failed {
                step_id: step_definition.id.clone(),
                reason: "step state is missing".to_owned(),
                sync_decision: None,
            }));
        };
        if step_state.status == GraphStepStatus::Running {
            return FanoutCandidatePlan::Plan(Box::new(SequentialGraphPlan::Blocked {
                step_id: step_definition.id.clone(),
                reason: "step is already running".to_owned(),
                sync_decision: None,
            }));
        }
        // A succeeded branch is done and a `when`-skipped branch is selected
        // out. Both are terminal for the group, exactly as the non-fanout
        // walker treats them, so neither is re-planned as a live branch.
        if matches!(
            step_state.status,
            GraphStepStatus::Succeeded | GraphStepStatus::Skipped
        ) || retry_budget_exhausted(step_state, step_definition)
        {
            continue;
        }
        if let Some(missing_context) =
            missing_context_at(state, step_index, definition_index, step_definition)
        {
            return FanoutCandidatePlan::Plan(Box::new(SequentialGraphPlan::Blocked {
                step_id: step_definition.id.clone(),
                reason: format!("waiting for context from {missing_context}"),
                sync_decision: None,
            }));
        }
        branches.push(FanoutBranchPlan {
            step_id: step_definition.id.clone(),
            attempt: step_state.attempts + 1,
            context_from: step_definition.context_from.clone().unwrap_or_default(),
        });
    }

    if branches.is_empty() {
        FanoutCandidatePlan::ProceedToSync
    } else {
        FanoutCandidatePlan::Plan(Box::new(SequentialGraphPlan::RunFanout {
            group_id: group_id.to_owned(),
            branches,
        }))
    }
}

fn sync_decision_plan(
    first_step: &SequentialGraphStepDefinition,
    decision: NonProceedFanoutDecision,
) -> SequentialGraphPlan {
    match decision {
        NonProceedFanoutDecision::Halt(decision) => SequentialGraphPlan::Failed {
            step_id: first_step.id.clone(),
            reason: decision.reason.clone(),
            sync_decision: Some(decision),
        },
        NonProceedFanoutDecision::Pause(decision) => SequentialGraphPlan::Paused {
            step_id: first_step.id.clone(),
            reason: decision.reason.clone(),
            sync_decision: decision,
        },
        NonProceedFanoutDecision::Escalate(decision) => SequentialGraphPlan::Escalated {
            step_id: first_step.id.clone(),
            reason: decision.reason.clone(),
            sync_decision: decision,
        },
    }
}

fn non_proceed_fanout_decision(decision: FanoutSyncDecision) -> Option<NonProceedFanoutDecision> {
    match decision.decision {
        FanoutSyncOutcome::Proceed => None,
        FanoutSyncOutcome::Halt => Some(NonProceedFanoutDecision::Halt(decision)),
        FanoutSyncOutcome::Pause => Some(NonProceedFanoutDecision::Pause(decision)),
        FanoutSyncOutcome::Escalate => Some(NonProceedFanoutDecision::Escalate(decision)),
    }
}

pub(super) fn contiguous_fanout_group<'a>(
    steps: &'a [SequentialGraphStepDefinition],
    start_index: usize,
    group_id: &str,
) -> &'a [SequentialGraphStepDefinition] {
    let mut end_index = start_index;
    while end_index < steps.len() && fanout_group_id(&steps[end_index]) == Some(group_id) {
        end_index += 1;
    }
    &steps[start_index..end_index]
}

pub(super) fn fanout_group_id(step: &SequentialGraphStepDefinition) -> Option<&str> {
    step.fanout_group
        .as_deref()
        .filter(|group_id| !group_id.is_empty())
}

fn default_fanout_policy(group_id: &str) -> FanoutGroupPolicy {
    FanoutGroupPolicy {
        group_id: group_id.to_owned(),
        strategy: FanoutSyncStrategy::All,
        min_success: None,
        on_branch_failure: FanoutBranchFailurePolicy::Halt,
        threshold_gates: Some(Vec::new()),
        conflict_gates: Some(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::state_machine::{
        GraphStepStatus, SequentialGraphPlan, SequentialGraphState, SequentialGraphStepDefinition,
        create_sequential_graph_state, plan_sequential_graph_transition,
    };

    fn branch(id: &str, context_from: Option<Vec<String>>) -> SequentialGraphStepDefinition {
        SequentialGraphStepDefinition {
            id: id.to_owned(),
            context_from,
            retry: None,
            fanout_group: Some("workers".to_owned()),
        }
    }

    fn plain(id: &str) -> SequentialGraphStepDefinition {
        SequentialGraphStepDefinition {
            id: id.to_owned(),
            context_from: None,
            retry: None,
            fanout_group: None,
        }
    }

    fn state_with(
        definitions: &[SequentialGraphStepDefinition],
        statuses: &[GraphStepStatus],
    ) -> SequentialGraphState {
        let mut state = create_sequential_graph_state("graph", definitions);
        for (step, status) in state.steps.iter_mut().zip(statuses) {
            step.status = status.clone();
        }
        state
    }

    #[test]
    fn selected_out_fanout_branch_is_not_replanned() {
        let definitions = vec![
            plain("router"),
            branch("branch_a", None),
            branch("branch_b", None),
        ];
        let state = state_with(
            &definitions,
            &[
                GraphStepStatus::Succeeded,
                GraphStepStatus::Succeeded,
                GraphStepStatus::Skipped,
            ],
        );

        let plan = plan_sequential_graph_transition(&state, &definitions, &BTreeMap::new(), None);

        assert_eq!(plan, SequentialGraphPlan::Complete);
    }

    #[test]
    fn selected_out_branch_is_not_replanned_for_its_context() {
        let definitions = vec![
            plain("router"),
            branch("branch_a", None),
            branch("branch_b", Some(vec!["router".to_owned()])),
        ];
        let state = state_with(
            &definitions,
            &[
                GraphStepStatus::Skipped,
                GraphStepStatus::Succeeded,
                GraphStepStatus::Skipped,
            ],
        );

        let plan = plan_sequential_graph_transition(&state, &definitions, &BTreeMap::new(), None);

        assert_eq!(plan, SequentialGraphPlan::Complete);
    }
}
