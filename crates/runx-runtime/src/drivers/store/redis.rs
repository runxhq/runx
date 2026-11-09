//! The Redis backend. A stream's keys share one hash tag, so a Redis
//! Cluster keeps them in one slot and an append is one atomic script. It
//! keeps `store`'s streams; a `private` instance is refused here when it's
//! configured, because this backend can't search.
//!
//! Redis has no SHA-256 for scripts, so the client computes the key's next
//! state from the one it read, and the script applies the append only while
//! the key is still there; otherwise the client reads again.

use redis::{Commands, IntoConnectionInfo};
use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};

use super::model::{self, Appended, EventRecord, KeyState, Listed};
use super::request::{Append, Key, ListKeys, ReadEvents};
use super::{Backend, StoreError};

/// Attempts at an append while other appends keep moving its key.
const APPEND_ATTEMPTS: usize = 8;
const KEY_PAGE: isize = 100;
/// Where the store keeps its schema version.
const SCHEMA_KEY: &str = "runx:store:schema";
/// 0.12 raised it from 2, so an older runx refuses the store when it opens it,
/// and 0.12 never reads a store from before it.
const SCHEMA_VERSION: &str = "3";
/// Where a store from before 0.12 kept its streams. Version 1 set no schema
/// version, so a store with none that holds a key here is one.
const BEFORE_0_12: &str = "runx:store:{*";

/// KEYS: events, idempotency, state, keys, order, members.
/// ARGV: base version, expected version, idempotency key, record, state,
/// newest event, order member, key.
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

/// One stream's Redis keys, and one key's within it.
struct Keys {
    events: String,
    idempotency: String,
    state: String,
    keys: String,
    order: String,
    members: String,
}

