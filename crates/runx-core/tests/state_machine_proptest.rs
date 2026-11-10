use proptest::prelude::*;
use runx_core::state_machine::{
    SequentialWorkflow, SequentialWorkflowEvent, SequentialWorkflowPlan, SequentialWorkflowStep,
    SequentialWorkflowStepDefinition, StepAdmissionWitness, WorkflowState, WorkflowStepState,
    plan_sequential_workflow_transition, transition_sequential_workflow,
};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn sequential_workflow_transitions_are_deterministic(
        state in sequential_workflow_state(),
        event in sequential_workflow_event(),
    ) {
        let left = transition_sequential_workflow(&state, &event);
        let right = transition_sequential_workflow(&state, &event);

        prop_assert_eq!(left, right);
    }

    #[test]
    fn failing_the_workflow_is_unconditional(
        state in sequential_workflow_state(),
        error in safe_string(),
    ) {
        let failed = transition_sequential_workflow(&state, &SequentialWorkflowEvent::FailWorkflow { error });

        prop_assert_eq!(failed.state, WorkflowState::Failed);
    }

    #[test]
    fn complete_only_succeeds_when_no_steps_are_pending_or_running(
        mut state in sequential_workflow_state(),
        has_blocking_step in any::<bool>(),
    ) {
        if has_blocking_step {
            state.steps[0].state = WorkflowStepState::Pending;
            let next = transition_sequential_workflow(&state, &SequentialWorkflowEvent::Complete);

            prop_assert_eq!(next, state);
        } else {
            for (index, step) in state.steps.iter_mut().enumerate() {
                step.state = if index % 2 == 0 {
                    WorkflowStepState::Succeeded
                } else {
                    WorkflowStepState::Failed
                };
            }
            let mut expected = state.clone();
            expected.state = WorkflowState::Succeeded;
            let next = transition_sequential_workflow(&state, &SequentialWorkflowEvent::Complete);

            prop_assert_eq!(next, expected);
        }
    }

}

#[test]
fn a_pending_step_is_planned_as_its_first_attempt() {
    let steps = vec![SequentialWorkflowStepDefinition {
        id: "first".to_owned(),
        context_from: None,
    }];
    let state = SequentialWorkflow {
        workflow_id: "workflow".to_owned(),
        state: WorkflowState::Pending,
        steps: vec![SequentialWorkflowStep {
            step_id: "first".to_owned(),
            state: WorkflowStepState::Pending,
            attempts: 0,
            started_at: None,
            completed_at: None,
            receipt_id: None,
            outputs: None,
            error: None,
        }],
    };

    let plan = plan_sequential_workflow_transition(&state, &steps);

    assert_eq!(
        plan,
        SequentialWorkflowPlan::RunStep {
            step_id: "first".to_owned(),
            attempt: 1,
            context_from: Vec::new(),
        }
    );
}

fn sequential_workflow_state() -> impl Strategy<Value = SequentialWorkflow> {
    (
        safe_string(),
        workflow_state(),
        prop::collection::vec(sequential_step_state(), 1..4),
    )
        .prop_map(|(workflow_id, state, steps)| SequentialWorkflow {
            workflow_id,
            state,
            steps,
        })
}

fn sequential_step_state() -> impl Strategy<Value = SequentialWorkflowStep> {
    (
        safe_string(),
        workflow_step_state(),
        0_u32..4,
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
    )
        .prop_map(
            |(step_id, state, attempts, started_at, completed_at, receipt_id, error)| {
                SequentialWorkflowStep {
                    step_id,
                    state,
                    attempts,
                    started_at,
                    completed_at,
                    receipt_id,
                    outputs: None,
                    error,
                }
            },
        )
}

fn sequential_workflow_event() -> impl Strategy<Value = SequentialWorkflowEvent> {
    prop_oneof![
        (safe_string(), safe_string())
            .prop_map(|(step_id, at)| { SequentialWorkflowEvent::StartStep { step_id, at } }),
        (safe_string(), safe_string(), safe_string()).prop_map(|(step_id, at, receipt_id)| {
            SequentialWorkflowEvent::StepSucceeded {
                admission_witness: Box::new(StepAdmissionWitness::local_runtime(
                    &step_id,
                    &receipt_id,
                )),
                at,
                outputs: None,
            }
        }),
        (safe_string(), safe_string(), safe_string()).prop_map(|(step_id, at, error)| {
            SequentialWorkflowEvent::StepFailed { step_id, at, error }
        }),
        Just(SequentialWorkflowEvent::Complete),
        safe_string().prop_map(|error| SequentialWorkflowEvent::FailWorkflow { error }),
    ]
}

fn workflow_state() -> impl Strategy<Value = WorkflowState> {
    prop_oneof![
        Just(WorkflowState::Pending),
        Just(WorkflowState::Running),
        Just(WorkflowState::Succeeded),
        Just(WorkflowState::Failed),
    ]
}

fn workflow_step_state() -> impl Strategy<Value = WorkflowStepState> {
    prop_oneof![
        Just(WorkflowStepState::Pending),
        Just(WorkflowStepState::Running),
        Just(WorkflowStepState::Succeeded),
        Just(WorkflowStepState::Failed),
    ]
}

fn safe_string() -> impl Strategy<Value = String> {
    (0_u8..26, prop::collection::vec(0_u8..37, 0..12)).prop_map(|(first, rest)| {
        let mut output = String::with_capacity(rest.len() + 1);
        output.push((b'a' + first) as char);
        for value in rest {
            output.push(safe_char(value));
        }
        output
    })
}

fn safe_char(value: u8) -> char {
    match value {
        0..=25 => (b'a' + value) as char,
        26..=35 => (b'0' + (value - 26)) as char,
        _ => '_',
    }
}
