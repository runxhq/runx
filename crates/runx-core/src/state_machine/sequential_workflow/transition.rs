use runx_contracts::JsonObject;

use super::super::types::{
    SequentialWorkflow, SequentialWorkflowEvent, SequentialWorkflowStep, StepAdmissionWitness,
    WorkflowState, WorkflowStepState,
};
use super::index::SequentialWorkflowStepIndex;

#[must_use]
pub fn transition_sequential_workflow(
    workflow: &SequentialWorkflow,
    event: &SequentialWorkflowEvent,
) -> SequentialWorkflow {
    let mut next = workflow.clone();
    apply_sequential_workflow_event(&mut next, event);
    next
}

pub fn apply_sequential_workflow_event(
    workflow: &mut SequentialWorkflow,
    event: &SequentialWorkflowEvent,
) {
    apply_sequential_workflow_event_owned_with_index(workflow, event.clone(), None);
}

/// Apply an owned event without cloning its payload.
///
/// Runtime executors construct an event for immediate application. Consuming
/// that event lets large step outputs and error strings move into workflow state;
/// the borrowed APIs remain available for callers that need to retain an
/// event for replay.
pub fn apply_sequential_workflow_event_owned_indexed(
    workflow: &mut SequentialWorkflow,
    event: SequentialWorkflowEvent,
    step_index: &SequentialWorkflowStepIndex,
) {
    apply_sequential_workflow_event_owned_with_index(workflow, event, Some(step_index));
}

fn apply_sequential_workflow_event_owned_with_index(
    workflow: &mut SequentialWorkflow,
    event: SequentialWorkflowEvent,
    step_index: Option<&SequentialWorkflowStepIndex>,
) {
    match event {
        SequentialWorkflowEvent::StartStep { step_id, at } => {
            start_sequential_workflow_step(workflow, &step_id, at, step_index);
        }
        SequentialWorkflowEvent::StepSucceeded {
            at,
            admission_witness,
            outputs,
        } => {
            succeed_sequential_workflow_step(workflow, at, *admission_witness, outputs, step_index)
        }
        SequentialWorkflowEvent::StepFailed { step_id, at, error } => {
            update_step_in_place(workflow, &step_id, step_index, |step| {
                fail_step_in_place(step, at, error);
            });
        }
        SequentialWorkflowEvent::StepSkipped { step_id, at } => {
            update_step_in_place(workflow, &step_id, step_index, |step| {
                skip_step_in_place(step, at);
            });
        }
        SequentialWorkflowEvent::Complete if is_workflow_complete(workflow) => {
            workflow.state = WorkflowState::Succeeded;
        }
        SequentialWorkflowEvent::Complete => {}
        SequentialWorkflowEvent::FailWorkflow { .. } => {
            workflow.state = WorkflowState::Failed;
        }
    }
}

pub fn start_sequential_workflow_step_indexed(
    workflow: &mut SequentialWorkflow,
    step_id: &str,
    at: String,
    step_index: &SequentialWorkflowStepIndex,
) {
    start_sequential_workflow_step(workflow, step_id, at, Some(step_index));
}

pub fn succeed_sequential_workflow_step_indexed(
    workflow: &mut SequentialWorkflow,
    at: String,
    admission_witness: StepAdmissionWitness,
    outputs: Option<JsonObject>,
    step_index: &SequentialWorkflowStepIndex,
) {
    succeed_sequential_workflow_step(workflow, at, admission_witness, outputs, Some(step_index));
}

fn start_sequential_workflow_step(
    workflow: &mut SequentialWorkflow,
    step_id: &str,
    at: String,
    step_index: Option<&SequentialWorkflowStepIndex>,
) {
    update_step_in_place(workflow, step_id, step_index, |step| {
        start_step_in_place(step, at);
    });
    workflow.state = WorkflowState::Running;
}

fn succeed_sequential_workflow_step(
    workflow: &mut SequentialWorkflow,
    at: String,
    admission_witness: StepAdmissionWitness,
    outputs: Option<JsonObject>,
    step_index: Option<&SequentialWorkflowStepIndex>,
) {
    if admission_witness.step_id.is_empty() || admission_witness.receipt_id.is_empty() {
        return;
    }
    let StepAdmissionWitness {
        step_id,
        receipt_id,
        ..
    } = admission_witness;
    update_step_in_place(workflow, &step_id, step_index, |step| {
        succeed_step_in_place(step, at, receipt_id, outputs);
    });
}

