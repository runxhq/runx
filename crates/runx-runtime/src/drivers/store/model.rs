//! Events, projections and results, the same for every backend. A record's
//! digest covers its event; a projection's digest chains the stream's events.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use runx_contracts::{JsonNumber, JsonObject, JsonValue, sha256_prefixed};

use super::StoreError;
use super::request::{Source, validate_aggregate_id, validate_event_type};

#[derive(Clone, Debug)]
pub(super) struct EventRecord {
    pub event_ref: String,
    pub version: u64,
    pub event_type: String,
    pub event: JsonObject,
    pub event_digest: String,
    pub idempotency_key: String,
    pub committed_at: String,
}

#[derive(Clone, Debug)]
pub(super) struct Projection {
    pub version: u64,
    pub last_event_ref: Option<String>,
    pub last_event_type: Option<String>,
    pub last_event_digest: Option<String>,
    pub projection_digest: String,
}

#[derive(Clone, Debug)]
pub(super) struct StreamHead {
    pub aggregate_id: String,
    pub event: EventRecord,
}

/// What an append did.
pub(super) enum Appended {
    Committed {
        before: u64,
        record: EventRecord,
        projection: Projection,
    },
    /// The key already holds this exact event.
    Replay {
        current: Projection,
        existing: EventRecord,
    },
    /// The key holds a different event, whose digest is `held`.
    KeyConflict {
        current: Projection,
        proposed: EventRecord,
        held: String,
    },
    /// The stream is not at the expected version.
    VersionConflict {
        current: Projection,
        proposed: EventRecord,
        expected: u64,
    },
}

impl Appended {
    /// The digest of the event the store holds under the call's key, which
    /// a readback must find there: none after a version conflict.
    pub(super) fn landed(&self) -> Option<&str> {
        match self {
            Self::Committed { record, .. } => Some(&record.event_digest),
            Self::Replay { existing, .. } => Some(&existing.event_digest),
            Self::KeyConflict { held, .. } => Some(held),
            Self::VersionConflict { .. } => None,
        }
    }
}

pub(super) fn record(
    source: Source<'_>,
    version: u64,
    idempotency_key: &str,
    event: &JsonObject,
    committed_at: &str,
) -> Result<EventRecord, StoreError> {
    let event_type = event_type(event);
    validate_event_type(&event_type)?;
    Ok(EventRecord {
        event_ref: format!("{}:{}:{version}", source.resource, source.aggregate_id),
        version,
        event_type,
        event: event.clone(),
        event_digest: digest(&JsonValue::Object(event.clone()))?,
        idempotency_key: idempotency_key.to_owned(),
        committed_at: normalize_time(committed_at)?,
    })
}

pub(super) fn empty_projection() -> Result<Projection, StoreError> {
    Ok(Projection {
        version: 0,
        last_event_ref: None,
        last_event_type: None,
        last_event_digest: None,
        projection_digest: digest(&JsonValue::Object(JsonObject::from([
            ("version".to_owned(), number(0)),
            ("event_digest".to_owned(), JsonValue::Null),
        ])))?,
    })
}

pub(super) fn advance(current: &Projection, event: &EventRecord) -> Result<Projection, StoreError> {
    Ok(Projection {
        version: event.version,
        last_event_ref: Some(event.event_ref.clone()),
        last_event_type: Some(event.event_type.clone()),
        last_event_digest: Some(event.event_digest.clone()),
        projection_digest: digest(&JsonValue::Object(JsonObject::from([
            ("version".to_owned(), number(event.version)),
            (
                "previous_projection_digest".to_owned(),
                text(&current.projection_digest),
            ),
            ("event_digest".to_owned(), text(&event.event_digest)),
        ])))?,
    })
}

/// The event type an event declares: `type`, `event_type`, the effect family
/// and operation, or the operation, else `data.event`.
fn event_type(event: &JsonObject) -> String {
    let valid = |value: &str| validate_event_type(value).is_ok();
    for field in ["type", "event_type"] {
        if let Some(value) = event.get(field).and_then(JsonValue::as_str)
            && valid(value)
        {
            return value.to_owned();
        }
    }
    let family = event.get("effect_family").and_then(JsonValue::as_str);
    let operation = event.get("operation").and_then(JsonValue::as_str);
    match (family, operation) {
        (Some(family), Some(operation)) if valid(family) && valid(operation) => {
            format!("{family}.{operation}")
        }
        (_, Some(operation)) if valid(operation) => operation.to_owned(),
        _ => "data.event".to_owned(),
    }
}

