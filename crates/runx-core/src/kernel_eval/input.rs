use runx_contracts::{JsonValue, json_string_field};
use serde::Deserialize;

use crate::policy::{
    BuildAuthorityProofOptions, CredentialBindingRequest, GraphScopeAdmissionRequest,
    LocalAdmissionGrant, LocalScopeAdmissionOptions, ScopeGrantPolicy,
};
use crate::state_machine::{
    SequentialGraphEvent, SequentialGraphState, SequentialGraphStepDefinition, SingleStepEvent,
    SingleStepState,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub(super) enum KernelInput {
    #[serde(rename = "policy.admitGraphStepScopes")]
    AdmitGraphStepScopes { request: GraphScopeAdmissionRequest },
    #[serde(rename = "policy.buildLocalScopeAdmission")]
    BuildLocalScopeAdmission {
        auth: Option<JsonValue>,
        #[serde(default)]
        grants: Vec<LocalAdmissionGrant>,
        #[serde(default)]
        options: LocalScopeAdmissionOptions,
    },
    #[serde(rename = "policy.buildAuthorityProofMetadata")]
    BuildAuthorityProofMetadata {
        options: Box<BuildAuthorityProofOptions>,
    },
    #[serde(rename = "policy.validateCredentialBinding")]
    ValidateCredentialBinding {
        request: Box<CredentialBindingRequest>,
    },
    #[serde(rename = "policy.scopeGrantAllows")]
    ScopeGrantAllows {
        granted_scope: String,
        requested_scope: String,
        policy: ScopeGrantPolicy,
    },
    #[serde(rename = "state-machine.createSingleStepState")]
    CreateSingleStepState { step_id: String },
    #[serde(rename = "state-machine.transitionSingleStep")]
    TransitionSingleStep {
        state: SingleStepState,
        event: SingleStepEvent,
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
        "policy.admitGraphStepScopes"
            | "policy.buildLocalScopeAdmission"
            | "policy.buildAuthorityProofMetadata"
            | "policy.validateCredentialBinding"
            | "policy.scopeGrantAllows"
            | "state-machine.createSingleStepState"
            | "state-machine.transitionSingleStep"
            | "state-machine.createSequentialGraphState"
            | "state-machine.planSequentialGraphTransition"
            | "state-machine.transitionSequentialGraph"
    )
}
