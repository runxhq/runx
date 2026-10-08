use super::super::types::{
    GraphStepStatus, SequentialGraphState, SequentialGraphStepDefinition, SequentialGraphStepState,
};
use super::index::SequentialGraphStepIndex;

pub(super) fn step_failed(step_state: &SequentialGraphStepState) -> bool {
    step_state.status == GraphStepStatus::Failed
}

fn missing_context(
    state: &SequentialGraphState,
    step_index: &SequentialGraphStepIndex,
    step_definition: &SequentialGraphStepDefinition,
) -> Option<String> {
    step_definition
        .context_from
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|step_id| {
            step_index
                .state_for(state, step_id)
                .is_none_or(|step| step.status != GraphStepStatus::Succeeded)
        })
        .cloned()
}

pub(super) fn missing_context_at(
    state: &SequentialGraphState,
    step_index: &SequentialGraphStepIndex,
    definition_index: usize,
    step_definition: &SequentialGraphStepDefinition,
) -> Option<String> {
    let Some(context_sources) = step_index.context_sources_at(definition_index) else {
        return missing_context(state, step_index, step_definition);
    };
    if context_sources.is_empty() {
        return None;
    }
    context_sources
        .iter()
        .find(|source| {
            source
                .position
                .and_then(|position| state.steps.get(position))
                .filter(|step| step.step_id == source.step_id)
                .is_none_or(|step| step.status != GraphStepStatus::Succeeded)
        })
        .map(|source| source.step_id.clone())
}
