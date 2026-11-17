//! A driven turn's telemetry and its sanitized failures.

use runx_contracts::{JsonObject, JsonValue, SealReason};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentExecutionTelemetry {
    pub rounds: Option<u64>,
    pub model_calls: Option<u64>,
    /// The tokens its model calls read and wrote, as the provider reported
    /// them; absent when any call reported none.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub tool_calls: Option<u64>,
    pub tools: Option<Vec<String>>,
    pub tool_executions: Option<Vec<AgentToolExecutionTrace>>,
}

impl AgentExecutionTelemetry {
    /// What the turn used, as its record states it whether the turn answers
    /// or fails: none before its first model call.
    #[must_use]
    pub fn usage(&self) -> Option<runx_contracts::TurnUsage> {
        use runx_contracts::schema::NonEmptyString;
        Some(runx_contracts::TurnUsage {
            model_calls: self.model_calls.and_then(std::num::NonZeroU64::new)?,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            rounds: self.rounds.and_then(std::num::NonZeroU64::new),
            calls: self
                .tool_executions
                .iter()
                .flatten()
                .filter_map(|call| {
                    Some(runx_contracts::TurnUsageCall {
                        tool: NonEmptyString::new(call.tool.clone())?,
                        status: NonEmptyString::new(call.status.clone())?,
                        kind: call.resolution_kind.clone().and_then(NonEmptyString::new),
                        rule: call.rule.clone().and_then(NonEmptyString::new),
                    })
                })
                .collect(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolExecutionTrace {
    pub tool: String,
    pub status: String,
    pub receipt_id: Option<String>,
    pub resolution_kind: Option<String>,
    /// For a final result the output contract refused, the rule it broke,
    /// in the contract's own words: the refused value never enters a trace.
    pub rule: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveError {
    reason: SealReason,
    sanitized_message: String,
    next: Option<String>,
    telemetry: Option<Box<AgentExecutionTelemetry>>,
}

impl DriveError {
    #[must_use]
    pub fn sanitized(message: impl Into<String>) -> Self {
        Self {
            reason: SealReason::DriveResolutionFailed,
            sanitized_message: message.into(),
            next: None,
            telemetry: None,
        }
    }

    /// The failure, saying what to do about it.
    #[must_use]
    pub fn next(mut self, next: impl Into<String>) -> Self {
        self.next = Some(next.into());
        self
    }

    /// What to do about it, when it says.
    #[must_use]
    pub fn next_step(&self) -> Option<&str> {
        self.next.as_deref()
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
            next: None,
            telemetry: Some(Box::new(telemetry)),
        }
    }

    /// Why the driven turn stopped, as the receipt seals it.
    #[must_use]
    pub const fn reason(&self) -> SealReason {
        self.reason
    }

    #[must_use]
    pub fn telemetry(&self) -> Option<&AgentExecutionTelemetry> {
        self.telemetry.as_deref()
    }

    /// The record a failed turn keeps: its code, the seal reason, its
    /// sentence, and its usage, the same as a turn that answered.
    #[must_use]
    pub(crate) fn public_failure_projection(&self) -> JsonObject {
        let mut projection = JsonObject::from([
            (
                "code".to_owned(),
                JsonValue::String(self.reason.label().to_owned()),
            ),
            (
                "reason".to_owned(),
                JsonValue::String(self.sanitized_message.clone()),
            ),
        ]);
        if let Some(usage) = self
            .telemetry()
            .and_then(AgentExecutionTelemetry::usage)
            .and_then(|usage| JsonValue::from_serialize(&usage).ok())
        {
            projection.insert("usage".to_owned(), usage);
        }
        projection
    }
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.sanitized_message)
    }
}

impl std::error::Error for DriveError {}
