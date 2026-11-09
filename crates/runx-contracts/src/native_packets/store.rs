//! What the runtime's own `store` and `private` connectors answer
//! (`store.md`): one shape per operation, each naming the stream and key it
//! covers, the backend that served it, and the digests a receipt binds.

use serde::{Deserialize, Serialize};

use crate::json::MAX_PORTABLE_INTEGER;
use crate::schema::{
    BoundedString, BoundedU64, BoundedVec, IsoDateTime, NonEmptyString, RunxSchema,
};
use crate::{Digest, JsonObject};

/// A key's version: an integer every JSON reader keeps exactly, so at most
/// 2^53 - 1, which the store refuses to pass.
pub type StoreVersion = BoundedU64<0, MAX_PORTABLE_INTEGER>;
/// A full stream, `<package>/<stream>`: non-empty, and within the 512
/// characters admission allows a rendered target.
pub type StoreStream = BoundedString<512>;
/// A key, within the 192 characters an operation's input allows.
pub type StoreKey = BoundedString<192>;
/// An event's type, within the 128 characters an append's input allows.
pub type StoreEventType = BoundedString<128>;

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
    pub package: NonEmptyString,
    pub digest: Digest,
}

/// One event of a key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreEvent {
    /// A length-delimited encoding of stream, key and version.
    pub id: NonEmptyString,
    pub version: StoreVersion,
    /// Declared by the writer, never guessed.
    #[serde(rename = "type")]
    pub event_type: StoreEventType,
    pub data: JsonObject,
    /// Covers the writer, the type and the data.
    pub digest: Digest,
    pub idempotency_key: NonEmptyString,
    /// When the store committed it; a fact's own date is in its data.
    pub committed_at: IsoDateTime,
    /// The package that wrote it and the digest of the exact bytes it ran
    /// from, set by the runtime: every event records one.
    pub writer: StoreWriter,
}

/// An append that committed or replayed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreAppended {
    pub schema: StoreResultSchema,
    pub operation: StoreAppendOperation,
    /// The full stream, `<package>/<stream>`.
    pub stream: StoreStream,
    pub key: StoreKey,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    pub status: StoreAppliedStatus,
    pub before_version: StoreVersion,
    pub version: StoreVersion,
    pub event: StoreEventRef,
    pub chain_digest: Digest,
}

/// An append that conflicted, appending nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreConflicted {
    pub schema: StoreResultSchema,
    pub operation: StoreAppendOperation,
    pub stream: StoreStream,
    pub key: StoreKey,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    pub status: StoreConflictStatus,
    pub before_version: StoreVersion,
    pub version: StoreVersion,
    pub chain_digest: Digest,
}

/// One page of a key's events, in version order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreRead {
    pub schema: StoreResultSchema,
    pub operation: StoreReadOperation,
    pub stream: StoreStream,
    pub key: StoreKey,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    /// The key's version.
    pub version: StoreVersion,
    /// At most 500, a read's largest page.
    pub events: BoundedVec<StoreEvent, 0, 500>,
    /// The last version in this page, which the next read passes as
    /// `after_version`; with no events, the version read after.
    pub last_version: StoreVersion,
    pub has_more: bool,
    pub chain_digest: Digest,
}

/// A key's version, its newest event whole, and its chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreInspected {
    pub schema: StoreResultSchema,
    pub operation: StoreInspectOperation,
    pub stream: StoreStream,
    pub key: StoreKey,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    pub version: StoreVersion,
    /// The key's newest event, or null at version 0.
    pub last_event: Option<StoreEvent>,
    pub chain_digest: Digest,
}

/// A listed key and its newest event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreListedKey {
    pub key: StoreKey,
    pub last_event: StoreEvent,
}

/// One page of a stream's keys, newest commit first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreListed {
    pub schema: StoreResultSchema,
    pub operation: StoreListOperation,
    pub stream: StoreStream,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    /// At most 100, a listing's largest page.
    pub keys: BoundedVec<StoreListedKey, 0, 100>,
    /// Where the next page starts, or null when there's no further page.
    pub next_cursor: Option<String>,
}

