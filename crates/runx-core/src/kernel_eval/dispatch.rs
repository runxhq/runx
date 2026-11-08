use runx_contracts::JsonValue;

use super::KernelEvalError;
use super::input::KernelInput;
use crate::policy::scope_grant_allows;
use crate::state_machine::{
    create_sequential_workflow, plan_sequential_workflow_transition, transition_sequential_workflow,
};

pub(super) fn evaluate_kernel_input(input: KernelInput) -> Result<JsonValue, KernelEvalError> {
    match input {
        KernelInput::ScopeGrantAllows {
            granted_scope,
            requested_scope,
            policy,
        } => to_value(scope_grant_allows(&granted_scope, &requested_scope, policy)),
        KernelInput::CreateSequentialWorkflowState { workflow_id, steps } => {
            to_value(create_sequential_workflow(workflow_id, &steps))
        }
        KernelInput::PlanSequentialWorkflowTransition { state, steps } => {
            to_value(plan_sequential_workflow_transition(&state, &steps))
        }
        KernelInput::TransitionSequentialWorkflow { state, event } => {
            to_value(transition_sequential_workflow(&state, &event))
        }
    }
}

fn to_value(value: impl serde::Serialize) -> Result<JsonValue, KernelEvalError> {
    let source = serde_json::to_string(&value)
        .map_err(|error| KernelEvalError::SerializeOutput(error.to_string()))?;
    serde_json::from_str(&source)
        .map_err(|error| KernelEvalError::SerializeOutput(error.to_string()))
}
