use runx_contracts::{JsonValue, json_string_field};
use serde::Deserialize;

use crate::policy::ScopeGrantPolicy;
use crate::state_machine::{
    SequentialGraphEvent, SequentialGraphState, SequentialGraphStepDefinition,
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
    #[serde(rename = "state-machine.createSequentialGraphState")]
    CreateSequentialGraphState {
        graph_id: String,
        steps: Vec<SequentialGraphStepDefinition>,
    },
    #[serde(rename = "state-machine.planSequentialGraphTransition")]
    PlanSequentialGraphTransition {
        state: SequentialGraphState,
        steps: Vec<SequentialGraphStepDefinition>,
    },
    #[serde(rename = "state-machine.transitionSequentialGraph")]
    TransitionSequentialGraph {
        state: SequentialGraphState,
        event: SequentialGraphEvent,
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
            | "state-machine.createSequentialGraphState"
            | "state-machine.planSequentialGraphTransition"
            | "state-machine.transitionSequentialGraph"
    )
}
