use std::collections::BTreeSet;

use super::super::fanout::{
    evaluate_fanout_sync, evaluate_fanout_sync_without_gates, gate_free_fanout_outcome,
};
use super::super::types::{
    FanoutBranchResult, FanoutGroupPolicy, FanoutSyncDecision, FanoutSyncOutcome, GraphStepStatus,
    SequentialGraphState, SequentialGraphStepDefinition,
};
use super::index::SequentialGraphStepIndex;

/// Evaluate one indexed fanout group from canonical graph state.
///
/// Planning, execution receipts, and performance probes all use this owner so
/// branch selection and output inclusion cannot drift between paths.
#[must_use]
pub fn evaluate_sequential_fanout_sync(
    state: &SequentialGraphState,
    definitions: &[SequentialGraphStepDefinition],
    step_index: &SequentialGraphStepIndex,
    policy: &FanoutGroupPolicy,
    resolved_gate_keys: Option<&BTreeSet<String>>,
) -> FanoutSyncDecision {
    let include_outputs = policy_requires_outputs(policy);
    let positions = step_index
        .fanout_positions(&policy.group_id)
        .unwrap_or_default();
    if !include_outputs {
        let counts = gate_free_counts(state, positions);
        return evaluate_fanout_sync_without_gates(
            policy,
            counts.branch_count,
            counts.success_count,
            counts.failure_count,
        );
    }
    let results = step_index
        .fanout_positions(&policy.group_id)
        .into_iter()
        .flatten()
        .filter_map(|position| {
            definitions
                .get(*position)
                .map(|definition| (*position, definition))
        })
        .map(|(position, definition)| {
            let step = state.steps.get(position);
            FanoutBranchResult {
                step_id: definition.id.clone(),
                status: step.map_or(GraphStepStatus::Failed, |step| step.status.clone()),
                outputs: step.and_then(|step| step.outputs.clone()),
            }
        })
        .filter(|result| !is_selected_out(&result.status))
        .collect::<Vec<_>>();
    evaluate_fanout_sync(policy, &results, resolved_gate_keys)
}

pub(super) fn sequential_fanout_proceeds_without_gates(
    state: &SequentialGraphState,
    step_index: &SequentialGraphStepIndex,
    policy: &FanoutGroupPolicy,
) -> Option<bool> {
    if policy_requires_outputs(policy) {
        return None;
    }
    let positions = step_index
        .fanout_positions(&policy.group_id)
        .unwrap_or_default();
    let counts = gate_free_counts(state, positions);
    Some(
        gate_free_fanout_outcome(
            policy,
            counts.branch_count,
            counts.success_count,
            counts.failure_count,
        ) == FanoutSyncOutcome::Proceed,
    )
}

struct GateFreeCounts {
    branch_count: usize,
    success_count: usize,
    failure_count: usize,
}

/// Count the branches that participate in the group's sync decision.
///
/// A `when`-skipped branch was selected out before it ran, so it is not a
/// participating branch: counting it would make `all` unsatisfiable and shift
/// every quorum denominator for a branch the graph deliberately never ran.
fn gate_free_counts(state: &SequentialGraphState, positions: &[usize]) -> GateFreeCounts {
    let mut counts = GateFreeCounts {
        branch_count: 0,
        success_count: 0,
        failure_count: 0,
    };
    for position in positions {
        match state.steps.get(*position).map(|step| &step.status) {
            Some(GraphStepStatus::Succeeded) => {
                counts.branch_count += 1;
                counts.success_count += 1;
            }
            Some(GraphStepStatus::Failed) | None => {
                counts.branch_count += 1;
                counts.failure_count += 1;
            }
            Some(GraphStepStatus::Pending | GraphStepStatus::Running) => {
                counts.branch_count += 1;
            }
            Some(GraphStepStatus::Skipped) => {}
        }
    }
    counts
}

fn is_selected_out(status: &GraphStepStatus) -> bool {
    *status == GraphStepStatus::Skipped
}

fn policy_requires_outputs(policy: &FanoutGroupPolicy) -> bool {
    policy
        .threshold_gates
        .as_ref()
        .is_some_and(|gates| !gates.is_empty())
        || policy
            .conflict_gates
            .as_ref()
            .is_some_and(|gates| !gates.is_empty())
}

#[cfg(test)]
mod tests {
    use super::evaluate_sequential_fanout_sync;
    use crate::state_machine::{
        FanoutBranchFailurePolicy, FanoutConflictGate, FanoutGateAction, FanoutGroupPolicy,
        FanoutSyncOutcome, FanoutSyncStrategy, GraphStepStatus, SequentialGraphStepDefinition,
        create_sequential_graph_state, create_sequential_graph_step_index,
    };

