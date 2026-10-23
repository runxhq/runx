use runx_contracts::JsonObject;

use super::super::types::{
    GraphState, GraphStepState, SequentialGraph, SequentialGraphEvent, SequentialGraphStep,
    StepAdmissionWitness,
};
use super::index::SequentialGraphStepIndex;

#[must_use]
pub fn transition_sequential_graph(
    graph: &SequentialGraph,
    event: &SequentialGraphEvent,
) -> SequentialGraph {
    let mut next = graph.clone();
    apply_sequential_graph_event(&mut next, event);
    next
}

pub fn apply_sequential_graph_event(graph: &mut SequentialGraph, event: &SequentialGraphEvent) {
    apply_sequential_graph_event_owned_with_index(graph, event.clone(), None);
}

pub fn apply_sequential_graph_event_indexed(
    graph: &mut SequentialGraph,
    event: &SequentialGraphEvent,
    step_index: &SequentialGraphStepIndex,
) {
    apply_sequential_graph_event_owned_with_index(graph, event.clone(), Some(step_index));
}

/// Apply an owned event without cloning its payload.
///
/// Runtime executors construct an event for immediate application. Consuming
/// that event lets large step outputs and error strings move into graph state;
/// the borrowed APIs remain available for callers that need to retain an
/// event for replay.
pub fn apply_sequential_graph_event_owned_indexed(
    graph: &mut SequentialGraph,
    event: SequentialGraphEvent,
    step_index: &SequentialGraphStepIndex,
) {
    apply_sequential_graph_event_owned_with_index(graph, event, Some(step_index));
}

fn apply_sequential_graph_event_owned_with_index(
    graph: &mut SequentialGraph,
    event: SequentialGraphEvent,
    step_index: Option<&SequentialGraphStepIndex>,
) {
    match event {
        SequentialGraphEvent::StartStep { step_id, at } => {
            start_sequential_graph_step(graph, &step_id, at, step_index);
        }
        SequentialGraphEvent::StepSucceeded {
            at,
            admission_witness,
            outputs,
        } => succeed_sequential_graph_step(graph, at, *admission_witness, outputs, step_index),
        SequentialGraphEvent::StepFailed { step_id, at, error } => {
            update_step_in_place(graph, &step_id, step_index, |step| {
                fail_step_in_place(step, at, error);
            });
        }
        SequentialGraphEvent::StepSkipped { step_id, at } => {
            update_step_in_place(graph, &step_id, step_index, |step| {
                skip_step_in_place(step, at);
            });
        }
        SequentialGraphEvent::Complete if is_graph_complete(graph) => {
            graph.state = GraphState::Succeeded;
        }
        SequentialGraphEvent::Complete => {}
        SequentialGraphEvent::FailGraph { .. } => {
            graph.state = GraphState::Failed;
        }
    }
}

pub fn start_sequential_graph_step_indexed(
    graph: &mut SequentialGraph,
    step_id: &str,
    at: String,
    step_index: &SequentialGraphStepIndex,
) {
    start_sequential_graph_step(graph, step_id, at, Some(step_index));
}

pub fn succeed_sequential_graph_step_indexed(
    graph: &mut SequentialGraph,
    at: String,
    admission_witness: StepAdmissionWitness,
    outputs: Option<JsonObject>,
    step_index: &SequentialGraphStepIndex,
) {
    succeed_sequential_graph_step(graph, at, admission_witness, outputs, Some(step_index));
}

fn start_sequential_graph_step(
    graph: &mut SequentialGraph,
    step_id: &str,
    at: String,
    step_index: Option<&SequentialGraphStepIndex>,
) {
    update_step_in_place(graph, step_id, step_index, |step| {
        start_step_in_place(step, at);
    });
    graph.state = GraphState::Running;
}

fn succeed_sequential_graph_step(
    graph: &mut SequentialGraph,
    at: String,
    admission_witness: StepAdmissionWitness,
    outputs: Option<JsonObject>,
    step_index: Option<&SequentialGraphStepIndex>,
) {
    if admission_witness.step_id.is_empty() || admission_witness.receipt_id.is_empty() {
        return;
    }
    let StepAdmissionWitness {
        step_id,
        receipt_id,
        ..
    } = admission_witness;
    update_step_in_place(graph, &step_id, step_index, |step| {
        succeed_step_in_place(step, at, receipt_id, outputs);
    });
}

