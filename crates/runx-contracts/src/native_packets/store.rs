//! What the runtime's own `store` and `private` connectors answer
//! (`store.md`): one shape per operation, each naming the stream and key it
//! covers, the backend that served it, and the digests a receipt binds.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;
use crate::{Digest, JsonObject};

/// One shape per operation, so an answer says only what its operation does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(untagged)]
#[runx_schema(
    id = "runx.store.result.v1",
    url = "https://schemas.runx.ai/runx/store/result/v1.json"
)]
pub enum StoreResult {
    Appended(StoreAppended),
    Conflicted(StoreConflicted),
    Read(StoreRead),
    Inspected(StoreInspected),
    Listed(StoreListed),
    Searched(StoreSearched),
}

/// Every answer's schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum StoreResultSchema {
    #[serde(rename = "runx.store.result.v1")]
    V1,
}

/// The backend that served an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoreBackend {
    Sqlite,
    Postgres,
    Redis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum StoreAppendOperation {
    #[serde(rename = "events.append")]
    EventsAppend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum StoreReadOperation {
    #[serde(rename = "events.read")]
    EventsRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum StoreInspectOperation {
    #[serde(rename = "key.inspect")]
    KeyInspect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum StoreListOperation {
    #[serde(rename = "keys.list")]
    KeysList,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum StoreSearchOperation {
    #[serde(rename = "keys.search")]
    KeysSearch,
}

/// An append that landed: `committed`, or `replayed`, the same idempotency
/// key with the same event, answered as the first time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoreAppliedStatus {
    Committed,
    Replayed,
}

/// An append that appended nothing: `version_conflict`, the key wasn't at
/// the expected version, retried after a read; or `idempotency_conflict`,
/// the idempotency key was used for a different event, checked first and
/// never retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoreConflictStatus {
    VersionConflict,
    IdempotencyConflict,
}

/// The event an append committed or replayed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreEventRef {
    pub id: String,
    pub digest: Digest,
}

/// Who wrote an event: the package and the digest of the exact bytes it ran
/// from, set by the runtime, never by a caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreWriter {
    pub package: String,
    pub digest: Digest,
}

/// One event of a key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreEvent {
    /// A length-delimited encoding of stream, key and version.
    pub id: String,
    pub version: u64,
    /// Declared by the writer, never guessed.
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: JsonObject,
    /// Covers the writer, the type and the data; an event written before
    /// writers were recorded covers its data alone.
    pub digest: Digest,
    pub idempotency_key: String,
    /// When the store committed it; a fact's own date is in its data.
    pub committed_at: String,
    /// Absent on an event written before 0.12.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<StoreWriter>,
}

/// An append that committed or replayed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreAppended {
    pub schema: StoreResultSchema,
    pub operation: StoreAppendOperation,
    /// The full stream, `<package>/<stream>`.
    pub stream: String,
    pub key: String,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    pub status: StoreAppliedStatus,
    pub before_version: u64,
    pub version: u64,
    pub event: StoreEventRef,
    pub chain_digest: Digest,
}

/// An append that conflicted, appending nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreConflicted {
    pub schema: StoreResultSchema,
    pub operation: StoreAppendOperation,
    pub stream: String,
    pub key: String,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    pub status: StoreConflictStatus,
    pub before_version: u64,
    pub version: u64,
    pub chain_digest: Digest,
}

/// One page of a key's events, in version order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreRead {
    pub schema: StoreResultSchema,
    pub operation: StoreReadOperation,
    pub stream: String,
    pub key: String,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    /// The key's version.
    pub version: u64,
    pub events: Vec<StoreEvent>,
    /// The last version in this page, which the next read passes as
    /// `after_version`; with no events, the version read after.
    pub last_version: u64,
    pub has_more: bool,
    pub chain_digest: Digest,
}

/// A key's version, its newest event whole, and its chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreInspected {
    pub schema: StoreResultSchema,
    pub operation: StoreInspectOperation,
    pub stream: String,
    pub key: String,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    pub version: u64,
    /// The key's newest event, or null at version 0.
    pub last_event: Option<StoreEvent>,
    pub chain_digest: Digest,
}

/// A listed key and its newest event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreListedKey {
    pub key: String,
    pub last_event: StoreEvent,
}

/// One page of a stream's keys, newest commit first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreListed {
    pub schema: StoreResultSchema,
    pub operation: StoreListOperation,
    pub stream: String,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    pub keys: Vec<StoreListedKey>,
    /// Where the next page starts, or null when there's no further page.
    pub next_cursor: Option<String>,
}

/// A key a search matched, by its newest event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreMatch {
    pub key: String,
    pub score: f64,
    pub last_event: StoreEvent,
}

/// One page of a search of one stream's current entries, in score order
/// with ties by key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreSearched {
    pub schema: StoreResultSchema,
    pub operation: StoreSearchOperation,
    pub stream: String,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    pub matches: Vec<StoreMatch>,
    pub next_cursor: Option<String>,
}