/// An RFC 3339 time as UTC with milliseconds, so times order as text.
pub(super) fn normalize_time(value: &str) -> Result<String, StoreError> {
    let (days, seconds, nanos) = runx_core::policy::parse_rfc3339_moment(value)
        .ok_or_else(|| StoreError::invalid("observed_at must be RFC 3339"))?;
    let unix_seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(seconds))
        .ok_or_else(|| StoreError::invalid("observed_at is outside the supported range"))?;
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

pub(super) fn record_object(record: &EventRecord) -> JsonObject {
    JsonObject::from([
        ("event_ref".to_owned(), text(&record.event_ref)),
        ("version".to_owned(), number(record.version)),
        ("event_type".to_owned(), text(&record.event_type)),
        ("event".to_owned(), JsonValue::Object(record.event.clone())),
        ("event_digest".to_owned(), text(&record.event_digest)),
        ("idempotency_key".to_owned(), text(&record.idempotency_key)),
        ("committed_at".to_owned(), text(&record.committed_at)),
    ])
}

/// A stored record back from its object form.
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
    Ok(EventRecord {
        event_ref: field("event_ref")?,
        version: object
            .get("version")
            .and_then(unsigned)
            .ok_or_else(|| StoreError::corrupt("a stored event has no version"))?,
        event_type: field("event_type")?,
        event: object
            .get("event")
            .and_then(JsonValue::as_object)
            .cloned()
            .ok_or_else(|| StoreError::corrupt("a stored event has no event"))?,
        event_digest: field("event_digest")?,
        idempotency_key: field("idempotency_key")?,
        committed_at: field("committed_at")?,
    })
}

fn projection_object(projection: &Projection, source: Source<'_>) -> JsonObject {
    let optional = |value: &Option<String>| value.as_deref().map_or(JsonValue::Null, text);
    JsonObject::from([
        ("aggregate_id".to_owned(), text(source.aggregate_id)),
        ("resource".to_owned(), text(source.resource)),
        ("version".to_owned(), number(projection.version)),
        ("event_count".to_owned(), number(projection.version)),
        (
            "last_event_ref".to_owned(),
            optional(&projection.last_event_ref),
        ),
        (
            "last_event_type".to_owned(),
            optional(&projection.last_event_type),
        ),
        (
            "last_event_digest".to_owned(),
            optional(&projection.last_event_digest),
        ),
    ])
}

/// The result of an append, and the digest the store holds under its key.
pub(super) fn appended_result(
    backend: &str,
    source: Source<'_>,
    appended: &Appended,
) -> Result<JsonValue, StoreError> {
    let mut result = envelope(backend, source, "events.append")?;
    match appended {
        Appended::Committed {
            before,
            record,
            projection,
        } => effect(
            &mut result,
            "committed",
            *before,
            record,
            &projection.projection_digest,
        )?,
        Appended::Replay { current, existing } => effect(
            &mut result,
            "idempotent_replay",
            current.version,
            existing,
            &current.projection_digest,
        )?,
        Appended::KeyConflict {
            current, proposed, ..
        } => conflict(
            &mut result,
            current,
            proposed,
            "the idempotency key was used for a different event",
        )?,
        Appended::VersionConflict {
            current,
            proposed,
            expected,
        } => conflict(
            &mut result,
            current,
            proposed,
            &format!("expected version {expected}, got {}", current.version),
        )?,
    }
    Ok(JsonValue::Object(result))
}

fn effect(
    result: &mut JsonObject,
    status: &str,
    before: u64,
    record: &EventRecord,
    projection_digest: &str,
) -> Result<(), StoreError> {
    let after = if status == "committed" {
        record.version
    } else {
        before
    };
    versions(result, before, after);
    result.insert("status".to_owned(), text(status));
    result.insert("idempotency_key".to_owned(), text(&record.idempotency_key));
    result.insert("event_ref".to_owned(), text(&record.event_ref));
    result.insert("event_digest".to_owned(), text(&record.event_digest));
    result.insert(
        "result_digest".to_owned(),
        text(&digest(&JsonValue::Object(record_object(record)))?),
    );
    result.insert("projection_digest".to_owned(), text(projection_digest));
    result.insert("projection".to_owned(), JsonValue::Null);
    empty(result, Vec::new());
    Ok(())
}

