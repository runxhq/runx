//! The Redis backend. A resource's keys share one hash tag, so a Redis
//! Cluster keeps them in one slot and an append is one atomic script.
//!
//! Redis has no SHA-256 for scripts, so the client computes the next
//! projection from the head it read, and the script applies the append only
//! while the stream is still at that head; otherwise the client reads again.

use redis::{Commands, IntoConnectionInfo};
use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};

use super::model::{self, Appended, EventRecord, Projection, StreamHead};
use super::request::{Append, ListHeads, ReadEvents, Source};
use super::{Backend, StoreError};

/// Attempts at an append while other appends keep moving its stream.
const APPEND_ATTEMPTS: usize = 8;
const HEAD_PAGE: isize = 100;

/// KEYS: events, keys, projection, heads, order, members.
/// ARGV: base version, expected version, idempotency key, record,
/// projection, head, order member, aggregate id.
const APPEND: &str = r#"
local current = redis.call('LLEN', KEYS[1])
if current ~= tonumber(ARGV[1]) then
  return {'moved', ''}
end
local existing = redis.call('HGET', KEYS[2], ARGV[3])
if existing then
  return {'existing', existing}
end
if current ~= tonumber(ARGV[2]) then
  return {'version', ''}
end
redis.call('RPUSH', KEYS[1], ARGV[4])
redis.call('HSET', KEYS[2], ARGV[3], ARGV[4])
redis.call('SET', KEYS[3], ARGV[5])
local previous = redis.call('HGET', KEYS[6], ARGV[8])
if previous then
  redis.call('ZREM', KEYS[5], previous)
end
redis.call('ZADD', KEYS[5], 0, ARGV[7])
redis.call('HSET', KEYS[6], ARGV[8], ARGV[7])
redis.call('HSET', KEYS[4], ARGV[8], ARGV[6])
return {'committed', ''}
"#;

pub(super) struct Redis {
    connection: redis::Connection,
}

/// One resource's keys, and one stream's within it.
struct Keys {
    events: String,
    keys: String,
    projection: String,
    heads: String,
    order: String,
    members: String,
}

impl Keys {
    fn of(source: Source<'_>) -> Result<Self, StoreError> {
        let resource = hex(&(source.data_source_ref, source.resource))?;
        let stream = hex(&(source.data_source_ref, source.resource, source.aggregate_id))?;
        let prefix = format!("runx:store:{{{resource}}}");
        Ok(Self {
            events: format!("{prefix}:events:{stream}"),
            keys: format!("{prefix}:keys:{stream}"),
            projection: format!("{prefix}:projection:{stream}"),
            heads: format!("{prefix}:heads"),
            order: format!("{prefix}:order"),
            members: format!("{prefix}:members"),
        })
    }
}

fn hex(value: &impl serde::Serialize) -> Result<String, StoreError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| StoreError::invalid(format!("encoding a key: {error}")))?;
    Ok(sha256_prefixed(&bytes)
        .trim_start_matches("sha256:")
        .to_owned())
}

/// A head's place in newest-first order: digits inverted so ascending text
/// order is newest first, then the aggregate.
fn order_member(committed_at: &str, aggregate_id: &str) -> String {
    let inverted = committed_at
        .chars()
        .map(|character| match character.to_digit(10) {
            Some(digit) => char::from_digit(9 - digit, 10).unwrap_or(character),
            None => character,
        })
        .collect::<String>();
    format!("{inverted}|{aggregate_id}")
}

impl Redis {
    pub(super) fn connect(url: &str, password: Option<&str>) -> Result<Self, StoreError> {
        let mut info = url
            .into_connection_info()
            .map_err(|error| StoreError::invalid(format!("the redis url: {error}")))?;
        if let Some(password) = password {
            let settings = info.redis_settings().clone().set_password(password);
            info = info.set_redis_settings(settings);
        }
        let connection = redis::Client::open(info)
            .and_then(|client| client.get_connection())
            .map_err(|error| StoreError::unavailable(format!("connecting to redis: {error}")))?;
        Ok(Self { connection })
    }