impl Keys {
    fn of(stream: &str, key: &str) -> Result<Self, StoreError> {
        let prefix = format!("runx:streams:{{{}}}", hex(&stream)?);
        let key = hex(&key)?;
        Ok(Self {
            events: format!("{prefix}:events:{key}"),
            idempotency: format!("{prefix}:idempotency:{key}"),
            state: format!("{prefix}:state:{key}"),
            keys: format!("{prefix}:keys"),
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

/// A key's place in newest-first order: digits inverted so ascending text
/// order is newest first, then the key.
fn order_member(committed_at: &str, key: &str) -> String {
    let inverted = committed_at
        .chars()
        .map(|character| match character.to_digit(10) {
            Some(digit) => char::from_digit(9 - digit, 10).unwrap_or(character),
            None => character,
        })
        .collect::<String>();
    format!("{inverted}|{key}")
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
        let mut store = Self { connection };
        store.ensure_schema()?;
        Ok(store)
    }

    /// The store at this schema version: a new one is marked with it, and a
    /// store from before 0.12, at version 2 or at none with its keys, is
    /// never read.
    fn ensure_schema(&mut self) -> Result<(), StoreError> {
        let version: Option<String> = self.connection.get(SCHEMA_KEY).map_err(read_error)?;
        let before = || {
            StoreError::invalid(
                "the redis database holds a store from before runx 0.12, which 0.12 doesn't read; move it aside, or name another database, to start a new store",
            )
        };
        match version.as_deref() {
            Some(SCHEMA_VERSION) => Ok(()),
            Some("2") => Err(before()),
            Some(other) => Err(StoreError::corrupt(format!(
                "the store has schema version {other}, not {SCHEMA_VERSION}"
            ))),
            None => {
                let mut earlier = self
                    .connection
                    .scan_match::<_, String>(BEFORE_0_12)
                    .map_err(read_error)?;
                if earlier.next().is_some() {
                    return Err(before());
                }
                drop(earlier);
                self.connection
                    .set::<_, _, ()>(SCHEMA_KEY, SCHEMA_VERSION)
                    .map_err(write_error)
            }
        }
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

fn write_error(error: redis::RedisError) -> StoreError {
    StoreError::unavailable(format!("the redis store failed a write: {error}"))
}

fn state_from(value: Option<JsonValue>) -> Result<KeyState, StoreError> {
    let Some(value) = value else {
        return model::empty_state();
    };
    let object = value
        .as_object()
        .ok_or_else(|| StoreError::corrupt("a stored key's state is not an object"))?;
    Ok(KeyState {
        version: object
            .get("version")
            .and_then(model::unsigned)
            .ok_or_else(|| StoreError::corrupt("a stored key has no version"))?,
        last: object
            .get("last_event")
            .map(model::record_from_object)
            .transpose()?,
        chain_digest: object
            .get("chain_digest")
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
            .ok_or_else(|| StoreError::corrupt("a stored key has no chain"))?,
    })
}

fn state_json(state: &KeyState) -> Result<String, StoreError> {
    let mut object = JsonObject::from([
        ("version".to_owned(), model::number(state.version)),
        ("chain_digest".to_owned(), model::text(&state.chain_digest)),
    ]);
    if let Some(last) = &state.last {
        object.insert(
            "last_event".to_owned(),
            JsonValue::Object(model::record_object(last)),
        );
    }
    encoded(&JsonValue::Object(object))
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
        let at = request.at;
        let keys = Keys::of(at.stream, at.key)?;
        let script = redis::Script::new(APPEND);
        for _ in 0..APPEND_ATTEMPTS {
            let current = state_from(self.get_json(&keys.state)?)?;
            let next_version = current.version.checked_add(1).ok_or_else(|| {
                StoreError::invalid("the key's version exceeds the supported range")
            })?;
            let proposed = super::proposed(request, next_version, committed_at)?;
            let next = model::advance(&current, &proposed)?;
            let record = encoded(&JsonValue::Object(model::record_object(&proposed)))?;
            let (outcome, existing): (String, String) = script
                .key(&keys.events)
                .key(&keys.idempotency)
                .key(&keys.state)
                .key(&keys.keys)
                .key(&keys.order)
                .key(&keys.members)
                .arg(current.version)
                .arg(request.expected_version)
                .arg(request.idempotency_key)
                .arg(&record)
                .arg(state_json(&next)?)
                .arg(&record)
                .arg(order_member(&proposed.committed_at, at.key))
                .arg(at.key)
                .invoke(&mut self.connection)
                .map_err(|error| {
                    // The script may have run before the answer was lost.
                    StoreError::uncertain(format!("redis did not confirm the append: {error}"))
                })?;
            return match outcome.as_str() {
                "moved" => continue,
                "existing" => {
                    let existing = record_from_text(&existing)?;
                    Ok(if existing.digest == proposed.digest {
                        Appended::Replayed { current, existing }
                    } else {
                        Appended::IdempotencyConflict {
                            current,
                            held: existing.digest,
                        }
                    })
                }
                "version" => Ok(Appended::VersionConflict { current }),
                "committed" => Ok(Appended::Committed {
                    before: current.version,
                    record: proposed,
                    state: next,
                }),
                other => Err(StoreError::corrupt(format!(
                    "the append script answered {other}"
                ))),
            };
        }
        Err(StoreError::unavailable(
            "the key kept moving; nothing was appended",
        ))
    }

    fn state(&mut self, at: Key<'_>) -> Result<KeyState, StoreError> {
        let keys = Keys::of(at.stream, at.key)?;
        state_from(self.get_json(&keys.state)?)
    }

    fn events(&mut self, request: &ReadEvents<'_>) -> Result<(Vec<EventRecord>, bool), StoreError> {
        let keys = Keys::of(request.at.stream, request.at.key)?;
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

    fn keys(&mut self, request: &ListKeys<'_>) -> Result<(Vec<Listed>, bool), StoreError> {
        let keys = Keys::of(request.stream, "")?;
        let mut start = match model::decode_cursor(request.cursor)? {
            Some(cursor) => format!("({}", order_member(&cursor.committed_at, &cursor.key)),
            None => "-".to_owned(),
        };
        let mut listed = Vec::new();
        loop {
            let members: Vec<String> = redis::cmd("ZRANGEBYLEX")
                .arg(&keys.order)
                .arg(&start)
                .arg("+")
                .arg("LIMIT")
                .arg(0)
                .arg(KEY_PAGE)
                .query(&mut self.connection)
                .map_err(read_error)?;
            let Some(last) = members.last() else {
                break;
            };
            start = format!("({last}");
            let names = members
                .iter()
                .map(|member| {
                    member
                        .split_once('|')
                        .map(|(_, key)| key.to_owned())
                        .ok_or_else(|| StoreError::corrupt("an order member names no key"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let records: Vec<Option<String>> = redis::cmd("HMGET")
                .arg(&keys.keys)
                .arg(&names)
                .query(&mut self.connection)
                .map_err(read_error)?;
            for (key, text) in names.into_iter().zip(records) {
                let text =
                    text.ok_or_else(|| StoreError::corrupt("an ordered key has no event"))?;
                let last = record_from_text(&text)?;
                if request.types.is_empty() || request.types.contains(&last.event_type.as_str()) {
                    listed.push(Listed { key, last });
                    if listed.len() > request.limit {
                        listed.truncate(request.limit);
                        return Ok((listed, true));
                    }
                }
            }
            if members.len() < KEY_PAGE as usize {
                break;
            }
        }
        Ok((listed, false))
    }

    fn landed(&mut self, at: Key<'_>, idempotency_key: &str) -> Result<Option<String>, StoreError> {
        let keys = Keys::of(at.stream, at.key)?;
        let text: Option<String> = self
            .connection
            .hget(&keys.idempotency, idempotency_key)
            .map_err(read_error)?;
        Ok(text
            .map(|text| record_from_text(&text))
            .transpose()?
            .map(|record| record.digest))
    }
}
