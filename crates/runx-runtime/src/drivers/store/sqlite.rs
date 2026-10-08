//! The SQLite backend: the workspace store, or a file an instance names.
//!
//! The database carries its own application id, so it is never the kernel's
//! protected store or another program's file.

use std::path::Path;
use std::time::Duration;

use runx_contracts::JsonObject;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::model::{self, Appended, EventRecord, Projection, StreamHead};
use super::request::{Append, ListHeads, ReadEvents, Source};
use super::{Backend, StoreError, Streams, append_in};

/// "RUNS": a runx store, distinct from the kernel store's "RUNK".
const APPLICATION_ID: i64 = 0x5255_4e53;
const SCHEMA_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS runx_events (
  data_source_ref TEXT NOT NULL,
  resource TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  idempotency_key TEXT NOT NULL,
  event_ref TEXT NOT NULL,
  event_type TEXT NOT NULL,
  event_digest TEXT NOT NULL,
  event_json TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  PRIMARY KEY (data_source_ref, resource, aggregate_id, version),
  UNIQUE (data_source_ref, resource, aggregate_id, idempotency_key)
);
CREATE TABLE IF NOT EXISTS runx_stream_heads (
  data_source_ref TEXT NOT NULL,
  resource TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  event_ref TEXT NOT NULL,
  event_type TEXT NOT NULL,
  event_digest TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  event_json TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  projection_digest TEXT NOT NULL,
  PRIMARY KEY (data_source_ref, resource, aggregate_id)
);
CREATE INDEX IF NOT EXISTS runx_stream_heads_recent
  ON runx_stream_heads (data_source_ref, resource, committed_at DESC, aggregate_id ASC);
CREATE INDEX IF NOT EXISTS runx_stream_heads_type_recent
  ON runx_stream_heads (data_source_ref, resource, event_type, committed_at DESC, aggregate_id ASC);
"#;

pub(super) struct Sqlite {
    connection: Connection,
}

impl Sqlite {
    pub(super) fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                StoreError::unavailable(format!("creating {}: {error}", parent.display()))
            })?;
        }
        let connection = Connection::open(path).map_err(|error| {
            StoreError::unavailable(format!("opening {}: {error}", path.display()))
        })?;
        restrict(path)?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| database("setting the busy timeout", &error))?;
        let mut store = Self { connection };
        store.ensure_schema()?;
        Ok(store)
    }

    fn ensure_schema(&mut self) -> Result<(), StoreError> {
        let pragma = |connection: &Connection, name: &str| -> Result<i64, StoreError> {
            connection
                .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
                .map_err(|error| database(&format!("reading {name}"), &error))
        };
        let application_id = pragma(&self.connection, "application_id")?;
        let user_version = pragma(&self.connection, "user_version")?;
        match (application_id, user_version) {
            (APPLICATION_ID, SCHEMA_VERSION) => return Ok(()),
            (0, 0) => {}
            (APPLICATION_ID, version) => {
                return Err(StoreError::corrupt(format!(
                    "the store has schema version {version}, not {SCHEMA_VERSION}"
                )));
            }
            _ => {
                return Err(StoreError::invalid(
                    "the file is not a runx store; name a new file or a store",
                ));
            }
        }
        let objects: i64 = self
            .connection
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
            .map_err(|error| database("reading the schema", &error))?;
        if objects != 0 {
            return Err(StoreError::invalid(
                "the file is not a runx store; name a new file or a store",
            ));
        }
        self.connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| database("enabling the write-ahead log", &error))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| database("starting the schema", &error))?;
        transaction
            .execute_batch(SCHEMA)
            .map_err(|error| database("creating the schema", &error))?;
        transaction
            .pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(|error| database("marking the store", &error))?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|error| database("versioning the store", &error))?;
        transaction
            .commit()
            .map_err(|error| database("committing the schema", &error))
    }
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        StoreError::unavailable(format!("restricting {}: {error}", path.display()))
    })
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

impl Backend for Sqlite {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn append(&mut self, request: &Append<'_>, committed_at: &str) -> Result<Appended, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| database("starting the append", &error))?;
        let mut streams = SqliteStreams(transaction);
        let appended = append_in(&mut streams, request, committed_at)?;
        // SQLite either commits or reports that it did not.
        streams
            .0
            .commit()
            .map_err(|error| database("committing the append", &error))?;
        Ok(appended)
    }

