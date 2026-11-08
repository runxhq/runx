use super::super::types::{
    SequentialWorkflow, SequentialWorkflowStep, SequentialWorkflowStepDefinition, WorkflowStepState,
};
use super::index::SequentialWorkflowStepIndex;

pub(super) fn step_failed(step: &SequentialWorkflowStep) -> bool {
    step.state == WorkflowStepState::Failed
}

fn missing_context(
    graph: &SequentialWorkflow,
    step_index: &SequentialWorkflowStepIndex,
    step_definition: &SequentialWorkflowStepDefinition,
) -> Option<String> {
    step_definition
        .context_from
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|step_id| {
            step_index
                .step_for(graph, step_id)
                .is_none_or(|step| step.state != WorkflowStepState::Succeeded)
        })
        .cloned()
}

pub(super) fn missing_context_at(
    graph: &SequentialWorkflow,
    step_index: &SequentialWorkflowStepIndex,
    definition_index: usize,
    step_definition: &SequentialWorkflowStepDefinition,
) -> Option<String> {
    let Some(context_sources) = step_index.context_sources_at(definition_index) else {
        return missing_context(graph, step_index, step_definition);
    };
    if context_sources.is_empty() {
        return None;
    }
    context_sources
        .iter()
        .find(|source| {
            source
                .position
                .and_then(|position| graph.steps.get(position))
                .filter(|step| step.step_id == source.step_id)
                .is_none_or(|step| step.state != WorkflowStepState::Succeeded)
        })
        .map(|source| source.step_id.clone())
}
