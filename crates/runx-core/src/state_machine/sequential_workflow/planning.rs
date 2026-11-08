use super::super::types::{
    SequentialWorkflow, SequentialWorkflowPlan, SequentialWorkflowStepDefinition, WorkflowStepState,
};
use super::index::SequentialWorkflowStepIndex;
use super::step_readiness::{missing_context_at, step_failed};

#[must_use]
pub fn plan_sequential_workflow_transition(
    graph: &SequentialWorkflow,
    steps: &[SequentialWorkflowStepDefinition],
) -> SequentialWorkflowPlan {
    let step_index = SequentialWorkflowStepIndex::new(steps);
    plan_sequential_workflow_transition_indexed(graph, steps, &step_index)
}

#[must_use]
pub fn plan_sequential_workflow_transition_indexed(
    graph: &SequentialWorkflow,
    steps: &[SequentialWorkflowStepDefinition],
    step_index: &SequentialWorkflowStepIndex,
) -> SequentialWorkflowPlan {
    if let Some(running_step) = graph
        .steps
        .iter()
        .find(|step| step.state == WorkflowStepState::Running)
    {
        return SequentialWorkflowPlan::Blocked {
            step_id: running_step.step_id.clone(),
            reason: "step is already running".to_owned(),
        };
    }

    plan_sequential_workflow_transition_indexed_from(graph, steps, step_index, 0)
}

/// Plan from a cursor whose preceding steps are known to be terminal.
///
/// Runtime executors maintain this cursor as steps complete, avoiding a fresh
/// scan of the terminal prefix for every transition. Callers that do not own
/// that invariant must use [`plan_sequential_workflow_transition_indexed`].
#[must_use]
pub fn plan_sequential_workflow_transition_indexed_from(
    graph: &SequentialWorkflow,
    steps: &[SequentialWorkflowStepDefinition],
    step_index: &SequentialWorkflowStepIndex,
    start_index: usize,
) -> SequentialWorkflowPlan {
    debug_assert!(graph.steps.iter().take(start_index).all(|step| matches!(
        step.state,
        WorkflowStepState::Succeeded | WorkflowStepState::Skipped
    )));

    for (index, step_definition) in steps.iter().enumerate().skip(start_index) {
        if let Some(plan) = plan_step(graph, step_index, index, step_definition) {
            return plan;
        }
    }
    SequentialWorkflowPlan::Complete
}

fn plan_step(
    graph: &SequentialWorkflow,
    step_index: &SequentialWorkflowStepIndex,
    definition_index: usize,
    step_definition: &SequentialWorkflowStepDefinition,
) -> Option<SequentialWorkflowPlan> {
    let Some(step) = step_index.step_at(graph, definition_index, &step_definition.id) else {
        return Some(SequentialWorkflowPlan::Failed {
            step_id: step_definition.id.clone(),
            reason: "step state is missing".to_owned(),
        });
    };

    if step.state == WorkflowStepState::Running {
        return Some(SequentialWorkflowPlan::Blocked {
            step_id: step_definition.id.clone(),
            reason: "step is already running".to_owned(),
        });
    }

    // A succeeded step is done; a `when`-skipped step is selected out. Both are
    // terminal, so the forward walker moves past them to the next live step.
    if matches!(
        step.state,
        WorkflowStepState::Succeeded | WorkflowStepState::Skipped
    ) {
        return None;
    }
    if step_failed(step) {
        return Some(SequentialWorkflowPlan::Failed {
            step_id: step_definition.id.clone(),
            reason: "step failed".to_owned(),
        });
    }
    if let Some(missing_context) =
        missing_context_at(graph, step_index, definition_index, step_definition)
    {
        return Some(SequentialWorkflowPlan::Blocked {
            step_id: step_definition.id.clone(),
            reason: format!("waiting for context from {missing_context}"),
        });
    }

    Some(SequentialWorkflowPlan::RunStep {
        step_id: step_definition.id.clone(),
        attempt: step.attempts + 1,
        context_from: step_definition.context_from.clone().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        plan_sequential_workflow_transition_indexed,
        plan_sequential_workflow_transition_indexed_from,
    };
    use crate::state_machine::{
        SequentialWorkflowStepDefinition, WorkflowStepState, create_sequential_workflow,
        create_sequential_workflow_step_index,
    };

    #[test]
    fn terminal_prefix_cursor_preserves_the_canonical_plan() {
        let definitions = (0..3)
            .map(|index| SequentialWorkflowStepDefinition {
                id: format!("step_{index}"),
                context_from: None,
            })
            .collect::<Vec<_>>();
        let index = create_sequential_workflow_step_index(&definitions);
        let mut graph = create_sequential_workflow("graph", &definitions);
        graph.steps[0].state = WorkflowStepState::Succeeded;

        let canonical = plan_sequential_workflow_transition_indexed(&graph, &definitions, &index);
        let from_cursor =
            plan_sequential_workflow_transition_indexed_from(&graph, &definitions, &index, 1);

        assert_eq!(from_cursor, canonical);
    }
}