    fn projection(&mut self, source: Source<'_>) -> Result<Projection, StoreError> {
        projection(&self.connection, source)
    }

    fn events(&mut self, request: &ReadEvents<'_>) -> Result<(Vec<EventRecord>, bool), StoreError> {
        let source = request.source;
        let (sql, limit) = match request.after_version {
            Some(_) => (
                "SELECT event_ref, version, event_type, event_digest, idempotency_key, committed_at, event_json FROM runx_events WHERE data_source_ref = ?1 AND resource = ?2 AND aggregate_id = ?3 AND version > ?4 ORDER BY version ASC LIMIT ?5",
                request.limit + 1,
            ),
            None => (
                "SELECT event_ref, version, event_type, event_digest, idempotency_key, committed_at, event_json FROM runx_events WHERE data_source_ref = ?1 AND resource = ?2 AND aggregate_id = ?3 AND version > ?4 ORDER BY version DESC LIMIT ?5",
                request.limit,
            ),
        };
        let mut statement = self
            .connection
            .prepare(sql)
            .map_err(|error| database("reading events", &error))?;
        let mut events = statement
            .query_map(
                params![
                    source.data_source_ref,
                    source.resource,
                    source.aggregate_id,
                    integer(request.after_version.unwrap_or(0))?,
                    integer(limit as u64)?,
                ],
                |row| record_at(row, 0),
            )
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .map_err(|error| database("reading events", &error))?;
        if request.after_version.is_none() {
            events.reverse();
        }
        let has_more = request.after_version.is_some() && events.len() > request.limit;
        events.truncate(request.limit);
        Ok((events, has_more))
    }

    fn heads(&mut self, request: &ListHeads<'_>) -> Result<(Vec<StreamHead>, bool), StoreError> {
        let cursor = model::decode_cursor(request.cursor)?;
        let mut sql = String::from(
            "SELECT aggregate_id, event_ref, version, event_type, event_digest, idempotency_key, committed_at, event_json FROM runx_stream_heads WHERE data_source_ref = ? AND resource = ?",
        );
        let mut values = vec![
            SqlValue::Text(request.data_source_ref.to_owned()),
            SqlValue::Text(request.resource.to_owned()),
        ];
        if !request.event_types.is_empty() {
            sql.push_str(" AND event_type IN (");
            sql.push_str(&vec!["?"; request.event_types.len()].join(","));
            sql.push(')');
            values.extend(
                request
                    .event_types
                    .iter()
                    .map(|value| SqlValue::Text((*value).to_owned())),
            );
        }
        if let Some(cursor) = cursor {
            sql.push_str(" AND (committed_at < ? OR (committed_at = ? AND aggregate_id > ?))");
            values.push(SqlValue::Text(cursor.committed_at.clone()));
            values.push(SqlValue::Text(cursor.committed_at));
            values.push(SqlValue::Text(cursor.aggregate_id));
        }
        sql.push_str(" ORDER BY committed_at DESC, aggregate_id ASC LIMIT ?");
        values.push(SqlValue::Integer(integer(request.limit as u64 + 1)?));
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| database("reading stream heads", &error))?;
        let mut heads = statement
            .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok(StreamHead {
                    aggregate_id: row.get(0)?,
                    event: record_at(row, 1)?,
                })
            })
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .map_err(|error| database("reading stream heads", &error))?;
        let has_more = heads.len() > request.limit;
        heads.truncate(request.limit);
        Ok((heads, has_more))
    }

    fn landed(
        &mut self,
        source: Source<'_>,
        idempotency_key: &str,
    ) -> Result<Option<String>, StoreError> {
        Ok(existing(&self.connection, source, idempotency_key)?.map(|record| record.event_digest))
    }
}

struct SqliteStreams<'a>(Transaction<'a>);

