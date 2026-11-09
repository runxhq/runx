use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

pub const PROTOCOL_VERSION: u16 = 4;

/// Authority-free helpers exposed by every matching supervised worker.
/// Profiles declare the helpers they need so unsupported runtimes reject them
/// during workflow admission, before any step executes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerApi {
    ParseUrl,
    ParseData,
}

impl WorkerApi {
    pub const fn name(self) -> &'static str {
        match self {
            Self::ParseUrl => "parseUrl",
            Self::ParseData => "parseData",
        }
    }
}
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FRAME_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_STDERR_BYTES: usize = 64 * 1024;
pub const MAX_QUEUED_JOBS: u32 = 4_096;
/// The CPU time one invocation may spend by default, and at most: the
/// supervisor reads it from the worker process's CPU clock.
pub const DEFAULT_CPU_MILLISECONDS: u64 = 2_000;
pub const MAX_CPU_MILLISECONDS: u64 = 30_000;
/// Maximum number of isolated JavaScript worker processes retained by one
/// runtime. Each process executes exactly one invocation at a time.
pub const MAX_WORKER_POOL_SIZE: usize = 4;
pub const JAVASCRIPT_HEAP_BYTES: u64 = 64 * 1024 * 1024;
pub const JAVASCRIPT_STACK_BYTES: usize = 4 * 1024 * 1024;
// glibc can reserve 64-128 MiB of non-committed address space for allocator
// arenas and loader mappings. The AS guard leaves conservative cross-libc
// headroom; the working-set limit below is the committed-memory boundary.
pub const WORKER_VIRTUAL_ADDRESS_SPACE_BYTES: u64 = 1024 * 1024 * 1024;
pub const WORKER_WORKING_SET_BYTES: u64 = 160 * 1024 * 1024;
/// Explicit host working-set budget implied by the bounded worker pool.
pub const AGGREGATE_WORKER_WORKING_SET_BYTES: u64 =
    WORKER_WORKING_SET_BYTES * MAX_WORKER_POOL_SIZE as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationLimits {
    pub source_bytes: usize,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub heap_bytes: u64,
    pub stack_bytes: usize,
    /// The CPU time the invocation may spend, which the supervisor enforces.
    pub cpu_milliseconds: u64,
    pub queued_jobs: u32,
}

impl Default for InvocationLimits {
    fn default() -> Self {
        Self {
            source_bytes: MAX_SOURCE_BYTES,
            input_bytes: MAX_INPUT_BYTES,
            output_bytes: MAX_OUTPUT_BYTES,
            heap_bytes: JAVASCRIPT_HEAP_BYTES,
            stack_bytes: JAVASCRIPT_STACK_BYTES,
            cpu_milliseconds: DEFAULT_CPU_MILLISECONDS,
            queued_jobs: MAX_QUEUED_JOBS,
        }
    }
}

impl InvocationLimits {
    pub fn validate(self) -> Result<Self, ProtocolError> {
        let maximum = Self::default();
        if self.source_bytes == 0
            || self.source_bytes > maximum.source_bytes
            || self.input_bytes == 0
            || self.input_bytes > maximum.input_bytes
            || self.output_bytes == 0
            || self.output_bytes > maximum.output_bytes
            || self.heap_bytes != maximum.heap_bytes
            || self.stack_bytes == 0
            || self.stack_bytes > maximum.stack_bytes
            || self.cpu_milliseconds == 0
            || self.cpu_milliseconds > MAX_CPU_MILLISECONDS
            || self.queued_jobs == 0
            || self.queued_jobs > maximum.queued_jobs
        {
            return Err(ProtocolError::InvalidLimits);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerInvocationRequest {
    pub protocol_version: u16,
    pub invocation_id: String,
    pub entry_module: String,
    pub export_name: String,
    pub modules: BTreeMap<String, String>,
    pub inputs: serde_json::Value,
    /// Exact values selected by the manifest environment declaration. The
    /// worker OS environment remains empty.
    pub environment: BTreeMap<String, String>,
    pub limits: InvocationLimits,
}

/// One pass of a script step: its module run from the beginning against
/// what its calls settled, delivered one at a time in the order they settled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptPassRequest {
    pub protocol_version: u16,
    pub invocation_id: String,
    pub entry_module: String,
    pub export_name: String,
    pub modules: BTreeMap<String, String>,
    pub inputs: serde_json::Value,
    pub limits: InvocationLimits,
    /// What the script's calls settled to, in the order they settled.
    pub settled: Vec<ScriptSettlement>,
    /// What a `ctx.now()` the record doesn't hold yet reads: milliseconds
    /// since the epoch.
    pub now: i64,
}

/// Further settlements for the live script a pass left waiting on this
/// worker, delivered one at a time in the order they settled. The answer
/// reports only the calls made since the last.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptSettleRequest {
    pub protocol_version: u16,
    /// The pass that started the live script.
    pub invocation_id: String,
    pub settled: Vec<ScriptSettlement>,
    /// What a `ctx.now()` read from here reads.
    pub now: i64,
}

/// How the call at `position` settled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptSettlement {
    pub position: u32,
    pub outcome: ScriptCallOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScriptCallOutcome {
    /// The call returned `value`. Each field `references` names reaches the
    /// script as a reference to its artifact in place of the value, which
    /// `ctx.read` reads.
    Resolved {
        value: serde_json::Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        references: Vec<ScriptReference>,
    },
    /// The call didn't succeed: the script's promise rejects with
    /// `{ reason, code, message }`.
    Rejected {
        reason: ScriptRejection,
        code: String,
        message: String,
    },
    /// What `ctx.now()` read at this position.
    Now { millis: i64 },
}

/// A field of a call's result past the kernel's inline bound, which reaches
/// the script as a reference to the artifact that holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptReference {
    pub field: String,
    pub artifact: crate::Artifact,
}

