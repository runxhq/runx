use std::time::Instant;

use runx_contracts::JsonValue;

use crate::adapter::{InvocationOutput, InvocationStatus};
use crate::observations::Observations;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdapterProjection {
    duration_ms: u64,
}

impl AdapterProjection {
    pub(crate) const fn from_duration_ms(duration_ms: u64) -> Self {
        Self { duration_ms }
    }

    pub(crate) fn from_started(started: Instant) -> Self {
        Self::from_duration_ms(duration_ms(started))
    }

    pub(crate) fn runtime_output(
        &self,
        status: InvocationStatus,
        value: JsonValue,
        failure: Option<String>,
        observations: Observations,
    ) -> InvocationOutput {
        let mut output = match status {
            InvocationStatus::Success => InvocationOutput::runtime_success(value, self.duration_ms),
            InvocationStatus::Failure => InvocationOutput::runtime_failure(
                value,
                failure.unwrap_or_else(|| "runtime invocation failed".to_owned()),
                self.duration_ms,
            ),
        };
        output.observations = observations;
        output
    }

    pub(crate) fn process_output(
        &self,
        status: InvocationStatus,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        observations: Observations,
    ) -> InvocationOutput {
        let mut output =
            InvocationOutput::process(status, stdout, stderr, exit_code, self.duration_ms);
        output.observations = observations;
        output
    }

    pub(crate) fn failure(self, message: String, observations: Observations) -> InvocationOutput {
        self.runtime_output(
            InvocationStatus::Failure,
            JsonValue::Null,
            Some(message),
            observations,
        )
    }
}

pub(crate) fn duration_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