impl Streams for SqliteStreams<'_> {
    fn projection(&mut self, source: Source<'_>) -> Result<Projection, StoreError> {
        projection(&self.0, source)
    }

    fn existing(
        &mut self,
        source: Source<'_>,
        idempotency_key: &str,
    ) -> Result<Option<EventRecord>, StoreError> {
        existing(&self.0, source, idempotency_key)
    }

    fn commit_event(
        &mut self,
        source: Source<'_>,
        record: &EventRecord,
        projection: &Projection,
    ) -> Result<(), StoreError> {
        let event_json = serde_json::to_string(&record.event)
            .map_err(|error| StoreError::invalid(format!("encoding the event: {error}")))?;
        let version = integer(record.version)?;
        self.0
            .execute(
                "INSERT INTO runx_events (data_source_ref, resource, aggregate_id, version, idempotency_key, event_ref, event_type, event_digest, event_json, committed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    source.data_source_ref,
                    source.resource,
                    source.aggregate_id,
                    version,
                    record.idempotency_key,
                    record.event_ref,
                    record.event_type,
                    record.event_digest,
                    event_json,
                    record.committed_at,
                ],
            )
            .map_err(|error| database("inserting the event", &error))?;
        self.0
            .execute(
                "INSERT INTO runx_stream_heads (data_source_ref, resource, aggregate_id, version, event_ref, event_type, event_digest, idempotency_key, event_json, committed_at, projection_digest) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) ON CONFLICT (data_source_ref, resource, aggregate_id) DO UPDATE SET version = excluded.version, event_ref = excluded.event_ref, event_type = excluded.event_type, event_digest = excluded.event_digest, idempotency_key = excluded.idempotency_key, event_json = excluded.event_json, committed_at = excluded.committed_at, projection_digest = excluded.projection_digest",
                params![
                    source.data_source_ref,
                    source.resource,
                    source.aggregate_id,
                    version,
                    record.event_ref,
                    record.event_type,
                    record.event_digest,
                    record.idempotency_key,
                    event_json,
                    record.committed_at,
                    projection.projection_digest,
                ],
            )
            .map_err(|error| database("moving the stream head", &error))?;
        Ok(())
    }
}

fn projection(connection: &Connection, source: Source<'_>) -> Result<Projection, StoreError> {
    let head = connection
        .query_row(
            "SELECT version, event_ref, event_type, event_digest, projection_digest FROM runx_stream_heads WHERE data_source_ref = ?1 AND resource = ?2 AND aggregate_id = ?3",
            params![source.data_source_ref, source.resource, source.aggregate_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| database("reading the stream projection", &error))?;
    let Some((version, event_ref, event_type, event_digest, projection_digest)) = head else {
        return model::empty_projection();
    };
    Ok(Projection {
        version: u64::try_from(version)
            .map_err(|_| StoreError::corrupt("a stream head has a negative version"))?,
        last_event_ref: Some(event_ref),
        last_event_type: Some(event_type),
        last_event_digest: Some(event_digest),
        projection_digest,
    })
}

fn existing(
    connection: &Connection,
    source: Source<'_>,
    idempotency_key: &str,
) -> Result<Option<EventRecord>, StoreError> {
    connection
        .query_row(
            "SELECT event_ref, version, event_type, event_digest, idempotency_key, committed_at, event_json FROM runx_events WHERE data_source_ref = ?1 AND resource = ?2 AND aggregate_id = ?3 AND idempotency_key = ?4",
            params![
                source.data_source_ref,
                source.resource,
                source.aggregate_id,
                idempotency_key
            ],
            |row| record_at(row, 0),
        )
        .optional()
        .map_err(|error| database("reading the event under the key", &error))
}

fn record_at(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<EventRecord> {
    let corrupt = |index: usize, error: Box<dyn std::error::Error + Send + Sync>| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, error)
    };
    let event_json = row.get::<_, String>(offset + 6)?;
    let event = serde_json::from_str::<JsonObject>(&event_json)
        .map_err(|error| corrupt(offset + 6, Box::new(error)))?;
    Ok(EventRecord {
        event_ref: row.get(offset)?,
        version: u64::try_from(row.get::<_, i64>(offset + 1)?)
            .map_err(|error| corrupt(offset + 1, Box::new(error)))?,
        event_type: row.get(offset + 2)?,
        event_digest: row.get(offset + 3)?,
        idempotency_key: row.get(offset + 4)?,
        committed_at: row.get(offset + 5)?,
        event,
    })
}

fn integer(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::invalid("a number exceeds the store's range"))
}

fn database(operation: &str, error: &rusqlite::Error) -> StoreError {
    StoreError::unavailable(format!("the SQLite store failed {operation}: {error}"))
}