/// Why a call rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptRejection {
    /// Admission refused it: authority, `calls`, the spending limit, depth.
    Refused,
    /// It ran and failed.
    Failed,
    /// A person declined its approval.
    Declined,
    /// Its host cancelled it before it settled.
    Cancelled,
    /// A read of a reference whose value is past the worker's input bound.
    TooLarge,
}

/// A call a script made, at its position in the step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptCall {
    pub position: u32,
    pub target: ScriptCallTarget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScriptCallTarget {
    /// `ctx.skill(reference, inputs)`: a method, `skill#method`, as a child run.
    Skill {
        reference: String,
        inputs: serde_json::Value,
    },
    /// `ctx.tool(name, inputs)`: a catalog tool.
    Tool {
        name: String,
        inputs: serde_json::Value,
    },
    /// `ctx.operation("<connector>/<operation>", inputs, { idempotency_key })`.
    Operation {
        operation: String,
        inputs: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    /// `ctx.now()`, and what it read.
    Now { millis: i64 },
    /// `ctx.wait({ until })`, in the milliseconds `ctx.now()` reads. It
    /// resolves to the time it settled.
    Wait { until: i64 },
    /// `ctx.read(reference)`, of a reference a call's result gave the
    /// script, by its artifact's digest. It resolves to the value.
    Read { digest: String },
}

/// How a pass ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScriptEnding {
    /// The function returned `output`.
    Returned { output: serde_json::Value },
    /// The function threw, or its promise rejected.
    Threw { message: String },
    /// The function waits on calls that haven't settled.
    Waiting,
    /// A recorded settlement names a position the script didn't call: replay
    /// went another way, and nothing may act on it.
    Diverged { position: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerRequest {
    Hello {
        protocol_version: u16,
    },
    Invoke(Box<WorkerInvocationRequest>),
    /// A script run from the beginning against its record. One still waiting
    /// stays live on the worker until the next request that isn't its
    /// settlement.
    Pass(Box<ScriptPassRequest>),
    Settle(Box<ScriptSettleRequest>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerResponse {
    Ready {
        protocol_version: u16,
    },
    Result {
        protocol_version: u16,
        invocation_id: String,
        output: serde_json::Value,
    },
    /// A script pass or settlement: the calls made since the last report,
    /// in position order, and how the script stands.
    Passed {
        protocol_version: u16,
        invocation_id: String,
        calls: Vec<ScriptCall>,
        ending: ScriptEnding,
    },
    Failure {
        protocol_version: u16,
        invocation_id: Option<String>,
        code: WorkerFailureCode,
        message: String,
        fate: WorkerFate,
    },
}

/// What becomes of the worker after a call: reused for the next, or discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerFate {
    Reuse,
    Discard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerFailureCode {
    InvalidProtocol,
    InvalidRequest,
    ResourceLimit,
    ModuleRejected,
    ExecutionFailed,
    OutputRejected,
    InternalFailure,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("worker frame I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("worker frame length {actual} exceeds limit {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },
    #[error("worker frame JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("worker invocation limits are invalid or wider than runtime maxima")]
    InvalidLimits,
}

pub fn write_frame<T: Serialize>(
    writer: &mut impl Write,
    value: &T,
    maximum: usize,
) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(value)?;
    if payload.len() > maximum {
        return Err(ProtocolError::FrameTooLarge {
            actual: payload.len(),
            maximum,
        });
    }
    let length = u32::try_from(payload.len()).map_err(|_| ProtocolError::FrameTooLarge {
        actual: payload.len(),
        maximum,
    })?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

pub fn read_frame<T: DeserializeOwned>(
    reader: &mut impl Read,
    maximum: usize,
) -> Result<Option<T>, ProtocolError> {
    let mut length = [0_u8; 4];
    let mut read = 0usize;
    while read < length.len() {
        match reader.read(&mut length[read..])? {
            0 if read == 0 => return Ok(None),
            0 => {
                return Err(ProtocolError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "truncated worker frame length",
                )));
            }
            count => read += count,
        }
    }
    // Runx release targets are 32-bit or wider, so every frame's u32 length
    // is representable as usize.
    let length = u32::from_be_bytes(length) as usize;
    if length > maximum {
        return Err(ProtocolError::FrameTooLarge {
            actual: length,
            maximum,
        });
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Some(serde_json::from_slice(&payload)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_is_length_delimited() -> Result<(), ProtocolError> {
        let request = WorkerRequest::Hello {
            protocol_version: PROTOCOL_VERSION,
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request, MAX_FRAME_BYTES)?;
        let decoded = read_frame::<WorkerRequest>(&mut bytes.as_slice(), MAX_FRAME_BYTES)?;
        assert_eq!(decoded, Some(request));
        Ok(())
    }

    #[test]
    fn frame_reader_rejects_oversized_length_before_allocating() {
        let bytes = u32::MAX.to_be_bytes();
        let error = read_frame::<WorkerRequest>(&mut bytes.as_slice(), 1024)
            .err()
            .map(|error| error.to_string());
        assert!(error.is_some_and(|message| message.contains("exceeds limit")));
    }

    #[test]
    fn invocation_limits_only_attenuate_worker_maxima() {
        let limits = InvocationLimits {
            output_bytes: MAX_OUTPUT_BYTES + 1,
            ..InvocationLimits::default()
        };
        assert!(matches!(
            limits.validate(),
            Err(ProtocolError::InvalidLimits)
        ));
    }
}
