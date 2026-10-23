//! Extensible caller context carried across governed workflow handoffs.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{JsonObject, JsonValue};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, RunxSchema)]
#[runx_schema(
    id = "runx.orchestrator.execution_context.v1",
    url = "https://schemas.runx.ai/runx/orchestrator/execution-context/v1.json"
)]
pub struct OrchestratorExecutionContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    /// The orchestrator's own id for the run that made the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff_audience: Option<String>,
    #[serde(flatten)]
    pub extensions: JsonObject,
}

impl OrchestratorExecutionContext {
    #[must_use]
    pub fn identifies_origin(&self) -> bool {
        [
            self.caller.as_deref(),
            self.caller_id.as_deref(),
            self.principal.as_deref(),
            self.principal_id.as_deref(),
            self.workflow.as_deref(),
            self.workflow_id.as_deref(),
            self.upstream_run_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty())
    }

    pub fn bind_handoff(
        &mut self,
        platform: &str,
        event_id: &str,
        idempotency_key: &str,
        handoff_scope: &str,
        handoff_audience: &str,
    ) {
        self.platform.get_or_insert_with(|| platform.to_owned());
        self.event_id.get_or_insert_with(|| event_id.to_owned());
        self.idempotency_key
            .get_or_insert_with(|| idempotency_key.to_owned());
        self.handoff_scope
            .get_or_insert_with(|| handoff_scope.to_owned());
        self.handoff_audience
            .get_or_insert_with(|| handoff_audience.to_owned());
    }

    #[must_use]
    pub fn binding_mismatches(
        &self,
        platform: &str,
        event_id: &str,
        idempotency_key: &str,
        handoff_scope: &str,
        handoff_audience: &str,
    ) -> Vec<(&'static str, String)> {
        let expected = [
            ("platform", self.platform.as_deref(), platform),
            ("event_id", self.event_id.as_deref(), event_id),
            (
                "idempotency_key",
                self.idempotency_key.as_deref(),
                idempotency_key,
            ),
            (
                "handoff_scope",
                self.handoff_scope.as_deref(),
                handoff_scope,
            ),
            (
                "handoff_audience",
                self.handoff_audience.as_deref(),
                handoff_audience,
            ),
        ];
        expected
            .into_iter()
            .filter_map(|(field, actual, expected)| {
                actual
                    .filter(|actual| actual.trim() != expected)
                    .map(|_| (field, expected.to_owned()))
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.orchestrator.handoff_context.v1",
    url = "https://schemas.runx.ai/runx/orchestrator/handoff-context/v1.json"
)]
pub struct OrchestratorHandoffContext {
    pub status: String,
    pub platform: String,
    pub event_id: String,
    pub idempotency: OrchestratorHandoffIdempotency,
    pub handoff: OrchestratorHandoffBinding,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver: Option<JsonObject>,
    pub delivery: OrchestratorHandoffDelivery,
    pub receiver_validation: OrchestratorReceiverValidation,
    pub receipt_expectations: OrchestratorReceiptExpectations,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorHandoffIdempotency {
    pub key: String,
    pub receiver_should_dedupe: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorHandoffBinding {
    pub scope: String,
    pub audience: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorHandoffDelivery {
    pub event_id: String,
    pub handoff_scope: String,
    pub handoff_audience: String,
    pub execution_context: OrchestratorExecutionContext,
    pub payload: JsonValue,
    pub source: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorReceiverValidation {
    pub require_scope: String,
    pub require_audience: String,
    pub require_event_id: String,
    pub reject_duplicate_event_id: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OrchestratorReceiptExpectations {
    pub context_artifact: String,
    pub outbound_effect_must_be_receipted: bool,
    pub receiver_response_must_be_captured: bool,
    pub delivered_credential_material_absent: bool,
}
