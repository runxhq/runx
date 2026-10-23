use std::collections::BTreeMap;

use super::super::types::{SequentialGraph, SequentialGraphStep, SequentialGraphStepDefinition};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SequentialGraphStepIndex {
    positions: BTreeMap<String, usize>,
    context_positions: Vec<Vec<ContextSourcePosition>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ContextSourcePosition {
    pub(super) step_id: String,
    pub(super) position: Option<usize>,
}

impl SequentialGraphStepIndex {
    #[must_use]
    pub fn new(steps: &[SequentialGraphStepDefinition]) -> Self {
        let positions = steps
            .iter()
            .enumerate()
            .map(|(index, step)| (step.id.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let context_positions = steps
            .iter()
            .map(|step| {
                step.context_from
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|step_id| ContextSourcePosition {
                        step_id: step_id.clone(),
                        position: positions.get(step_id).copied(),
                    })
                    .collect()
            })
            .collect();
        Self {
            positions,
            context_positions,
        }
    }

    pub(super) fn step_for<'a>(
        &self,
        graph: &'a SequentialGraph,
        step_id: &str,
    ) -> Option<&'a SequentialGraphStep> {
        self.positions
            .get(step_id)
            .and_then(|index| graph.steps.get(*index))
            .filter(|step| step.step_id == step_id)
    }

    pub(super) fn position(&self, step_id: &str) -> Option<usize> {
        self.positions.get(step_id).copied()
    }

    pub(super) fn step_at<'a>(
        &self,
        graph: &'a SequentialGraph,
        position: usize,
        step_id: &str,
    ) -> Option<&'a SequentialGraphStep> {
        graph
            .steps
            .get(position)
            .filter(|step| step.step_id == step_id)
            .or_else(|| self.step_for(graph, step_id))
    }

    pub(super) fn context_sources_at(
        &self,
        definition_index: usize,
    ) -> Option<&[ContextSourcePosition]> {
        self.context_positions
            .get(definition_index)
            .map(Vec::as_slice)
    }
}

#[must_use]
pub fn create_sequential_graph_step_index(
    steps: &[SequentialGraphStepDefinition],
) -> SequentialGraphStepIndex {
    SequentialGraphStepIndex::new(steps)
}
