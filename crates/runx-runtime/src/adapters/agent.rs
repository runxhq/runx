//! The managed agent's telemetry and its sanitized failures.

use runx_contracts::{JsonNumber, JsonObject, JsonValue, SealReason};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentExecutionTelemetry {
    pub rounds: Option<u64>,
    pub model_calls: Option<u64>,
    pub tool_calls: Option<u64>,
    pub tools: Option<Vec<String>>,
    pub tool_executions: Option<Vec<AgentToolExecutionTrace>>,
}

impl AgentExecutionTelemetry {
    #[must_use]
    pub fn public_projection(&self) -> JsonObject {
        let mut projection = JsonObject::new();
        if let Some(rounds) = self.rounds {
            projection.insert(
                "rounds".to_owned(),
                JsonValue::Number(JsonNumber::U64(rounds)),
            );
        }
        if let Some(model_calls) = self.model_calls {
            projection.insert(
                "model_calls".to_owned(),
                JsonValue::Number(JsonNumber::U64(model_calls)),
            );
        }
        if let Some(tool_calls) = self.tool_calls {
            projection.insert(
                "tool_calls".to_owned(),
                JsonValue::Number(JsonNumber::U64(tool_calls)),
            );
        }
        if let Some(tools) = &self.tools {
            projection.insert(
                "tools".to_owned(),
                JsonValue::Array(tools.iter().cloned().map(JsonValue::String).collect()),
            );
        }
        if let Some(tool_executions) = &self.tool_executions {
            projection.insert(
                "tool_executions".to_owned(),
                JsonValue::Array(
                    tool_executions
                        .iter()
                        .map(tool_execution_trace)
                        .collect::<Vec<_>>(),
                ),
            );
        }
        projection
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolExecutionTrace {
    pub tool: String,
    pub status: String,
    pub receipt_id: Option<String>,
    pub resolution_kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedAgentError {
    reason: SealReason,
    sanitized_message: String,
    telemetry: Option<Box<AgentExecutionTelemetry>>,
}

impl ManagedAgentError {
    #[must_use]
    pub fn sanitized(message: impl Into<String>) -> Self {
        Self {
            reason: SealReason::ManagedAgentResolutionFailed,
            sanitized_message: message.into(),
            telemetry: None,
        }
    }

    #[must_use]
    pub fn bounded_failure(
        reason: SealReason,
        sanitized_message: impl Into<String>,
        telemetry: AgentExecutionTelemetry,
    ) -> Self {
        Self {
            reason,
            sanitized_message: sanitized_message.into(),
            telemetry: Some(Box::new(telemetry)),
        }
    }

    /// Why the managed agent stopped, as the receipt seals it.
    #[must_use]
    pub const fn reason(&self) -> SealReason {
        self.reason
    }

    #[must_use]
    pub fn sanitized_message(&self) -> &str {
        &self.sanitized_message
    }

    #[must_use]
    pub fn telemetry(&self) -> Option<&AgentExecutionTelemetry> {
        self.telemetry.as_deref()
    }

    #[must_use]
    pub(crate) fn public_failure_projection(&self) -> JsonObject {
        let mut projection = JsonObject::from([
            (
                "schema".to_owned(),
                JsonValue::String("runx.managed_agent_failure.v1".to_owned()),
            ),
            ("status".to_owned(), JsonValue::String("failed".to_owned())),
            (
                "reason".to_owned(),
                JsonValue::String(self.reason.label().to_owned()),
            ),
            (
                "message".to_owned(),
                JsonValue::String(self.sanitized_message.clone()),
            ),
        ]);
        if let Some(telemetry) = self.telemetry() {
            projection.insert(
                "telemetry".to_owned(),
                JsonValue::Object(telemetry.public_projection()),
            );
        }
        projection
    }
}

impl std::fmt::Display for ManagedAgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.sanitized_message)
    }
}

impl std::error::Error for ManagedAgentError {}

fn tool_execution_trace(trace: &AgentToolExecutionTrace) -> JsonValue {
    let mut object = JsonObject::new();
    object.insert("tool".to_owned(), JsonValue::String(trace.tool.clone()));
    object.insert("status".to_owned(), JsonValue::String(trace.status.clone()));
    if let Some(receipt_id) = &trace.receipt_id {
        object.insert(
            "receiptId".to_owned(),
            JsonValue::String(receipt_id.clone()),
        );
    }
    if let Some(resolution_kind) = &trace.resolution_kind {
        object.insert(
            "resolutionKind".to_owned(),
            JsonValue::String(resolution_kind.clone()),
        );
    }
    JsonValue::Object(object)
}
