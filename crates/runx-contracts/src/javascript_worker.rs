use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

pub const PROTOCOL_VERSION: u16 = 3;
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FRAME_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_STDERR_BYTES: usize = 64 * 1024;
pub const MAX_QUEUED_JOBS: u32 = 4_096;
pub const DEFAULT_WALL_MILLISECONDS: u64 = 2_000;
pub const MAX_WALL_MILLISECONDS: u64 = 30_000;
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
    pub wall_milliseconds: u64,
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
            wall_milliseconds: DEFAULT_WALL_MILLISECONDS,
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
            || self.wall_milliseconds == 0
            || self.wall_milliseconds > MAX_WALL_MILLISECONDS
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerRequest {
    Hello { protocol_version: u16 },
    Invoke(Box<WorkerInvocationRequest>),
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
    Failure {
        protocol_version: u16,
        invocation_id: Option<String>,
        code: WorkerFailureCode,
        message: String,
        disposition: WorkerDisposition,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerDisposition {
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

impl WorkerFailureCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidProtocol => "invalid_protocol",
            Self::InvalidRequest => "invalid_request",
            Self::ResourceLimit => "resource_limit",
            Self::ModuleRejected => "module_rejected",
            Self::ExecutionFailed => "execution_failed",
            Self::OutputRejected => "output_rejected",
            Self::InternalFailure => "internal_failure",
        }
    }
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
