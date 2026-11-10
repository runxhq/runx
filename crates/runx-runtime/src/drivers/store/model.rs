//! Events, a key's state and the answers, the same for every backend
//! (`store.md`). Every event records its writer, and its digest covers its
//! writer, its type and its data, and on `private` whether it's standing and
//! its end date when it states them. A key's `chain_digest` chains its
//! events, its hash input keeping the field names it always had, so existing
//! chains still verify.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use runx_contracts::{JsonNumber, JsonObject, JsonValue, sha256_prefixed};

use super::StoreError;
use super::request::{Append, Key, validate_key};

/// What every answer names as its schema.
pub(super) const RESULT_SCHEMA: &str = "runx.store.result.v1";

/// Who wrote an event: the package the runtime established, and the digest
/// of the exact bytes it ran from. Never a caller's statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Writer {
    pub package: String,
    pub digest: String,
}

#[derive(Clone, Debug)]
pub(super) struct EventRecord {
    pub id: String,
    pub version: u64,
    pub event_type: String,
    pub data: JsonObject,
    pub digest: String,
    pub idempotency_key: String,
    pub committed_at: String,
    /// The package that wrote it, and the digest of the bytes it ran from.
    pub writer: Writer,
    /// On `private`, whether its key is standing while it's the key's
    /// newest event.
    pub standing: bool,
    /// On `private`, the last date it's valid, `YYYY-MM-DD`.
    pub valid_until: Option<String>,
    /// On `private`, the fields of its data a search matches its key by.
    pub searchable: Option<Vec<String>>,
}

/// One key's state: its version, its newest event, and the digest that
/// chains its events.
#[derive(Clone, Debug)]
pub(super) struct KeyState {
    pub version: u64,
    pub last: Option<EventRecord>,
    pub chain_digest: String,
}

/// A listed key and its newest event.
#[derive(Clone, Debug)]
pub(super) struct Listed {
    pub key: String,
    pub last: EventRecord,
}

/// A key a search matched, by its newest event, and how well.
#[derive(Clone, Debug)]
pub(super) struct Matched {
    pub key: String,
    pub last: EventRecord,
    pub score: f64,
}

/// What an append did.
pub(super) enum Appended {
    Committed {
        before: u64,
        record: EventRecord,
        state: KeyState,
    },
    /// The idempotency key already holds this exact event.
    Replayed {
        current: KeyState,
        existing: EventRecord,
    },
    /// The idempotency key holds a different event, whose digest is `held`.
    IdempotencyConflict { current: KeyState, held: String },
    /// The key is not at the expected version.
    VersionConflict { current: KeyState },
}

impl Appended {
    /// The digest of the event the store holds under the call's idempotency
    /// key, which a readback must find there: none after a version conflict.
    pub(super) fn landed(&self) -> Option<&str> {
        match self {
            Self::Committed { record, .. } => Some(&record.digest),
            Self::Replayed { existing, .. } => Some(&existing.digest),
            Self::IdempotencyConflict { held, .. } => Some(held),
            Self::VersionConflict { .. } => None,
        }
    }
}

/// An event's identity: its stream, key and version, each length-delimited,
/// so a `:` in a stream or a key never makes two events one.
pub(super) fn event_id(stream: &str, key: &str, version: u64) -> String {
    format!("{}:{stream},{}:{key},{version}", stream.len(), key.len())
}

/// The event `append` proposes at `version`, with its digest.
pub(super) fn record(
    append: &Append<'_>,
    version: u64,
    committed_at: &str,
) -> Result<EventRecord, StoreError> {
    let mut record = EventRecord {
        id: event_id(append.at.stream, append.at.key, version),
        version,
        event_type: append.event_type.to_owned(),
        data: append.data.clone(),
        digest: String::new(),
        idempotency_key: append.idempotency_key.to_owned(),
        committed_at: normalize_time(committed_at)?,
        writer: append.writer.clone(),
        standing: append.standing,
        valid_until: append.valid_until.map(str::to_owned),
        searchable: append
            .searchable
            .as_ref()
            .map(|fields| fields.iter().map(|field| (*field).to_owned()).collect()),
    };
    record.digest = event_digest(&record)?;
    Ok(record)
}