/// A key a search matched, by its newest event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreMatch {
    pub key: StoreKey,
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
    pub stream: StoreStream,
    pub backend: StoreBackend,
    pub result_digest: Digest,
    /// At most 100, a search's largest page.
    pub matches: BoundedVec<StoreMatch, 0, 100>,
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::StoreResult;
    use crate::schema::RunxSchema;

    const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    /// 2^53 − 1, the largest integer every JSON reader keeps exactly.
    const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

    fn event() -> Value {
        json!({
            "id": "6:runx/x,1:k,1", "version": 1, "type": "notes.kept", "data": {},
            "digest": DIGEST, "idempotency_key": "runx:sha256:abc",
            "committed_at": "2026-11-09T00:00:00.000Z",
            "writer": {"package": "runx/notes", "digest": DIGEST},
        })
    }

    fn appended(status: &str) -> Value {
        json!({
            "schema": "runx.store.result.v1", "operation": "events.append",
            "stream": "runx/notes/said", "key": "k", "backend": "sqlite",
            "result_digest": DIGEST, "status": status, "before_version": 0, "version": 1,
            "event": {"id": "6:runx/x,1:k,1", "digest": DIGEST}, "chain_digest": DIGEST,
        })
    }

    fn read(events: Vec<Value>) -> Value {
        json!({
            "schema": "runx.store.result.v1", "operation": "events.read",
            "stream": "runx/notes/said", "key": "k", "backend": "sqlite",
            "result_digest": DIGEST, "version": 1, "events": events, "last_version": 1,
            "has_more": false, "chain_digest": DIGEST,
        })
    }

    fn with(mut value: Value, path: &[&str], field: Value) -> Value {
        let mut at = &mut value;
        for key in &path[..path.len() - 1] {
            at = &mut at[*key];
        }
        at[path[path.len() - 1]] = field;
        value
    }

    fn admits(validator: &jsonschema::Validator, value: &Value) -> bool {
        let valid = validator.is_valid(value);
        assert_eq!(
            valid,
            serde_json::from_value::<StoreResult>(value.clone()).is_ok(),
            "the schema and decoding agree on {value}"
        );
        valid
    }

    /// store.md's rules the code means, carried by the generated schema:
    /// versions within 2^53 − 1, every event with its writer, an event only
    /// with a status that has one,
    /// a non-empty stream, a key and a type within their input bounds, a
    /// non-empty event id and idempotency key, a date-time commit and a page
    /// within its operation's limit.
    #[test]
    fn a_store_result_keeps_store_mds_rules() -> Result<(), String> {
        let schema = StoreResult::json_schema();
        let validator = jsonschema::draft202012::options()
            .build(&schema)
            .map_err(|error| error.to_string())?;
        let admits = |value: &Value| admits(&validator, value);
        assert!(admits(&appended("committed")));
        assert!(admits(&read(vec![event()])));
        for status in ["version_conflict", "idempotency_conflict"] {
            assert!(!admits(&appended(status)), "a conflict carries no event");
        }
        let conflicted = appended("version_conflict")
            .as_object()
            .map(|object| {
                let mut object = object.clone();
                object.remove("event");
                Value::Object(object)
            })
            .unwrap_or_default();
        assert!(admits(&conflicted));
        let refused = [
            with(
                appended("committed"),
                &["version"],
                json!(MAX_SAFE_INTEGER + 1),
            ),
            with(
                appended("committed"),
                &["before_version"],
                json!(MAX_SAFE_INTEGER + 1),
            ),
            with(
                read(vec![event()]),
                &["last_version"],
                json!(MAX_SAFE_INTEGER + 1),
            ),
            read(vec![with(
                event(),
                &["version"],
                json!(MAX_SAFE_INTEGER + 1),
            )]),
            with(appended("committed"), &["stream"], json!("")),
            with(appended("committed"), &["key"], json!("")),
            with(appended("committed"), &["key"], json!("k".repeat(193))),
            read(vec![with(event(), &["type"], json!("t".repeat(129)))]),
            read(vec![with(event(), &["type"], json!(""))]),
            read(vec![with(event(), &["id"], json!(""))]),
            read(vec![with(event(), &["idempotency_key"], json!(""))]),
            read(vec![with(event(), &["committed_at"], json!("yesterday"))]),
            read(vec![event(); 501]),
            // Every event records its writer: there's no migration, so no
            // event from before writers were recorded.
            read(vec![
                event()
                    .as_object()
                    .map(|object| {
                        let mut object = object.clone();
                        object.remove("writer");
                        Value::Object(object)
                    })
                    .unwrap_or_default(),
            ]),
        ];
        for value in &refused {
            assert!(!admits(value), "{value}");
        }
        assert!(admits(&with(
            appended("committed"),
            &["version"],
            json!(MAX_SAFE_INTEGER)
        )));
        assert!(admits(&read(vec![event(); 500])));
        Ok(())
    }
}
