use super::super::types::{
    GraphStepState, SequentialGraph, SequentialGraphStep, SequentialGraphStepDefinition,
};
use super::index::SequentialGraphStepIndex;

pub(super) fn step_failed(step: &SequentialGraphStep) -> bool {
    step.state == GraphStepState::Failed
}

fn missing_context(
    graph: &SequentialGraph,
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
                .step_for(graph, step_id)
                .is_none_or(|step| step.state != GraphStepState::Succeeded)
        })
        .cloned()
}

pub(super) fn missing_context_at(
    graph: &SequentialGraph,
    step_index: &SequentialGraphStepIndex,
    definition_index: usize,
    step_definition: &SequentialGraphStepDefinition,
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
                .is_none_or(|step| step.state != GraphStepState::Succeeded)
        })
        .map(|source| source.step_id.clone())
}