/// What an event's digest covers: its writer, its type and its data, and
/// whether it's standing, its end date and what's searchable when it states
/// them. An event that states none digests as every event did before any
/// was stated.
pub(super) fn event_digest(record: &EventRecord) -> Result<String, StoreError> {
    let mut covered = JsonObject::from([
        ("data".to_owned(), JsonValue::Object(record.data.clone())),
        ("type".to_owned(), text(&record.event_type)),
        ("writer".to_owned(), writer_object(&record.writer)),
    ]);
    stated(&mut covered, record);
    digest(&JsonValue::Object(covered))
}

/// Whether `record` is standing, its end date and what's searchable, into
/// `object`, each only when it states it.
fn stated(object: &mut JsonObject, record: &EventRecord) {
    if record.standing {
        object.insert("standing".to_owned(), JsonValue::Bool(true));
    }
    if let Some(until) = &record.valid_until {
        object.insert("valid_until".to_owned(), text(until));
    }
    if let Some(fields) = &record.searchable {
        object.insert(
            "searchable".to_owned(),
            JsonValue::Array(fields.iter().map(|field| text(field)).collect()),
        );
    }
}

pub(super) fn empty_state() -> Result<KeyState, StoreError> {
    Ok(KeyState {
        version: 0,
        last: None,
        chain_digest: empty_chain()?,
    })
}

/// The chain of a key with no events.
pub(super) fn empty_chain() -> Result<String, StoreError> {
    digest(&JsonValue::Object(JsonObject::from([
        ("version".to_owned(), number(0)),
        ("event_digest".to_owned(), JsonValue::Null),
    ])))
}

/// The chain after `event_digest` at `version` follows `previous`.
pub(super) fn chain(
    previous: &str,
    version: u64,
    event_digest: &str,
) -> Result<String, StoreError> {
    digest(&JsonValue::Object(JsonObject::from([
        ("version".to_owned(), number(version)),
        ("previous_projection_digest".to_owned(), text(previous)),
        ("event_digest".to_owned(), text(event_digest)),
    ])))
}

pub(super) fn advance(current: &KeyState, event: &EventRecord) -> Result<KeyState, StoreError> {
    Ok(KeyState {
        version: event.version,
        last: Some(event.clone()),
        chain_digest: chain(&current.chain_digest, event.version, &event.digest)?,
    })
}

/// An RFC 3339 time as UTC with milliseconds, so times order as text.
pub(super) fn normalize_time(value: &str) -> Result<String, StoreError> {
    let (days, seconds, nanos) = runx_core::policy::parse_rfc3339_moment(value)
        .ok_or_else(|| StoreError::invalid("a commit time must be RFC 3339"))?;
    let unix_seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(seconds))
        .ok_or_else(|| StoreError::invalid("a commit time is outside the supported range"))?;
    let whole = crate::time::iso8601_from_unix_seconds(unix_seconds);
    let prefix = whole.strip_suffix('Z').unwrap_or(&whole);
    Ok(format!("{prefix}.{:03}Z", nanos / 1_000_000))
}

pub(super) fn digest(value: &JsonValue) -> Result<String, StoreError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| StoreError::invalid(format!("encoding a digest input: {error}")))?;
    Ok(sha256_prefixed(&bytes))
}

/// A JSON number as an unsigned integer.
pub(super) fn unsigned(value: &JsonValue) -> Option<u64> {
    match value {
        JsonValue::Number(JsonNumber::U64(value)) => Some(*value),
        JsonValue::Number(JsonNumber::I64(value)) => u64::try_from(*value).ok(),
        _ => None,
    }
}

pub(super) fn text(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}

pub(super) fn number(value: u64) -> JsonValue {
    JsonValue::Number(JsonNumber::U64(value))
}

fn writer_object(writer: &Writer) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("package".to_owned(), text(&writer.package)),
        ("digest".to_owned(), text(&writer.digest)),
    ]))
}

/// An event as every answer and every JSON backend holds it.
pub(super) fn record_object(record: &EventRecord) -> JsonObject {
    let mut object = JsonObject::from([
        ("id".to_owned(), text(&record.id)),
        ("version".to_owned(), number(record.version)),
        ("type".to_owned(), text(&record.event_type)),
        ("data".to_owned(), JsonValue::Object(record.data.clone())),
        ("digest".to_owned(), text(&record.digest)),
        ("idempotency_key".to_owned(), text(&record.idempotency_key)),
        ("committed_at".to_owned(), text(&record.committed_at)),
        ("writer".to_owned(), writer_object(&record.writer)),
    ]);
    stated(&mut object, record);
    object
}

