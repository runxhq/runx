use proptest::prelude::*;
use runx_core::state_machine::{
    GraphStatus, GraphStepStatus, SequentialGraphEvent, SequentialGraphPlan, SequentialGraphState,
    SequentialGraphStepDefinition, SequentialGraphStepState, StepAdmissionWitness,
    plan_sequential_graph_transition, transition_sequential_graph,
};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn sequential_graph_transitions_are_deterministic(
        state in sequential_graph_state(),
        event in sequential_graph_event(),
    ) {
        let left = transition_sequential_graph(&state, &event);
        let right = transition_sequential_graph(&state, &event);

        prop_assert_eq!(left, right);
    }

    #[test]
    fn failing_the_graph_is_unconditional(
        state in sequential_graph_state(),
        error in safe_string(),
    ) {
        let failed = transition_sequential_graph(&state, &SequentialGraphEvent::FailGraph { error });

        prop_assert_eq!(failed.status, GraphStatus::Failed);
    }

    #[test]
    fn complete_only_succeeds_when_no_steps_are_pending_or_running(
        mut state in sequential_graph_state(),
        has_blocking_step in any::<bool>(),
    ) {
        if has_blocking_step {
            state.steps[0].status = GraphStepStatus::Pending;
            let next = transition_sequential_graph(&state, &SequentialGraphEvent::Complete);

            prop_assert_eq!(next, state);
        } else {
            for (index, step) in state.steps.iter_mut().enumerate() {
                step.status = if index % 2 == 0 {
                    GraphStepStatus::Succeeded
                } else {
                    GraphStepStatus::Failed
                };
            }
            let mut expected = state.clone();
            expected.status = GraphStatus::Succeeded;
            let next = transition_sequential_graph(&state, &SequentialGraphEvent::Complete);

            prop_assert_eq!(next, expected);
        }
    }

}

#[test]
fn a_pending_step_is_planned_as_its_first_attempt() {
    let steps = vec![SequentialGraphStepDefinition {
        id: "first".to_owned(),
        context_from: None,
    }];
    let state = SequentialGraphState {
        graph_id: "graph".to_owned(),
        status: GraphStatus::Pending,
        steps: vec![SequentialGraphStepState {
            step_id: "first".to_owned(),
            status: GraphStepStatus::Pending,
            attempts: 0,
            started_at: None,
            completed_at: None,
            receipt_id: None,
            outputs: None,
            error: None,
        }],
    };

    let plan = plan_sequential_graph_transition(&state, &steps);

    assert_eq!(
        plan,
        SequentialGraphPlan::RunStep {
            step_id: "first".to_owned(),
            attempt: 1,
            context_from: Vec::new(),
        }
    );
}

fn sequential_graph_state() -> impl Strategy<Value = SequentialGraphState> {
    (
        safe_string(),
        graph_status(),
        prop::collection::vec(sequential_step_state(), 1..4),
    )
        .prop_map(|(graph_id, status, steps)| SequentialGraphState {
            graph_id,
            status,
            steps,
        })
}

fn sequential_step_state() -> impl Strategy<Value = SequentialGraphStepState> {
    (
        safe_string(),
        graph_step_status(),
        0_u32..4,
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
    )
        .prop_map(
            |(step_id, status, attempts, started_at, completed_at, receipt_id, error)| {
                SequentialGraphStepState {
                    step_id,
                    status,
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

fn sequential_graph_event() -> impl Strategy<Value = SequentialGraphEvent> {
    prop_oneof![
        (safe_string(), safe_string())
            .prop_map(|(step_id, at)| { SequentialGraphEvent::StartStep { step_id, at } }),
        (safe_string(), safe_string(), safe_string()).prop_map(|(step_id, at, receipt_id)| {
            SequentialGraphEvent::StepSucceeded {
                admission_witness: Box::new(StepAdmissionWitness::local_runtime(
                    &step_id,
                    &receipt_id,
                )),
                at,
                outputs: None,
            }
        }),
        (safe_string(), safe_string(), safe_string()).prop_map(|(step_id, at, error)| {
            SequentialGraphEvent::StepFailed { step_id, at, error }
        }),
        Just(SequentialGraphEvent::Complete),
        safe_string().prop_map(|error| SequentialGraphEvent::FailGraph { error }),
    ]
}

fn graph_status() -> impl Strategy<Value = GraphStatus> {
    prop_oneof![
        Just(GraphStatus::Pending),
        Just(GraphStatus::Running),
        Just(GraphStatus::Succeeded),
        Just(GraphStatus::Failed),
    ]
}

fn graph_step_status() -> impl Strategy<Value = GraphStepStatus> {
    prop_oneof![
        Just(GraphStepStatus::Pending),
        Just(GraphStepStatus::Running),
        Just(GraphStepStatus::Succeeded),
        Just(GraphStepStatus::Failed),
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
