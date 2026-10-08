use runx_contracts::JsonValue;

use super::KernelEvalError;
use super::input::KernelInput;
use crate::policy::{
    admit_graph_step_scopes, build_authority_proof_metadata, build_local_scope_admission,
    scope_grant_allows, validate_credential_binding,
};
use crate::state_machine::{
    create_sequential_graph_state, create_single_step_state, plan_sequential_graph_transition,
    transition_sequential_graph, transition_single_step,
};

pub(super) fn evaluate_kernel_input(input: KernelInput) -> Result<JsonValue, KernelEvalError> {
    match input {
        KernelInput::AdmitGraphStepScopes { .. }
        | KernelInput::BuildLocalScopeAdmission { .. }
        | KernelInput::BuildAuthorityProofMetadata { .. }
        | KernelInput::ValidateCredentialBinding { .. }
        | KernelInput::ScopeGrantAllows { .. } => evaluate_policy_input(input),
        KernelInput::CreateSingleStepState { .. }
        | KernelInput::TransitionSingleStep { .. }
        | KernelInput::CreateSequentialGraphState { .. }
        | KernelInput::PlanSequentialGraphTransition { .. }
        | KernelInput::TransitionSequentialGraph { .. } => evaluate_state_machine_input(input),
    }
}

fn evaluate_policy_input(input: KernelInput) -> Result<JsonValue, KernelEvalError> {
    match input {
        KernelInput::AdmitGraphStepScopes { request } => {
            to_value(admit_graph_step_scopes(&request))
        }
        KernelInput::BuildLocalScopeAdmission {
            auth,
            grants,
            options,
        } => to_value(build_local_scope_admission(
            auth.as_ref(),
            &grants,
            &options,
        )),
        KernelInput::BuildAuthorityProofMetadata { options } => {
            to_value(build_authority_proof_metadata(&options))
        }
        KernelInput::ValidateCredentialBinding { request } => {
            to_value(validate_credential_binding(&request))
        }
        KernelInput::ScopeGrantAllows {
            granted_scope,
            requested_scope,
            policy,
        } => to_value(scope_grant_allows(&granted_scope, &requested_scope, policy)),
        _ => unreachable!("policy dispatch only receives policy inputs"),
    }
}

fn evaluate_state_machine_input(input: KernelInput) -> Result<JsonValue, KernelEvalError> {
    match input {
        KernelInput::CreateSingleStepState { step_id } => {
            to_value(create_single_step_state(step_id))
        }
        KernelInput::TransitionSingleStep { state, event } => {
            to_value(transition_single_step(&state, &event))
        }
        KernelInput::CreateSequentialGraphState { graph_id, steps } => {
            to_value(create_sequential_graph_state(graph_id, &steps))
        }
        KernelInput::PlanSequentialGraphTransition { state, steps } => {
            to_value(plan_sequential_graph_transition(&state, &steps))
        }
        KernelInput::TransitionSequentialGraph { state, event } => {
            to_value(transition_sequential_graph(&state, &event))
        }
        _ => unreachable!("state-machine dispatch only receives state-machine inputs"),
    }
}

fn to_value(value: impl serde::Serialize) -> Result<JsonValue, KernelEvalError> {
    let source = serde_json::to_string(&value)
        .map_err(|error| KernelEvalError::SerializeOutput(error.to_string()))?;
    serde_json::from_str(&source)
        .map_err(|error| KernelEvalError::SerializeOutput(error.to_string()))
}
