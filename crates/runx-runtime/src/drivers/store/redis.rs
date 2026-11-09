//! The Redis backend. A stream's keys share one hash tag, so a Redis
//! Cluster keeps them in one slot and an append is one atomic script. It
//! keeps `store`'s streams; a `private` instance is refused here when it's
//! configured, because this backend can't search.
//!
//! Redis has no SHA-256 for scripts, so the client computes the key's next
//! state from the one it read, and the script applies the append only while
//! the key is still there; otherwise the client reads again.

use std::collections::BTreeMap;

use redis::{Commands, IntoConnectionInfo};
use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};

use super::migration::{self, OldAddress, OldEvent};
use super::model::{self, Appended, EventRecord, KeyState, Listed};
use super::request::{Append, Key, ListKeys, ReadEvents};
use super::{Backend, StoreError};

/// Attempts at an append while other appends keep moving its key.
const APPEND_ATTEMPTS: usize = 8;
const KEY_PAGE: isize = 100;
/// Where the store keeps its schema version.
const SCHEMA_KEY: &str = "runx:store:schema";
/// 0.12 raised it from 2, so an older runx refuses the store when it opens it.
const SCHEMA_VERSION: &str = "3";
/// Held while one process migrates the store, so two never do at once.
const MIGRATION_LOCK: &str = "runx:store:schema:migrating";
const MIGRATION_LOCK_SECONDS: u64 = 600;
/// Where version 2 and earlier kept their streams, by data source and
/// resource.
const VERSION_2_PREFIX: &str = "runx:store:{";

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

    /// The store at this schema version. A store with no version is new or
    /// version 1, which named a record's `event_id` `event_ref` and a
    /// projection's `last_event_id` `last_event_ref`; a store at version 1
    /// or 2 moves to this version's streams once, under a lock, before the
    /// version is set.
    fn ensure_schema(&mut self) -> Result<(), StoreError> {
        let version: Option<String> = self.connection.get(SCHEMA_KEY).map_err(read_error)?;
        let from_1 = match version.as_deref() {
            Some(SCHEMA_VERSION) => return Ok(()),
            Some("2") => false,
            None => true,
            Some(other) => {
                return Err(StoreError::corrupt(format!(
                    "the store has schema version {other}, not {SCHEMA_VERSION}"
                )));
            }
        };
        let locked: Option<String> = redis::cmd("SET")
            .arg(MIGRATION_LOCK)
            .arg("1")
            .arg("NX")
            .arg("EX")
            .arg(MIGRATION_LOCK_SECONDS)
            .query(&mut self.connection)
            .map_err(write_error)?;
        if locked.is_none() {
            return Err(StoreError::unavailable(
                "another runx is migrating this store; try again once it finishes",
            ));
        }
        let migrated = self.migrate(from_1).and_then(|()| {
            self.connection
                .set::<_, _, ()>(SCHEMA_KEY, SCHEMA_VERSION)
                .map_err(write_error)
        });
        let released: Result<(), redis::RedisError> = self.connection.del(MIGRATION_LOCK);
        migrated?;
        released.map_err(write_error)
    }

    /// Move every stream an earlier version kept to the stream and key it
    /// now belongs to (§6.9). Version 2 hashed each resource's data source
    /// into its keys, so a resource is recovered from the heads it kept and
    /// its data source stands as the group of keys that held it.
    fn migrate(&mut self, from_1: bool) -> Result<(), StoreError> {
        let mut groups = BTreeMap::<String, Vec<(String, String)>>::new();
        let keys = self
            .connection
            .scan_match::<_, String>(format!("{VERSION_2_PREFIX}*"))
            .map_err(read_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(read_error)?;
        for key in keys {
            let Some((tag, rest)) = key
                .strip_prefix(VERSION_2_PREFIX)
                .and_then(|rest| rest.split_once("}:"))
            else {
                continue;
            };
            groups
                .entry(tag.to_owned())
                .or_default()
                .push((rest.to_owned(), key.clone()));
        }
        let mut events = Vec::new();
        let mut chains = BTreeMap::<OldAddress, String>::new();
        for (tag, parts) in &groups {
            self.read_group(tag, parts, from_1, &mut events, &mut chains)?;
        }
        for key in migration::migrate(events, &chains)? {
            self.write_key(&key.stream, &key.key, &key.events, &key.state)?;
        }
        for parts in groups.values() {
            let keys = parts
                .iter()
                .map(|(_, key)| key.as_str())
                .collect::<Vec<_>>();
            self.connection.del::<_, ()>(keys).map_err(write_error)?;
        }
        Ok(())
    }

    /// One version 2 resource's events and chains, under the stand-in data
    /// source `tag`.
    fn read_group(
        &mut self,
        tag: &str,
        parts: &[(String, String)],
        from_1: bool,
        events: &mut Vec<OldEvent>,
        chains: &mut BTreeMap<OldAddress, String>,
    ) -> Result<(), StoreError> {
        let id_field = if from_1 { "event_ref" } else { "event_id" };
        let find = |part: &str| {
            parts
                .iter()
                .find(|(rest, _)| rest == part)
                .map(|(_, key)| key.clone())
        };
        let Some(heads) = find("heads") else {
            return Ok(());
        };
        let heads: Vec<(String, String)> = self.connection.hgetall(&heads).map_err(read_error)?;
        let Some((aggregate, head)) = heads.first() else {
            return Ok(());
        };
        let head = old_object(head)?;
        let suffix = format!(":{aggregate}:{}", old_number(&head, "version")?);
        let resource = old_text(&head, id_field)?
            .strip_suffix(&suffix)
            .ok_or_else(|| StoreError::corrupt("a version 2 head's id names another aggregate"))?
            .to_owned();
        let source = format!("redis group {tag}");
        for (rest, key) in parts {
            let Some(stream) = rest.strip_prefix("events:") else {
                continue;
            };
            let records: Vec<String> = self.connection.lrange(key, 0, -1).map_err(read_error)?;
            let mut aggregate = None;
            for record in &records {
                let record = old_object(record)?;
                let version = old_number(&record, "version")?;
                let named = old_text(&record, id_field)?
                    .strip_prefix(&format!("{resource}:"))
                    .and_then(|rest| rest.strip_suffix(&format!(":{version}")))
                    .ok_or_else(|| StoreError::corrupt("a version 2 event names another resource"))?
                    .to_owned();
                if aggregate.get_or_insert_with(|| named.clone()) != &named {
                    return Err(StoreError::corrupt(
                        "a version 2 stream holds two aggregates",
                    ));
                }
                events.push(OldEvent {
                    data_source_uri: source.clone(),
                    resource: resource.clone(),
                    aggregate_id: named,
                    version,
                    event_type: old_text(&record, "event_type")?,
                    data: record
                        .get("event")
                        .and_then(JsonValue::as_object)
                        .cloned()
                        .ok_or_else(|| StoreError::corrupt("a version 2 event has no body"))?,
                    digest: old_text(&record, "event_digest")?,
                    idempotency_key: old_text(&record, "idempotency_key")?,
                    committed_at: old_text(&record, "committed_at")?,
                });
            }
            let (Some(aggregate), Some(projection)) =
                (aggregate, find(&format!("projection:{stream}")))
            else {
                continue;
            };
            let projection: Option<String> =
                self.connection.get(&projection).map_err(read_error)?;
            if let Some(projection) = projection {
                chains.insert(
                    (source.clone(), resource.clone(), aggregate),
                    old_text(&old_object(&projection)?, "projection_digest")?,
                );
            }
        }
        Ok(())
    }

    /// Write one migrated key whole, replacing whatever a migration that
    /// stopped partway left of it.
    fn write_key(
        &mut self,
        stream: &str,
        key: &str,
        events: &[EventRecord],
        state: &KeyState,
    ) -> Result<(), StoreError> {
        let keys = Keys::of(stream, key)?;
        let Some(last) = &state.last else {
            return Ok(());
        };
        let records = events
            .iter()
            .map(|event| encoded(&JsonValue::Object(model::record_object(event))))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pipe = redis::pipe();
        pipe.atomic()
            .del(&keys.events)
            .del(&keys.idempotency)
            .rpush(&keys.events, &records);
        for (event, record) in events.iter().zip(&records) {
            pipe.hset(&keys.idempotency, &event.idempotency_key, record);
        }
        let member = order_member(&last.committed_at, key);
        let previous: Option<String> = self
            .connection
            .hget(&keys.members, key)
            .map_err(read_error)?;
        if let Some(previous) = previous {
            pipe.zrem(&keys.order, previous);
        }
        pipe.set(&keys.state, state_json(state)?)
            .zadd(&keys.order, &member, 0)
            .hset(&keys.members, key, &member)
            .hset(
                &keys.keys,
                key,
                encoded(&JsonValue::Object(model::record_object(last)))?,
            );
        pipe.query::<()>(&mut self.connection).map_err(write_error)
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

fn old_object(text: &str) -> Result<JsonObject, StoreError> {
    serde_json::from_str::<JsonValue>(text)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| StoreError::corrupt("a version 2 value is not a JSON object"))
}

fn old_text(object: &JsonObject, field: &str) -> Result<String, StoreError> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| StoreError::corrupt(format!("a version 2 value has no {field}")))
}

fn old_number(object: &JsonObject, field: &str) -> Result<u64, StoreError> {
    object
        .get(field)
        .and_then(model::unsigned)
        .ok_or_else(|| StoreError::corrupt(format!("a version 2 value has no {field}")))
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
