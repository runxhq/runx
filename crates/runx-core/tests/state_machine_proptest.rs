use proptest::prelude::*;
use runx_core::state_machine::{
    GraphState, GraphStepState, SequentialGraph, SequentialGraphEvent, SequentialGraphPlan,
    SequentialGraphStep, SequentialGraphStepDefinition, StepAdmissionWitness,
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

        prop_assert_eq!(failed.state, GraphState::Failed);
    }

    #[test]
    fn complete_only_succeeds_when_no_steps_are_pending_or_running(
        mut state in sequential_graph_state(),
        has_blocking_step in any::<bool>(),
    ) {
        if has_blocking_step {
            state.steps[0].state = GraphStepState::Pending;
            let next = transition_sequential_graph(&state, &SequentialGraphEvent::Complete);

            prop_assert_eq!(next, state);
        } else {
            for (index, step) in state.steps.iter_mut().enumerate() {
                step.state = if index % 2 == 0 {
                    GraphStepState::Succeeded
                } else {
                    GraphStepState::Failed
                };
            }
            let mut expected = state.clone();
            expected.state = GraphState::Succeeded;
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
    let state = SequentialGraph {
        graph_id: "graph".to_owned(),
        state: GraphState::Pending,
        steps: vec![SequentialGraphStep {
            step_id: "first".to_owned(),
            state: GraphStepState::Pending,
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

fn sequential_graph_state() -> impl Strategy<Value = SequentialGraph> {
    (
        safe_string(),
        graph_state(),
        prop::collection::vec(sequential_step_state(), 1..4),
    )
        .prop_map(|(graph_id, state, steps)| SequentialGraph {
            graph_id,
            state,
            steps,
        })
}

fn sequential_step_state() -> impl Strategy<Value = SequentialGraphStep> {
    (
        safe_string(),
        graph_step_state(),
        0_u32..4,
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
        prop::option::of(safe_string()),
    )
        .prop_map(
            |(step_id, state, attempts, started_at, completed_at, receipt_id, error)| {
                SequentialGraphStep {
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

fn graph_state() -> impl Strategy<Value = GraphState> {
    prop_oneof![
        Just(GraphState::Pending),
        Just(GraphState::Running),
        Just(GraphState::Succeeded),
        Just(GraphState::Failed),
    ]
}

fn graph_step_state() -> impl Strategy<Value = GraphStepState> {
    prop_oneof![
        Just(GraphStepState::Pending),
        Just(GraphStepState::Running),
        Just(GraphStepState::Succeeded),
        Just(GraphStepState::Failed),
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