/// A record back from the object [`record_object`] wrote, as a backend that
/// keeps JSON stores it.
#[cfg(feature = "store")]
pub(super) fn record_from_object(value: &JsonValue) -> Result<EventRecord, StoreError> {
    let object = value
        .as_object()
        .ok_or_else(|| StoreError::corrupt("a stored event is not an object"))?;
    let field = |name: &str| {
        object
            .get(name)
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
            .ok_or_else(|| StoreError::corrupt(format!("a stored event has no {name}")))
    };
    let writer = object
        .get("writer")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| StoreError::corrupt("a stored event has no writer"))?;
    let part = |name: &str| {
        writer
            .get(name)
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
            .ok_or_else(|| StoreError::corrupt(format!("a stored writer has no {name}")))
    };
    let writer = Writer {
        package: part("package")?,
        digest: part("digest")?,
    };
    Ok(EventRecord {
        id: field("id")?,
        version: object
            .get("version")
            .and_then(unsigned)
            .ok_or_else(|| StoreError::corrupt("a stored event has no version"))?,
        event_type: field("type")?,
        data: object
            .get("data")
            .and_then(JsonValue::as_object)
            .cloned()
            .ok_or_else(|| StoreError::corrupt("a stored event has no data"))?,
        digest: field("digest")?,
        idempotency_key: field("idempotency_key")?,
        committed_at: field("committed_at")?,
        writer,
        standing: object.get("standing") == Some(&JsonValue::Bool(true)),
        valid_until: object
            .get("valid_until")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        searchable: object
            .get("searchable")
            .and_then(JsonValue::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(JsonValue::as_str)
                    .map(str::to_owned)
                    .collect()
            }),
    })
}

/// An answer's common fields: its schema, operation, stream and backend.
fn answer(backend: &str, operation: &str, stream: &str) -> JsonObject {
    JsonObject::from([
        ("schema".to_owned(), text(RESULT_SCHEMA)),
        ("operation".to_owned(), text(operation)),
        ("stream".to_owned(), text(stream)),
        ("backend".to_owned(), text(backend)),
    ])
}

/// The answer, with the digest of everything it says.
fn sealed(mut answer: JsonObject) -> Result<JsonValue, StoreError> {
    let result_digest = digest(&JsonValue::Object(answer.clone()))?;
    answer.insert("result_digest".to_owned(), text(&result_digest));
    Ok(JsonValue::Object(answer))
}

/// The answer to an append. A replay answers as the first time did, with
/// the key's chain as it is now; a conflict appends nothing and names no
/// event.
pub(super) fn appended_result(
    backend: &str,
    at: Key<'_>,
    appended: &Appended,
) -> Result<JsonValue, StoreError> {
    let mut result = answer(backend, "events.append", at.stream);
    result.insert("key".to_owned(), text(at.key));
    let (status, before, version, event, chain_digest) = match appended {
        Appended::Committed {
            before,
            record,
            state,
        } => (
            "committed",
            *before,
            record.version,
            Some(record),
            &state.chain_digest,
        ),
        Appended::Replayed { current, existing } => (
            "replayed",
            existing.version.saturating_sub(1),
            existing.version,
            Some(existing),
            &current.chain_digest,
        ),
        Appended::IdempotencyConflict { current, .. } => (
            "idempotency_conflict",
            current.version,
            current.version,
            None,
            &current.chain_digest,
        ),
        Appended::VersionConflict { current } => (
            "version_conflict",
            current.version,
            current.version,
            None,
            &current.chain_digest,
        ),
    };
    result.insert("status".to_owned(), text(status));
    result.insert("before_version".to_owned(), number(before));
    result.insert("version".to_owned(), number(version));
    if let Some(event) = event {
        result.insert(
            "event".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("id".to_owned(), text(&event.id)),
                ("digest".to_owned(), text(&event.digest)),
            ])),
        );
    }
    result.insert("chain_digest".to_owned(), text(chain_digest));
    sealed(result)
}

