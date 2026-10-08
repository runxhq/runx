#[cfg(feature = "async-http")]
use std::time::Instant;

use runx_contracts::{JsonObject, JsonValue};

use crate::adapter::{InvocationOutput, InvocationStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdapterProjection {
    duration_ms: u64,
}

impl AdapterProjection {
    pub(crate) const fn from_duration_ms(duration_ms: u64) -> Self {
        Self { duration_ms }
    }

    #[cfg(feature = "async-http")]
    pub(crate) fn from_started(started: Instant) -> Self {
        Self::from_duration_ms(duration_ms(started))
    }

    pub(crate) fn runtime_output(
        &self,
        status: InvocationStatus,
        value: JsonValue,
        failure: Option<String>,
        metadata: JsonObject,
    ) -> InvocationOutput {
        match status {
            InvocationStatus::Success => {
                InvocationOutput::runtime_success(value, self.duration_ms, metadata)
            }
            InvocationStatus::Failure => InvocationOutput::runtime_failure(
                value,
                failure.unwrap_or_else(|| "runtime invocation failed".to_owned()),
                self.duration_ms,
                metadata,
            ),
        }
    }

    #[cfg(feature = "process-tool")]
    pub(crate) fn process_output(
        &self,
        status: InvocationStatus,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        metadata: JsonObject,
    ) -> InvocationOutput {
        InvocationOutput::process(
            status,
            stdout,
            stderr,
            exit_code,
            self.duration_ms,
            metadata,
        )
    }

    #[cfg(feature = "async-http")]
    pub(crate) fn failure(self, message: String, metadata: JsonObject) -> InvocationOutput {
        self.runtime_output(
            InvocationStatus::Failure,
            JsonValue::Null,
            Some(message),
            metadata,
        )
    }
}

#[cfg(feature = "async-http")]
pub(crate) fn duration_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