fn conflict(
    result: &mut JsonObject,
    current: &Projection,
    proposed: &EventRecord,
    reason: &str,
) -> Result<(), StoreError> {
    let stop = JsonValue::Object(JsonObject::from([
        ("code".to_owned(), text("conflict")),
        ("message".to_owned(), text(reason)),
    ]));
    versions(result, current.version, current.version);
    result.insert("status".to_owned(), text("conflict"));
    result.insert(
        "idempotency_key".to_owned(),
        text(&proposed.idempotency_key),
    );
    result.insert("event_ref".to_owned(), JsonValue::Null);
    result.insert("event_digest".to_owned(), text(&proposed.event_digest));
    result.insert("result_digest".to_owned(), text(&digest(&stop)?));
    result.insert(
        "projection_digest".to_owned(),
        text(&current.projection_digest),
    );
    result.insert("projection".to_owned(), JsonValue::Null);
    empty(result, vec![stop]);
    Ok(())
}

pub(super) fn events_result(
    backend: &str,
    source: Source<'_>,
    projection: &Projection,
    events: &[EventRecord],
    limit: usize,
    after_version: Option<u64>,
    has_more: bool,
) -> Result<JsonValue, StoreError> {
    let rows = events
        .iter()
        .map(|record| JsonValue::Object(record_object(record)))
        .collect::<Vec<_>>();
    let next_after_version = events.last().map_or_else(
        || after_version.unwrap_or(projection.version),
        |event| event.version,
    );
    let page = JsonObject::from([
        ("events".to_owned(), JsonValue::Array(rows.clone())),
        ("limit".to_owned(), number(limit as u64)),
        ("next_after_version".to_owned(), number(next_after_version)),
        ("has_more".to_owned(), JsonValue::Bool(has_more)),
    ]);
    let mut result = envelope(backend, source, "events.read")?;
    versions(&mut result, projection.version, projection.version);
    read(&mut result);
    result.insert(
        "result_digest".to_owned(),
        text(&digest(&JsonValue::Object(page))?),
    );
    result.insert(
        "projection_digest".to_owned(),
        text(&projection.projection_digest),
    );
    result.insert("projection".to_owned(), JsonValue::Null);
    result.insert("limit".to_owned(), number(limit as u64));
    result.insert("next_after_version".to_owned(), number(next_after_version));
    result.insert("has_more".to_owned(), JsonValue::Bool(has_more));
    result.insert("events".to_owned(), JsonValue::Array(rows.clone()));
    result.insert("rows".to_owned(), JsonValue::Array(rows));
    result.insert("redactions".to_owned(), JsonValue::Array(Vec::new()));
    result.insert("stop_conditions".to_owned(), JsonValue::Array(Vec::new()));
    Ok(JsonValue::Object(result))
}

pub(super) fn projection_result(
    backend: &str,
    source: Source<'_>,
    projection: &Projection,
) -> Result<JsonValue, StoreError> {
    let object = projection_object(projection, source);
    let mut result = envelope(backend, source, "projection.read")?;
    versions(&mut result, projection.version, projection.version);
    read(&mut result);
    result.insert(
        "result_digest".to_owned(),
        text(&digest(&JsonValue::Object(object.clone()))?),
    );
    result.insert(
        "projection_digest".to_owned(),
        text(&projection.projection_digest),
    );
    result.insert("projection".to_owned(), JsonValue::Object(object));
    empty(&mut result, Vec::new());
    Ok(JsonValue::Object(result))
}