fn start_step_in_place(step: &mut SequentialWorkflowStep, at: String) {
    if matches!(
        step.state,
        WorkflowStepState::Running | WorkflowStepState::Succeeded
    ) {
        return;
    }
    step.state = WorkflowStepState::Running;
    step.attempts += 1;
    step.started_at = Some(at);
    step.completed_at = None;
    step.outputs = None;
    step.error = None;
}

fn succeed_step_in_place(
    step: &mut SequentialWorkflowStep,
    at: String,
    receipt_id: String,
    outputs: Option<JsonObject>,
) {
    if step.state != WorkflowStepState::Running {
        return;
    }
    step.state = WorkflowStepState::Succeeded;
    step.completed_at = Some(at);
    step.receipt_id = Some(receipt_id);
    step.outputs = outputs;
    step.error = None;
}

fn fail_step_in_place(step: &mut SequentialWorkflowStep, at: String, error: String) {
    if step.state != WorkflowStepState::Running {
        return;
    }
    step.state = WorkflowStepState::Failed;
    step.completed_at = Some(at);
    step.outputs = None;
    step.error = Some(error);
}

fn skip_step_in_place(step: &mut SequentialWorkflowStep, at: String) {
    if step.state != WorkflowStepState::Pending {
        return;
    }
    step.state = WorkflowStepState::Skipped;
    step.completed_at = Some(at);
    step.outputs = None;
    step.error = None;
}

fn update_step_in_place(
    workflow: &mut SequentialWorkflow,
    step_id: &str,
    step_index: Option<&SequentialWorkflowStepIndex>,
    update: impl FnOnce(&mut SequentialWorkflowStep),
) {
    let position = step_index.and_then(|index| index.position(step_id));
    let step = match position {
        Some(position) => workflow
            .steps
            .get_mut(position)
            .filter(|step| step.step_id == step_id),
        None if step_index.is_none() => workflow
            .steps
            .iter_mut()
            .find(|step| step.step_id == step_id),
        None => None,
    };
    if let Some(step) = step {
        update(step);
    }
}

fn is_workflow_complete(workflow: &SequentialWorkflow) -> bool {
    workflow.steps.iter().all(|step| {
        !matches!(
            step.state,
            WorkflowStepState::Pending | WorkflowStepState::Running
        )
    })
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonObject, JsonValue};

    use super::{
        apply_sequential_workflow_event, apply_sequential_workflow_event_owned_indexed,
        start_sequential_workflow_step_indexed, succeed_sequential_workflow_step_indexed,
    };
    use crate::state_machine::{
        SequentialWorkflowEvent, SequentialWorkflowStepDefinition, StepAdmissionWitness,
        create_sequential_workflow, create_sequential_workflow_step_index,
    };

    #[test]
    fn owned_indexed_events_match_the_borrowed_transition_contract() {
        let definitions = vec![SequentialWorkflowStepDefinition {
            id: "step".to_owned(),
            context_from: None,
        }];
        let index = create_sequential_workflow_step_index(&definitions);
        let mut borrowed = create_sequential_workflow("graph", &definitions);
        let mut owned = borrowed.clone();
        let mut direct = borrowed.clone();
        let events = vec![
            SequentialWorkflowEvent::StartStep {
                step_id: "step".to_owned(),
                at: "2026-07-20T00:00:00Z".to_owned(),
            },
            SequentialWorkflowEvent::StepSucceeded {
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
            SequentialWorkflowEvent::Complete,
        ];

        for event in events {
            apply_sequential_workflow_event(&mut borrowed, &event);
            apply_sequential_workflow_event_owned_indexed(&mut owned, event.clone(), &index);
            match event {
                SequentialWorkflowEvent::StartStep { step_id, at } => {
                    start_sequential_workflow_step_indexed(&mut direct, &step_id, at, &index);
                }
                SequentialWorkflowEvent::StepSucceeded {
                    at,
                    admission_witness,
                    outputs,
                } => succeed_sequential_workflow_step_indexed(
                    &mut direct,
                    at,
                    *admission_witness,
                    outputs,
                    &index,
                ),
                event => apply_sequential_workflow_event_owned_indexed(&mut direct, event, &index),
            }
            assert_eq!(owned, borrowed);
            assert_eq!(direct, borrowed);
        }
    }
}
