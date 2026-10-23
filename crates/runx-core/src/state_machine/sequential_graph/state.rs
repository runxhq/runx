use super::super::types::{
    GraphState, GraphStepState, SequentialGraph, SequentialGraphStep, SequentialGraphStepDefinition,
};

#[must_use]
pub fn create_sequential_graph(
    graph_id: impl Into<String>,
    steps: &[SequentialGraphStepDefinition],
) -> SequentialGraph {
    SequentialGraph {
        graph_id: graph_id.into(),
        state: GraphState::Pending,
        steps: steps
            .iter()
            .map(|step| SequentialGraphStep {
                step_id: step.id.clone(),
                state: GraphStepState::Pending,
                attempts: 0,
                started_at: None,
                completed_at: None,
                receipt_id: None,
                outputs: None,
                error: None,
            })
            .collect(),
    }
}