fn start_step_in_place(step: &mut SequentialGraphStep, at: String) {
    if matches!(
        step.state,
        GraphStepState::Running | GraphStepState::Succeeded
    ) {
        return;
    }
    step.state = GraphStepState::Running;
    step.attempts += 1;
    step.started_at = Some(at);
    step.completed_at = None;
    step.outputs = None;
    step.error = None;
}

fn succeed_step_in_place(
    step: &mut SequentialGraphStep,
    at: String,
    receipt_id: String,
    outputs: Option<JsonObject>,
) {
    if step.state != GraphStepState::Running {
        return;
    }
    step.state = GraphStepState::Succeeded;
    step.completed_at = Some(at);
    step.receipt_id = Some(receipt_id);
    step.outputs = outputs;
    step.error = None;
}

fn fail_step_in_place(step: &mut SequentialGraphStep, at: String, error: String) {
    if step.state != GraphStepState::Running {
        return;
    }
    step.state = GraphStepState::Failed;
    step.completed_at = Some(at);
    step.outputs = None;
    step.error = Some(error);
}

fn skip_step_in_place(step: &mut SequentialGraphStep, at: String) {
    if step.state != GraphStepState::Pending {
        return;
    }
    step.state = GraphStepState::Skipped;
    step.completed_at = Some(at);
    step.outputs = None;
    step.error = None;
}

fn update_step_in_place(
    graph: &mut SequentialGraph,
    step_id: &str,
    step_index: Option<&SequentialGraphStepIndex>,
    update: impl FnOnce(&mut SequentialGraphStep),
) {
    let position = step_index.and_then(|index| index.position(step_id));
    let step = match position {
        Some(position) => graph
            .steps
            .get_mut(position)
            .filter(|step| step.step_id == step_id),
        None if step_index.is_none() => graph.steps.iter_mut().find(|step| step.step_id == step_id),
        None => None,
    };
    if let Some(step) = step {
        update(step);
    }
}

fn is_graph_complete(graph: &SequentialGraph) -> bool {
    graph.steps.iter().all(|step| {
        !matches!(
            step.state,
            GraphStepState::Pending | GraphStepState::Running
        )
    })
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonObject, JsonValue};

    use super::{
        apply_sequential_graph_event, apply_sequential_graph_event_owned_indexed,
        start_sequential_graph_step_indexed, succeed_sequential_graph_step_indexed,
    };
    use crate::state_machine::{
        SequentialGraphEvent, SequentialGraphStepDefinition, StepAdmissionWitness,
        create_sequential_graph, create_sequential_graph_step_index,
    };

    #[test]
    fn owned_indexed_events_match_the_borrowed_transition_contract() {
        let definitions = vec![SequentialGraphStepDefinition {
            id: "step".to_owned(),
            context_from: None,
        }];
        let index = create_sequential_graph_step_index(&definitions);
        let mut borrowed = create_sequential_graph("graph", &definitions);
        let mut owned = borrowed.clone();
        let mut direct = borrowed.clone();
        let events = vec![
            SequentialGraphEvent::StartStep {
                step_id: "step".to_owned(),
                at: "2026-07-20T00:00:00Z".to_owned(),
            },
            SequentialGraphEvent::StepSucceeded {
                at: "2026-07-20T00:00:01Z".to_owned(),
                admission_witness: Box::new(StepAdmissionWitness::local_runtime(
                    "step",
                    "sha256:step",
                )),
                outputs: Some(JsonObject::from([(
                    "result".to_owned(),
                    JsonValue::String("moved once".to_owned()),
                )])),
            },
            SequentialGraphEvent::Complete,
        ];

        for event in events {
            apply_sequential_graph_event(&mut borrowed, &event);
            apply_sequential_graph_event_owned_indexed(&mut owned, event.clone(), &index);
            match event {
                SequentialGraphEvent::StartStep { step_id, at } => {
                    start_sequential_graph_step_indexed(&mut direct, &step_id, at, &index);
                }
                SequentialGraphEvent::StepSucceeded {
                    at,
                    admission_witness,
                    outputs,
                } => succeed_sequential_graph_step_indexed(
                    &mut direct,
                    at,
                    *admission_witness,
                    outputs,
                    &index,
                ),
                event => apply_sequential_graph_event_owned_indexed(&mut direct, event, &index),
            }
            assert_eq!(owned, borrowed);
            assert_eq!(direct, borrowed);
        }
    }
}