    #[test]
    fn missing_indexed_branch_state_fails_closed() {
        let definitions = ["first", "second"]
            .into_iter()
            .map(|id| SequentialGraphStepDefinition {
                id: id.to_owned(),
                context_from: None,
                retry: None,
                fanout_group: Some("workers".to_owned()),
            })
            .collect::<Vec<_>>();
        let index = create_sequential_graph_step_index(&definitions);
        let mut state = create_sequential_graph_state("graph", &definitions);
        state.steps[0].status = GraphStepStatus::Succeeded;
        state.steps.pop();
        let policy = FanoutGroupPolicy {
            group_id: "workers".to_owned(),
            strategy: FanoutSyncStrategy::All,
            min_success: None,
            on_branch_failure: FanoutBranchFailurePolicy::Continue,
            threshold_gates: None,
            conflict_gates: None,
        };

        let decision = evaluate_sequential_fanout_sync(&state, &definitions, &index, &policy, None);

        assert_eq!(decision.decision, FanoutSyncOutcome::Halt);
        assert_eq!(decision.branch_count, 2);
        assert_eq!(decision.success_count, 1);
        assert_eq!(decision.failure_count, 1);
    }

    #[test]
    fn selected_out_branches_leave_the_group_branch_population() {
        for conflict_gates in [
            None,
            Some(vec![FanoutConflictGate {
                field: "verdict".to_owned(),
                steps: Vec::new(),
                action: FanoutGateAction::Escalate,
            }]),
        ] {
            let definitions = ["first", "second", "third"]
                .into_iter()
                .map(|id| SequentialGraphStepDefinition {
                    id: id.to_owned(),
                    context_from: None,
                    retry: None,
                    fanout_group: Some("workers".to_owned()),
                })
                .collect::<Vec<_>>();
            let index = create_sequential_graph_step_index(&definitions);
            let mut state = create_sequential_graph_state("graph", &definitions);
            state.steps[0].status = GraphStepStatus::Succeeded;
            state.steps[1].status = GraphStepStatus::Succeeded;
            state.steps[2].status = GraphStepStatus::Skipped;
            let policy = FanoutGroupPolicy {
                group_id: "workers".to_owned(),
                strategy: FanoutSyncStrategy::All,
                min_success: None,
                on_branch_failure: FanoutBranchFailurePolicy::Halt,
                threshold_gates: None,
                conflict_gates,
            };

            let decision =
                evaluate_sequential_fanout_sync(&state, &definitions, &index, &policy, None);

            assert_eq!(decision.decision, FanoutSyncOutcome::Proceed);
            assert_eq!(decision.branch_count, 2);
            assert_eq!(decision.success_count, 2);
            assert_eq!(decision.required_successes, 2);
        }
    }

    #[test]
    fn selection_does_not_reduce_explicit_success_requirements() {
        for (strategy, minimum, statuses, expected) in [
            (
                FanoutSyncStrategy::All,
                None,
                [GraphStepStatus::Skipped, GraphStepStatus::Skipped],
                FanoutSyncOutcome::Proceed,
            ),
            (
                FanoutSyncStrategy::Any,
                None,
                [GraphStepStatus::Skipped, GraphStepStatus::Skipped],
                FanoutSyncOutcome::Halt,
            ),
            (
                FanoutSyncStrategy::Quorum,
                Some(1),
                [GraphStepStatus::Skipped, GraphStepStatus::Skipped],
                FanoutSyncOutcome::Halt,
            ),
            (
                FanoutSyncStrategy::Quorum,
                Some(2),
                [GraphStepStatus::Succeeded, GraphStepStatus::Skipped],
                FanoutSyncOutcome::Halt,
            ),
            (
                FanoutSyncStrategy::Any,
                None,
                [GraphStepStatus::Succeeded, GraphStepStatus::Skipped],
                FanoutSyncOutcome::Proceed,
            ),
            (
                FanoutSyncStrategy::All,
                None,
                [GraphStepStatus::Failed, GraphStepStatus::Skipped],
                FanoutSyncOutcome::Halt,
            ),
            (
                FanoutSyncStrategy::All,
                None,
                [GraphStepStatus::Pending, GraphStepStatus::Skipped],
                FanoutSyncOutcome::Halt,
            ),
        ] {
            for gated in [false, true] {
                let definitions = ["first", "second"]
                    .into_iter()
                    .map(|id| SequentialGraphStepDefinition {
                        id: id.to_owned(),
                        context_from: None,
                        retry: None,
                        fanout_group: Some("workers".to_owned()),
                    })
                    .collect::<Vec<_>>();
                let index = create_sequential_graph_step_index(&definitions);
                let mut state = create_sequential_graph_state("graph", &definitions);
                for (step, status) in state.steps.iter_mut().zip(&statuses) {
                    step.status = status.clone();
                }
                let policy = FanoutGroupPolicy {
                    group_id: "workers".to_owned(),
                    strategy: strategy.clone(),
                    min_success: minimum,
                    on_branch_failure: FanoutBranchFailurePolicy::Halt,
                    threshold_gates: None,
                    conflict_gates: gated.then(|| {
                        vec![FanoutConflictGate {
                            field: "verdict".to_owned(),
                            steps: Vec::new(),
                            action: FanoutGateAction::Escalate,
                        }]
                    }),
                };
                let decision =
                    evaluate_sequential_fanout_sync(&state, &definitions, &index, &policy, None);
                assert_eq!(
                    decision.decision, expected,
                    "strategy={strategy:?}, statuses={statuses:?}, gated={gated}"
                );
                if let Some(minimum) = minimum {
                    assert_eq!(decision.required_successes, minimum as usize);
                }
            }
        }
    }
}
