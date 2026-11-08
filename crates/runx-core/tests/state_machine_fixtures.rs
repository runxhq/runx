use runx_core::state_machine::{
    SequentialWorkflow, SequentialWorkflowEvent, SequentialWorkflowStepDefinition,
    create_sequential_workflow, plan_sequential_workflow_transition,
    transition_sequential_workflow,
};
use serde::Deserialize;
use serde_json::Value;

type TestResult = Result<(), String>;

#[derive(Deserialize)]
struct KernelFixture {
    name: String,
    input: StateMachineInput,
    expected: Expected,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Expected {
    Output {
        value: Value,
    },
    Error {
        code: String,
        message: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
enum StateMachineInput {
    #[serde(rename = "state-machine.createSequentialWorkflow")]
    CreateSequentialWorkflowState {
        workflow_id: String,
        steps: Vec<SequentialWorkflowStepDefinition>,
    },
    #[serde(rename = "state-machine.planSequentialWorkflowTransition")]
    PlanSequentialWorkflowTransition {
        state: SequentialWorkflow,
        steps: Vec<SequentialWorkflowStepDefinition>,
    },
    #[serde(rename = "state-machine.transitionSequentialWorkflow")]
    TransitionSequentialWorkflow {
        state: SequentialWorkflow,
        event: SequentialWorkflowEvent,
    },
}

#[test]
fn fixture_sequential_create_workflow() -> TestResult {
    assert_fixture(include_str!(
        "../../../fixtures/kernel/state-machine/sequential-create-workflow.json"
    ))
}

#[test]
fn fixture_sequential_plan_first_step() -> TestResult {
    assert_fixture(include_str!(
        "../../../fixtures/kernel/state-machine/sequential-plan-first-step.json"
    ))
}

#[test]
fn fixture_sequential_plan_fails_after_a_failed_step() -> TestResult {
    assert_fixture(include_str!(
        "../../../fixtures/kernel/state-machine/sequential-plan-fails-after-a-failed-step.json"
    ))
}

#[test]
fn fixture_sequential_transition_step_succeeded() -> TestResult {
    assert_fixture(include_str!(
        "../../../fixtures/kernel/state-machine/sequential-transition-step-succeeded.json"
    ))
}

fn assert_fixture(json: &str) -> TestResult {
    let fixture: KernelFixture = serde_json::from_str(json).map_err(string_error)?;
    let actual = evaluate_input(fixture.input)?;
    match fixture.expected {
        Expected::Output { value } => {
            assert_eq!(actual, value, "fixture {}", fixture.name);
            Ok(())
        }
        Expected::Error { code, message } => Err(format!(
            "fixture {} expected error {code} {message:?}, but state-machine dispatch succeeded",
            fixture.name
        )),
    }
}

fn evaluate_input(input: StateMachineInput) -> Result<Value, String> {
    match input {
        StateMachineInput::CreateSequentialWorkflowState { workflow_id, steps } => {
            to_value(create_sequential_workflow(workflow_id, &steps))
        }
        StateMachineInput::PlanSequentialWorkflowTransition { state, steps } => {
            to_value(plan_sequential_workflow_transition(&state, &steps))
        }
        StateMachineInput::TransitionSequentialWorkflow { state, event } => {
            to_value(transition_sequential_workflow(&state, &event))
        }
    }
}

fn to_value(value: impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(string_error)
}

fn string_error(error: serde_json::Error) -> String {
    error.to_string()
}