/// The answer to a read of one page of a key's events. `last_version` is
/// the last version in the page, or the version read after when it's empty.
pub(super) fn events_result(
    backend: &str,
    at: Key<'_>,
    state: &KeyState,
    events: &[EventRecord],
    after_version: Option<u64>,
    has_more: bool,
) -> Result<JsonValue, StoreError> {
    let last_version = events.last().map_or_else(
        || after_version.unwrap_or(state.version),
        |event| event.version,
    );
    let mut result = answer(backend, "events.read", at.stream);
    result.insert("key".to_owned(), text(at.key));
    result.insert("version".to_owned(), number(state.version));
    result.insert(
        "events".to_owned(),
        JsonValue::Array(
            events
                .iter()
                .map(|record| JsonValue::Object(record_object(record)))
                .collect(),
        ),
    );
    result.insert("last_version".to_owned(), number(last_version));
    result.insert("has_more".to_owned(), JsonValue::Bool(has_more));
    result.insert("chain_digest".to_owned(), text(&state.chain_digest));
    sealed(result)
}

/// The answer to a key's inspection: its version, its newest event whole,
/// and its chain.
pub(super) fn inspected_result(
    backend: &str,
    at: Key<'_>,
    state: &KeyState,
) -> Result<JsonValue, StoreError> {
    let mut result = answer(backend, "key.inspect", at.stream);
    result.insert("key".to_owned(), text(at.key));
    result.insert("version".to_owned(), number(state.version));
    result.insert(
        "last_event".to_owned(),
        state.last.as_ref().map_or(JsonValue::Null, |last| {
            JsonValue::Object(record_object(last))
        }),
    );
    result.insert("chain_digest".to_owned(), text(&state.chain_digest));
    sealed(result)
}

/// The answer to one page of a stream's keys, newest commit first, or in key
/// order under a prefix.
pub(super) fn listed_result(
    backend: &str,
    stream: &str,
    keys: &[Listed],
    has_more: bool,
) -> Result<JsonValue, StoreError> {
    let next_cursor = match (has_more, keys.last()) {
        (true, Some(last)) => text(&encode_cursor(&ListCursor {
            committed_at: last.last.committed_at.clone(),
            key: last.key.clone(),
        })?),
        _ => JsonValue::Null,
    };
    let mut result = answer(backend, "keys.list", stream);
    result.insert(
        "keys".to_owned(),
        JsonValue::Array(
            keys.iter()
                .map(|listed| {
                    JsonValue::Object(JsonObject::from([
                        ("key".to_owned(), text(&listed.key)),
                        (
                            "last_event".to_owned(),
                            JsonValue::Object(record_object(&listed.last)),
                        ),
                    ]))
                })
                .collect(),
        ),
    );
    result.insert("next_cursor".to_owned(), next_cursor);
    sealed(result)
}

/// The answer to one page of a search, in score order, ties newest commit
/// first, then by key.
pub(super) fn matched_result(
    backend: &str,
    stream: &str,
    matches: &[Matched],
    has_more: bool,
) -> Result<JsonValue, StoreError> {
    let next_cursor = match (has_more, matches.last()) {
        (true, Some(last)) => text(&encode_search_cursor(&SearchCursor {
            score: last.score,
            committed_at: last.last.committed_at.clone(),
            key: last.key.clone(),
        })?),
        _ => JsonValue::Null,
    };
    let mut result = answer(backend, "keys.search", stream);
    result.insert(
        "matches".to_owned(),
        JsonValue::Array(
            matches
                .iter()
                .map(|matched| {
                    JsonValue::Object(JsonObject::from([
                        ("key".to_owned(), text(&matched.key)),
                        (
                            "score".to_owned(),
                            JsonValue::Number(JsonNumber::F64(matched.score)),
                        ),
                        (
                            "last_event".to_owned(),
                            JsonValue::Object(record_object(&matched.last)),
                        ),
                    ]))
                })
                .collect(),
        ),
    );
    result.insert("next_cursor".to_owned(), next_cursor);
    sealed(result)
}

/// Where a page of keys resumes: after this key in (newest commit, key)
/// order.
pub(super) struct ListCursor {
    pub committed_at: String,
    pub key: String,
}

/// Where a page of matches resumes: after this key in (score, newest
/// commit, key) order.
pub(super) struct SearchCursor {
    pub score: f64,
    pub committed_at: String,
    pub key: String,
}

