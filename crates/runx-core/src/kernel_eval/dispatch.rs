use runx_contracts::JsonValue;

use super::KernelEvalError;
use super::input::KernelInput;
use crate::policy::scope_grant_allows;
use crate::state_machine::{
    create_sequential_graph, plan_sequential_graph_transition, transition_sequential_graph,
};

pub(super) fn evaluate_kernel_input(input: KernelInput) -> Result<JsonValue, KernelEvalError> {
    match input {
        KernelInput::ScopeGrantAllows {
            granted_scope,
            requested_scope,
            policy,
        } => to_value(scope_grant_allows(&granted_scope, &requested_scope, policy)),
        KernelInput::CreateSequentialGraphState { graph_id, steps } => {
            to_value(create_sequential_graph(graph_id, &steps))
        }
        KernelInput::PlanSequentialGraphTransition { state, steps } => {
            to_value(plan_sequential_graph_transition(&state, &steps))
        }
        KernelInput::TransitionSequentialGraph { state, event } => {
            to_value(transition_sequential_graph(&state, &event))
        }
    }
}

fn to_value(value: impl serde::Serialize) -> Result<JsonValue, KernelEvalError> {
    let source = serde_json::to_string(&value)
        .map_err(|error| KernelEvalError::SerializeOutput(error.to_string()))?;
    serde_json::from_str(&source)
        .map_err(|error| KernelEvalError::SerializeOutput(error.to_string()))
}