    fn get_json(&mut self, key: &str) -> Result<Option<JsonValue>, StoreError> {
        let value: Option<String> = self.connection.get(key).map_err(read_error)?;
        value
            .map(|text| {
                serde_json::from_str(&text).map_err(|error| {
                    StoreError::corrupt(format!("a stored value is not JSON: {error}"))
                })
            })
            .transpose()
    }
}

fn read_error(error: redis::RedisError) -> StoreError {
    StoreError::unavailable(format!("the redis store failed a read: {error}"))
}

fn projection_from(value: Option<JsonValue>) -> Result<Projection, StoreError> {
    let Some(value) = value else {
        return model::empty_projection();
    };
    let object = value
        .as_object()
        .ok_or_else(|| StoreError::corrupt("a stored projection is not an object"))?;
    let text = |field: &str| {
        object
            .get(field)
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
    };
    Ok(Projection {
        version: object
            .get("version")
            .and_then(model::unsigned)
            .ok_or_else(|| StoreError::corrupt("a stored projection has no version"))?,
        last_event_ref: text("last_event_ref"),
        last_event_type: text("last_event_type"),
        last_event_digest: text("last_event_digest"),
        projection_digest: text("projection_digest")
            .ok_or_else(|| StoreError::corrupt("a stored projection has no digest"))?,
    })
}

fn projection_json(projection: &Projection) -> Result<String, StoreError> {
    let optional = |value: &Option<String>| value.as_deref().map_or(JsonValue::Null, model::text);
    encoded(&JsonValue::Object(JsonObject::from([
        ("version".to_owned(), model::number(projection.version)),
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
        (
            "projection_digest".to_owned(),
            model::text(&projection.projection_digest),
        ),
    ])))
}

/// A value as the JSON text the store keeps.
fn encoded(value: &JsonValue) -> Result<String, StoreError> {
    serde_json::to_string(value)
        .map_err(|error| StoreError::invalid(format!("encoding a stored value: {error}")))
}

fn record_from_text(text: &str) -> Result<EventRecord, StoreError> {
    let value = serde_json::from_str::<JsonValue>(text)
        .map_err(|error| StoreError::corrupt(format!("a stored event is not JSON: {error}")))?;
    model::record_from_object(&value)
}