fn decode(value: &str, what: &str) -> Result<JsonObject, StoreError> {
    let invalid = || StoreError::invalid(format!("cursor must be an opaque {what} cursor"));
    if value.len() > 1024
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

fn encode(cursor: &JsonObject) -> Result<String, StoreError> {
    let bytes = serde_json::to_vec(cursor)
        .map_err(|error| StoreError::invalid(format!("encoding a cursor: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn cursor_key(cursor: &JsonObject, what: &str) -> Result<String, StoreError> {
    let key = cursor
        .get("key")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| StoreError::invalid(format!("cursor must be an opaque {what} cursor")))?;
    validate_key(key)
        .map_err(|_| StoreError::invalid(format!("cursor must be an opaque {what} cursor")))?;
    Ok(key.to_owned())
}

pub(super) fn decode_cursor(value: Option<&str>) -> Result<Option<ListCursor>, StoreError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let cursor = decode(value, "key list")?;
    let committed_at = cursor
        .get("committed_at")
        .and_then(JsonValue::as_str)
        .filter(|value| runx_core::policy::parse_rfc3339_moment(value).is_some())
        .ok_or_else(|| StoreError::invalid("cursor must be an opaque key list cursor"))?;
    Ok(Some(ListCursor {
        committed_at: committed_at.to_owned(),
        key: cursor_key(&cursor, "key list")?,
    }))
}

fn encode_cursor(cursor: &ListCursor) -> Result<String, StoreError> {
    encode(&JsonObject::from([
        ("committed_at".to_owned(), text(&cursor.committed_at)),
        ("key".to_owned(), text(&cursor.key)),
    ]))
}

pub(super) fn decode_search_cursor(
    value: Option<&str>,
) -> Result<Option<SearchCursor>, StoreError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let cursor = decode(value, "search")?;
    let score = match cursor.get("score") {
        Some(JsonValue::Number(JsonNumber::F64(score))) if score.is_finite() => *score,
        Some(JsonValue::Number(JsonNumber::I64(score))) => *score as f64,
        Some(JsonValue::Number(JsonNumber::U64(score))) => *score as f64,
        _ => {
            return Err(StoreError::invalid(
                "cursor must be an opaque search cursor",
            ));
        }
    };
    let committed_at = cursor
        .get("committed_at")
        .and_then(JsonValue::as_str)
        .filter(|value| runx_core::policy::parse_rfc3339_moment(value).is_some())
        .ok_or_else(|| StoreError::invalid("cursor must be an opaque search cursor"))?;
    Ok(Some(SearchCursor {
        score,
        committed_at: committed_at.to_owned(),
        key: cursor_key(&cursor, "search")?,
    }))
}

fn encode_search_cursor(cursor: &SearchCursor) -> Result<String, StoreError> {
    encode(&JsonObject::from([
        (
            "score".to_owned(),
            JsonValue::Number(JsonNumber::F64(cursor.score)),
        ),
        ("committed_at".to_owned(), text(&cursor.committed_at)),
        ("key".to_owned(), text(&cursor.key)),
    ]))
}

/// The words a search on `private` matches an event by: every string under
/// the fields of its data it names searchable, their keys left out. None
/// when it names none, so its key is in no search.
pub(super) fn searchable_text(record: &EventRecord) -> Option<String> {
    fn collect(value: &JsonValue, words: &mut Vec<String>) {
        match value {
            JsonValue::String(text) => words.push(text.clone()),
            JsonValue::Array(items) => items.iter().for_each(|item| collect(item, words)),
            JsonValue::Object(object) => object.values().for_each(|item| collect(item, words)),
            _ => {}
        }
    }
    let fields = record.searchable.as_ref()?;
    let mut words = Vec::new();
    for field in fields {
        if let Some(value) = record.data.get(field) {
            collect(value, &mut words);
        }
    }
    Some(words.join("\n"))
}

/// A query as plain words: each run of letters and digits quoted, any of
/// them matching, so FTS5's own syntax is never read as syntax. None when
/// it holds no word.
pub(super) fn quoted_query(query: &str) -> Option<String> {
    let words = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{word}\""))
        .collect::<Vec<_>>();
    (!words.is_empty()).then(|| words.join(" OR "))
}
