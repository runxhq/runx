use std::collections::BTreeMap;

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{JsonObject, JsonValue, ProvenanceEntry};
use runx_core::state_machine::{
    SequentialGraphEvent, SequentialGraphPlan, SequentialGraphState, SequentialGraphStepDefinition,
    SequentialGraphStepIndex, apply_sequential_graph_event_owned_indexed,
    create_sequential_graph_step_index, plan_sequential_graph_transition_indexed_from,
    start_sequential_graph_step_indexed, succeed_sequential_graph_step_indexed,
};
use runx_parser::{ExecutionGraph, GraphStep};

use crate::{RuntimeError, StepRun};

pub(crate) struct ExecutionGraphIndex {
    definitions: Vec<SequentialGraphStepDefinition>,
    planner_index: SequentialGraphStepIndex,
    step_positions: StepPositionIndex,
}

struct StepPositionIndex {
    positions: BTreeMap<String, usize>,
}

impl StepPositionIndex {
    fn new() -> Self {
        Self {
            positions: BTreeMap::new(),
        }
    }

    fn insert(&mut self, step_id: &str, index: usize) {
        self.positions.insert(step_id.to_owned(), index);
    }

    fn position(&self, step_id: &str) -> Option<usize> {
        self.positions.get(step_id).copied()
    }
}

impl ExecutionGraphIndex {
    #[must_use]
    pub(crate) fn new(
        graph: &ExecutionGraph,
        definitions: Vec<SequentialGraphStepDefinition>,
    ) -> Self {
        let planner_index = create_sequential_graph_step_index(&definitions);
        let mut step_positions = StepPositionIndex::new();
        for (index, step) in graph.steps.iter().enumerate() {
            step_positions.insert(&step.id, index);
        }
        Self {
            definitions,
            planner_index,
            step_positions,
        }
    }

    pub(crate) fn plan_transition(
        &self,
        state: &SequentialGraphState,
        start_index: usize,
    ) -> SequentialGraphPlan {
        plan_sequential_graph_transition_indexed_from(
            state,
            &self.definitions,
            &self.planner_index,
            start_index,
        )
    }

    pub(crate) fn apply_event(
        &self,
        state: &mut SequentialGraphState,
        event: SequentialGraphEvent,
    ) {
        apply_sequential_graph_event_owned_indexed(state, event, &self.planner_index);
    }

    pub(crate) fn start_step(&self, state: &mut SequentialGraphState, step_id: &str, at: String) {
        start_sequential_graph_step_indexed(state, step_id, at, &self.planner_index);
    }

    pub(crate) fn succeed_step(
        &self,
        state: &mut SequentialGraphState,
        at: String,
        admission_witness: runx_core::state_machine::StepAdmissionWitness,
        outputs: Option<JsonObject>,
    ) {
        succeed_sequential_graph_step_indexed(
            state,
            at,
            admission_witness,
            outputs,
            &self.planner_index,
        );
    }

    pub(crate) fn find_step<'a>(
        &self,
        graph: &'a ExecutionGraph,
        step_id: &str,
    ) -> Result<&'a GraphStep, RuntimeError> {
        graph
            .steps
            .get(self.step_positions.position(step_id).ok_or_else(|| {
                RuntimeError::StepMissing {
                    step_id: step_id.to_owned(),
                }
            })?)
            .filter(|step| step.id == step_id)
            .ok_or_else(|| RuntimeError::StepMissing {
                step_id: step_id.to_owned(),
            })
    }
}

pub(crate) struct PriorRunIndex<'a> {
    runs: BTreeMap<&'a str, &'a StepRun>,
}

impl<'a> PriorRunIndex<'a> {
    #[must_use]
    pub(crate) fn from_positions(
        prior_runs: &'a [StepRun],
        positions: &'a BTreeMap<String, usize>,
    ) -> Self {
        Self {
            runs: positions
                .iter()
                .filter_map(|(step_id, index)| {
                    prior_runs
                        .get(*index)
                        .map(|run| (step_id.as_str(), run))
                        .filter(|(_, run)| run.step_id == *step_id)
                })
                .collect(),
        }
    }

    pub(crate) fn output(
        &self,
        to_step: &str,
        input: &str,
        from_step: &str,
        output: &str,
    ) -> Result<JsonValue, RuntimeError> {
        let Some(run) = self.runs.get(from_step) else {
            return Err(RuntimeError::GraphBlocked {
                step_id: from_step.to_owned(),
                reason: "context source step has not run".to_owned(),
            });
        };
        resolve_output_path(&run.contract, output).map_err(|break_at| {
            RuntimeError::context_edge_unresolved(
                to_step,
                input,
                from_step,
                output,
                break_at.missing_segment,
                break_at.available_keys,
            )
        })
    }

    pub(crate) fn provenance(
        &self,
        to_step: &str,
        input: &str,
        from_step: &str,
        output: &str,
    ) -> Result<ProvenanceEntry, RuntimeError> {
        let run = self
            .runs
            .get(from_step)
            .ok_or_else(|| RuntimeError::GraphBlocked {
                step_id: from_step.to_owned(),
                reason: "context source step has not run".to_owned(),
            })?;
        let input =
            NonEmptyString::new(input.to_owned()).ok_or_else(|| RuntimeError::InvalidRunStep {
                step_id: to_step.to_owned(),
                reason: "context edge input must not be empty".to_owned(),
            })?;
        let output =
            NonEmptyString::new(output.to_owned()).ok_or_else(|| RuntimeError::InvalidRunStep {
                step_id: to_step.to_owned(),
                reason: "context edge output must not be empty".to_owned(),
            })?;
        Ok(ProvenanceEntry {
            input,
            output,
            from_step: Some(from_step.to_owned()),
            artifact_id: None,
            receipt_id: Some(run.receipt.id.to_string()),
        })
    }
}

/// Where a context-edge path stopped resolving: the segment that was absent and the keys
/// that were available at that depth (empty when the value there was not an object).
pub(crate) struct ContextPathBreak {
    pub(crate) missing_segment: String,
    pub(crate) available_keys: Vec<String>,
}

pub(crate) fn resolve_output_path(
    outputs: &JsonObject,
    output: &str,
) -> Result<JsonValue, ContextPathBreak> {
    let mut segments = output.split('.');
    let Some(first) = segments.next() else {
        return Err(ContextPathBreak {
            missing_segment: String::new(),
            available_keys: outputs.keys().cloned().collect(),
        });
    };
    let mut value = outputs.get(first).ok_or_else(|| ContextPathBreak {
        missing_segment: first.to_owned(),
        available_keys: outputs.keys().cloned().collect(),
    })?;
    for segment in segments {
        let JsonValue::Object(object) = value else {
            return Err(ContextPathBreak {
                missing_segment: segment.to_owned(),
                available_keys: Vec::new(),
            });
        };
        value = object.get(segment).ok_or_else(|| ContextPathBreak {
            missing_segment: segment.to_owned(),
            available_keys: object.keys().cloned().collect(),
        })?;
    }
    Ok(value.clone())
}