impl Backend for Redis {
    fn name(&self) -> &'static str {
        "redis"
    }

    fn append(&mut self, request: &Append<'_>, committed_at: &str) -> Result<Appended, StoreError> {
        let source = request.source;
        let keys = Keys::of(source)?;
        let script = redis::Script::new(APPEND);
        for _ in 0..APPEND_ATTEMPTS {
            let current = projection_from(self.get_json(&keys.projection)?)?;
            let next_version = current.version.checked_add(1).ok_or_else(|| {
                StoreError::invalid("the stream version exceeds the supported range")
            })?;
            let proposed = model::record(
                source,
                next_version,
                request.idempotency_key,
                request.event,
                committed_at,
            )?;
            let next = model::advance(&current, &proposed)?;
            let record = encoded(&JsonValue::Object(model::record_object(&proposed)))?;
            let mut head = model::record_object(&proposed);
            head.insert("aggregate_id".to_owned(), model::text(source.aggregate_id));
            let head = encoded(&JsonValue::Object(head))?;
            let projection = projection_json(&next)?;
            let (outcome, existing): (String, String) = script
                .key(&keys.events)
                .key(&keys.keys)
                .key(&keys.projection)
                .key(&keys.heads)
                .key(&keys.order)
                .key(&keys.members)
                .arg(current.version)
                .arg(request.expected_version)
                .arg(request.idempotency_key)
                .arg(&record)
                .arg(&projection)
                .arg(&head)
                .arg(order_member(&proposed.committed_at, source.aggregate_id))
                .arg(source.aggregate_id)
                .invoke(&mut self.connection)
                .map_err(|error| {
                    // The script may have run before the answer was lost.
                    StoreError::uncertain(format!("redis did not confirm the append: {error}"))
                })?;
            return match outcome.as_str() {
                "moved" => continue,
                "existing" => {
                    let existing = record_from_text(&existing)?;
                    Ok(if existing.event_digest == proposed.event_digest {
                        Appended::Replay { current, existing }
                    } else {
                        Appended::KeyConflict {
                            current,
                            proposed,
                            held: existing.event_digest,
                        }
                    })
                }
                "version" => Ok(Appended::VersionConflict {
                    current,
                    proposed,
                    expected: request.expected_version,
                }),
                "committed" => Ok(Appended::Committed {
                    before: current.version,
                    record: proposed,
                    projection: next,
                }),
                other => Err(StoreError::corrupt(format!(
                    "the append script answered {other}"
                ))),
            };
        }
        Err(StoreError::unavailable(
            "the stream kept moving; nothing was appended",
        ))
    }

    fn projection(&mut self, source: Source<'_>) -> Result<Projection, StoreError> {
        let keys = Keys::of(source)?;
        projection_from(self.get_json(&keys.projection)?)
    }

    fn events(&mut self, request: &ReadEvents<'_>) -> Result<(Vec<EventRecord>, bool), StoreError> {
        let keys = Keys::of(request.source)?;
        let limit = isize::try_from(request.limit)
            .map_err(|_| StoreError::invalid("the limit exceeds the store's range"))?;
        let (start, stop) = match request.after_version {
            Some(after) => {
                let after = isize::try_from(after)
                    .map_err(|_| StoreError::invalid("after_version exceeds the store's range"))?;
                (after, after + limit)
            }
            None => (-limit, -1),
        };
        let texts: Vec<String> = self
            .connection
            .lrange(&keys.events, start, stop)
            .map_err(read_error)?;
        let mut events = texts
            .iter()
            .map(|text| record_from_text(text))
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = request.after_version.is_some() && events.len() > request.limit;
        events.truncate(request.limit);
        Ok((events, has_more))
    }

    fn heads(&mut self, request: &ListHeads<'_>) -> Result<(Vec<StreamHead>, bool), StoreError> {
        let keys = Keys::of(Source {
            data_source_ref: request.data_source_ref,
            resource: request.resource,
            aggregate_id: "stream-heads",
        })?;
        let mut start = match model::decode_cursor(request.cursor)? {
            Some(cursor) => format!(
                "({}",
                order_member(&cursor.committed_at, &cursor.aggregate_id)
            ),
            None => "-".to_owned(),
        };
        let mut heads = Vec::new();
        loop {
            let members: Vec<String> = redis::cmd("ZRANGEBYLEX")
                .arg(&keys.order)
                .arg(&start)
                .arg("+")
                .arg("LIMIT")
                .arg(0)
                .arg(HEAD_PAGE)
                .query(&mut self.connection)
                .map_err(read_error)?;
            let Some(last) = members.last() else {
                break;
            };
            start = format!("({last}");
            let aggregates = members
                .iter()
                .map(|member| {
                    member
                        .split_once('|')
                        .map(|(_, aggregate)| aggregate.to_owned())
                        .ok_or_else(|| StoreError::corrupt("a head order member has no aggregate"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let records: Vec<Option<String>> = redis::cmd("HMGET")
                .arg(&keys.heads)
                .arg(&aggregates)
                .query(&mut self.connection)
                .map_err(read_error)?;
            for (aggregate_id, text) in aggregates.into_iter().zip(records) {
                let text =
                    text.ok_or_else(|| StoreError::corrupt("an ordered head has no record"))?;
                let event = record_from_text(&text)?;
                if request.event_types.is_empty()
                    || request.event_types.contains(&event.event_type.as_str())
                {
                    heads.push(StreamHead {
                        aggregate_id,
                        event,
                    });
                    if heads.len() > request.limit {
                        heads.truncate(request.limit);
                        return Ok((heads, true));
                    }
                }
            }
            if members.len() < HEAD_PAGE as usize {
                break;
            }
        }
        Ok((heads, false))
    }

    fn landed(
        &mut self,
        source: Source<'_>,
        idempotency_key: &str,
    ) -> Result<Option<String>, StoreError> {
        let keys = Keys::of(source)?;
        let text: Option<String> = self
            .connection
            .hget(&keys.keys, idempotency_key)
            .map_err(read_error)?;
        Ok(text
            .map(|text| record_from_text(&text))
            .transpose()?
            .map(|record| record.event_digest))
    }
}
