use runx_contracts::{JsonValue, json_string_field};
use serde::Deserialize;

use crate::policy::ScopeGrantPolicy;
use crate::state_machine::{
    SequentialWorkflow, SequentialWorkflowEvent, SequentialWorkflowStepDefinition,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub(super) enum KernelInput {
    #[serde(rename = "policy.scopeGrantAllows")]
    ScopeGrantAllows {
        granted_scope: String,
        requested_scope: String,
        policy: ScopeGrantPolicy,
    },
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

pub(super) fn kernel_document_kind(document: &JsonValue) -> Option<&str> {
    let JsonValue::Object(fields) = document else {
        return None;
    };
    json_string_field(fields, "kind")
}

pub(super) fn is_supported_kernel_kind(kind: &str) -> bool {
    matches!(
        kind,
        "policy.scopeGrantAllows"
            | "state-machine.createSequentialWorkflow"
            | "state-machine.planSequentialWorkflowTransition"
            | "state-machine.transitionSequentialWorkflow"
    )
}