pub(super) fn heads_result(
    backend: &str,
    data_source_ref: &str,
    resource: &str,
    limit: usize,
    heads: &[StreamHead],
    has_more: bool,
) -> Result<JsonValue, StoreError> {
    let rows = heads
        .iter()
        .map(|head| {
            let mut object = record_object(&head.event);
            object.insert("aggregate_id".to_owned(), text(&head.aggregate_id));
            JsonValue::Object(object)
        })
        .collect::<Vec<_>>();
    let next_cursor = match (has_more, heads.last()) {
        (true, Some(last)) => text(&encode_cursor(last)?),
        _ => JsonValue::Null,
    };
    let page = JsonObject::from([
        ("limit".to_owned(), number(limit as u64)),
        ("count".to_owned(), number(rows.len() as u64)),
        ("has_more".to_owned(), JsonValue::Bool(has_more)),
        ("next_cursor".to_owned(), next_cursor),
    ]);
    let identities = rows
        .iter()
        .filter_map(JsonValue::as_object)
        .map(|row| {
            JsonValue::Array(
                ["aggregate_id", "version", "event_digest"]
                    .iter()
                    .map(|field| row.get(*field).cloned().unwrap_or(JsonValue::Null))
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    let source = Source {
        data_source_ref,
        resource,
        aggregate_id: "stream-heads",
    };
    let mut result = envelope(backend, source, "heads.list")?;
    versions(&mut result, 0, 0);
    read(&mut result);
    result.insert(
        "result_digest".to_owned(),
        text(&digest(&JsonValue::Object(JsonObject::from([
            ("rows".to_owned(), JsonValue::Array(rows.clone())),
            ("page".to_owned(), JsonValue::Object(page.clone())),
        ])))?),
    );
    result.insert(
        "projection_digest".to_owned(),
        text(&digest(&JsonValue::Array(identities))?),
    );
    result.insert("projection".to_owned(), JsonValue::Object(page));
    result.insert("events".to_owned(), JsonValue::Array(Vec::new()));
    result.insert("rows".to_owned(), JsonValue::Array(rows));
    result.insert("redactions".to_owned(), JsonValue::Array(Vec::new()));
    result.insert("stop_conditions".to_owned(), JsonValue::Array(Vec::new()));
    Ok(JsonValue::Object(result))
}

fn envelope(backend: &str, source: Source<'_>, operation: &str) -> Result<JsonObject, StoreError> {
    Ok(JsonObject::from([
        ("schema".to_owned(), text("runx.data.operation_result.v1")),
        ("data_source_ref".to_owned(), text(source.data_source_ref)),
        ("provider".to_owned(), text("store")),
        ("operation".to_owned(), text(operation)),
        ("resource".to_owned(), text(source.resource)),
        ("aggregate_id".to_owned(), text(source.aggregate_id)),
        (
            "provider_evidence".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("backend".to_owned(), text(backend)),
                (
                    "data_source_ref_digest".to_owned(),
                    text(&digest(&text(source.data_source_ref))?),
                ),
                ("resource".to_owned(), text(source.resource)),
                ("aggregate_id".to_owned(), text(source.aggregate_id)),
            ])),
        ),
    ]))
}

fn versions(result: &mut JsonObject, before: u64, after: u64) {
    result.insert("before_version".to_owned(), number(before));
    result.insert("after_version".to_owned(), number(after));
}

fn read(result: &mut JsonObject) {
    result.insert("status".to_owned(), text("read"));
    result.insert("idempotency_key".to_owned(), JsonValue::Null);
    result.insert("event_ref".to_owned(), JsonValue::Null);
    result.insert("event_digest".to_owned(), JsonValue::Null);
}

fn empty(result: &mut JsonObject, stop_conditions: Vec<JsonValue>) {
    result.insert("events".to_owned(), JsonValue::Array(Vec::new()));
    result.insert("rows".to_owned(), JsonValue::Array(Vec::new()));
    result.insert("redactions".to_owned(), JsonValue::Array(Vec::new()));
    result.insert(
        "stop_conditions".to_owned(),
        JsonValue::Array(stop_conditions),
    );
}

/// Where a stream-head page resumes: after this head in (newest commit,
/// aggregate) order.
pub(super) struct HeadCursor {
    pub committed_at: String,
    pub aggregate_id: String,
}

pub(super) fn decode_cursor(value: Option<&str>) -> Result<Option<HeadCursor>, StoreError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let invalid = || StoreError::invalid("cursor must be an opaque stream-head cursor");
    if value.len() > 1024
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| invalid())?;
    let cursor: JsonObject = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let committed_at = cursor
        .get("committed_at")
        .and_then(JsonValue::as_str)
        .filter(|value| runx_core::policy::parse_rfc3339_moment(value).is_some())
        .ok_or_else(invalid)?;
    let aggregate_id = cursor
        .get("aggregate_id")
        .and_then(JsonValue::as_str)
        .ok_or_else(invalid)?;
    validate_aggregate_id(aggregate_id).map_err(|_| invalid())?;
    Ok(Some(HeadCursor {
        committed_at: committed_at.to_owned(),
        aggregate_id: aggregate_id.to_owned(),
    }))
}

fn encode_cursor(head: &StreamHead) -> Result<String, StoreError> {
    let cursor = JsonObject::from([
        ("committed_at".to_owned(), text(&head.event.committed_at)),
        ("aggregate_id".to_owned(), text(&head.aggregate_id)),
    ]);
    let bytes = serde_json::to_vec(&cursor)
        .map_err(|error| StoreError::invalid(format!("encoding a cursor: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
