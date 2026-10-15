//! Public packets emitted by reusable native runtime capabilities.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{JsonObject, JsonValue};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.local_artifact.v1",
    url = "https://schemas.runx.ai/runx/local-artifact/v1.json"
)]
pub struct LocalArtifact {
    pub artifact_ref: String,
    pub media_type: String,
    pub bytes: u64,
    pub whole_digest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.local_artifact.page.v1",
    url = "https://schemas.runx.ai/runx/local-artifact/page/v1.json"
)]
pub struct LocalArtifactPage {
    pub artifact_ref: String,
    pub media_type: String,
    pub offset: u64,
    pub length: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub range_digest: String,
    pub whole_digest: String,
    pub encoding: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.git.blob_digest.v1",
    url = "https://schemas.runx.ai/runx/git/blob-digest/v1.json"
)]
pub struct GitBlobDigest {
    pub algorithm: String,
    pub digest: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.data.operation_result.v1",
    url = "https://schemas.runx.ai/runx/data/operation-result/v1.json"
)]
pub struct DataOperationResult {
    pub schema: String,
    pub data_source_ref: String,
    pub provider: String,
    pub operation: String,
    pub resource: String,
    pub aggregate_id: String,
    pub status: DataOperationStatus,
    pub before_version: u64,
    pub after_version: u64,
    pub idempotency_key: Option<String>,
    pub event_ref: Option<String>,
    pub event_digest: Option<String>,
    pub result_digest: String,
    pub projection_digest: String,
    pub projection: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_after_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    pub events: Vec<JsonValue>,
    pub rows: Vec<JsonValue>,
    pub redactions: Vec<JsonValue>,
    pub stop_conditions: Vec<DataStopCondition>,
    pub provider_evidence: JsonObject,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataOperationStatus {
    Committed,
    IdempotentReplay,
    Read,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct DataStopCondition {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.approval.decision.v1",
    url = "https://schemas.runx.ai/runx/approval/decision/v1.json"
)]
pub struct ApprovalDecisionPacket {
    pub approved: Option<bool>,
    pub gate_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_type: Option<String>,
    pub idempotency_key: String,
    pub status: ApprovalDecisionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<ApprovalDecisionActor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionStatus {
    Approved,
    Denied,
    Pending,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionActor {
    Human,
    Agent,
}

/// A process runx supervises apart from any driver: started by one call, and
/// read, written and stopped by later ones until it exits. It belongs to the
/// actor whose run started it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.process.v1",
    url = "https://schemas.runx.ai/runx/process/v1.json"
)]
pub struct RunxProcess {
    pub process_id: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// The absolute directory it runs in.
    pub cwd: String,
    /// The terminal it runs in, when it was given one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tty: Option<TerminalSize>,
    /// How long its supervisor lets it run before stopping it.
    pub max_seconds: u64,
    /// The actor whose runs may read, write and stop it; none for an
    /// operator's run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub created_at: String,
    pub state: ProcessState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<ProcessExit>,
}

/// A terminal's size, in character cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

/// Where a process stands: recorded and waiting on its supervisor, running
/// under it, or exited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Starting,
    Running,
    Exited,
}

/// How a process ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessExit {
    pub reason: ProcessExitReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    /// Why it failed, for a process that never ran or whose supervisor ended
    /// without recording its exit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessExitReason {
    /// It exited on its own.
    Exited,
    /// It was stopped.
    Stopped,
    /// Its supervisor stopped it at `max_seconds`.
    TimedOut,
    /// It never ran, or its supervisor ended without recording its exit.
    Failed,
}

/// What a process wrote after a cursor, as its log keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.process.output.v1",
    url = "https://schemas.runx.ai/runx/process/output/v1.json"
)]
pub struct ProcessOutput {
    pub process_id: String,
    pub state: ProcessState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<ProcessExit>,
    /// Where this read began: the cursor asked for, or the oldest output the
    /// log still keeps when that is later.
    pub from: u64,
    /// The cursor the next read passes as `after`.
    pub next: u64,
    /// Bytes written before `from` that the log no longer keeps.
    pub dropped: u64,
    pub chunks: Vec<ProcessOutputChunk>,
}

/// Consecutive output of one stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessOutputChunk {
    pub stream: ProcessStream,
    pub text: String,
}

/// A process's stdout or stderr, or the terminal a `tty` process writes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStream {
    Stdout,
    Stderr,
    Terminal,
}

/// The processes an actor's runs may reach in a project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.process.list.v1",
    url = "https://schemas.runx.ai/runx/process/list/v1.json"
)]
pub struct ProcessList {
    pub processes: Vec<RunxProcess>,
}
