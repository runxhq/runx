//! The SQLite backend: the workspace store, `private`'s own file in the
//! runx home, or a file an instance names.
//!
//! The database carries its own application id, one for `store` and one for
//! `private`, so it is never the kernel's protected store, another
//! program's file, or the other connector's: a `store` instance naming the
//! private file is refused, and so is the reverse. `private`'s file also
//! keeps a full-text index of each key's newest event, which `keys.search`
//! ranks with BM25 (SQLite FTS5); an append replaces its key's entry.

use std::path::Path;
use std::time::Duration;

use runx_contracts::JsonObject;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::model::{self, Appended, EventRecord, KeyState, Listed, Matched, Writer};
use super::request::{Append, Key, ListKeys, ReadEvents, SearchKeys};
use super::{Backend, Kind, StoreError, Streams, append_in};

/// "RUNS": a `store` file, distinct from the kernel store's "RUNK".
const STORE_APPLICATION_ID: i64 = 0x5255_4e53;
/// "RUNP": a `private` file.
const PRIVATE_APPLICATION_ID: i64 = 0x5255_4e50;
/// 0.12 raised it from 2. A store from before 0.12 is never read: it refuses
/// to open, and a released runx refuses a store whose version isn't its own
/// rather than misreading a changed event.
const SCHEMA_VERSION: i64 = 3;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const SCHEMA: &str = r#"
CREATE TABLE store_events (
  stream TEXT NOT NULL,
  key TEXT NOT NULL,
  version INTEGER NOT NULL,
  idempotency_key TEXT NOT NULL,
  id TEXT NOT NULL,
  type TEXT NOT NULL,
  digest TEXT NOT NULL,
  data TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  writer_package TEXT NOT NULL,
  writer_digest TEXT NOT NULL,
  PRIMARY KEY (stream, key, version),
  UNIQUE (stream, key, idempotency_key)
);
CREATE TABLE store_keys (
  stream TEXT NOT NULL,
  key TEXT NOT NULL,
  version INTEGER NOT NULL,
  id TEXT NOT NULL,
  type TEXT NOT NULL,
  digest TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  data TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  writer_package TEXT NOT NULL,
  writer_digest TEXT NOT NULL,
  chain_digest TEXT NOT NULL,
  PRIMARY KEY (stream, key)
);
CREATE INDEX store_keys_recent ON store_keys (stream, committed_at DESC, key ASC);
CREATE INDEX store_keys_type_recent ON store_keys (stream, type, committed_at DESC, key ASC);
"#;

/// `private`'s index of each key's newest event, one row per key by the
/// key's rowid, shared by every stream so scores compare across streams.
const SEARCH_SCHEMA: &str = "CREATE VIRTUAL TABLE store_search USING fts5(text);";

const EVENT_COLUMNS: &str =
    "id, version, type, digest, idempotency_key, committed_at, data, writer_package, writer_digest";

pub(super) struct Sqlite {
    connection: Connection,
    kind: Kind,
}

impl Sqlite {
    /// Append each event of `history` in one transaction, each at its own
    /// `committed_at` and at the version it expects, as a probe lays down a
    /// long record. A call appends one event in a transaction of its own.
    #[cfg(feature = "probe-fixtures")]
    pub(super) fn append_history(
        &mut self,
        history: &[(Append<'_>, String)],
    ) -> Result<(), StoreError> {
        let kind = self.kind;
        let _open = crate::work_counts::OpenTransaction::begin();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| database("starting the history", &error))?;
        let mut streams = SqliteStreams(transaction, kind);
        for (append, committed_at) in history {
            match append_in(&mut streams, append, committed_at)? {
                Appended::Committed { .. } => {}
                _ => {
                    return Err(StoreError::invalid(
                        "a history's event didn't commit at the version it expected",
                    ));
                }
            }
        }
        streams
            .0
            .commit()
            .map_err(|error| database("committing the history", &error))
    }

    pub(super) fn open(path: &Path, kind: Kind) -> Result<Self, StoreError> {
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
        let mut store = Self { connection, kind };
        store.ensure_schema(path)?;
        Ok(store)
    }

    fn application_id(&self) -> i64 {
        match self.kind {
            Kind::Store => STORE_APPLICATION_ID,
            Kind::Private => PRIVATE_APPLICATION_ID,
        }
    }

    fn ensure_schema(&mut self, path: &Path) -> Result<(), StoreError> {
        let pragma = |connection: &Connection, name: &str| -> Result<i64, StoreError> {
            connection
                .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
                .map_err(|error| database(&format!("reading {name}"), &error))
        };
        let application_id = pragma(&self.connection, "application_id")?;
        let user_version = pragma(&self.connection, "user_version")?;
        let own = self.application_id();
        let foreign = || {
            StoreError::invalid(match self.kind {
                Kind::Store => "the file is not a runx store; name a new file or a store",
                Kind::Private => "the file is not a private store; name a new file or one",
            })
        };
        match (application_id, user_version) {
            (id, SCHEMA_VERSION) if id == own => return Ok(()),
            // Only `store` was ever written before 0.12, and 0.12 reads none
            // of it.
            (STORE_APPLICATION_ID, 1 | 2) if own == STORE_APPLICATION_ID => {
                return Err(StoreError::invalid(format!(
                    "{} is a store from before runx 0.12, which 0.12 doesn't read; move the file aside to start a new store",
                    path.display()
                )));
            }
            (0, 0) => {}
            (id, version) if id == own => {
                return Err(StoreError::corrupt(format!(
                    "the store has schema version {version}, not {SCHEMA_VERSION}"
                )));
            }
            _ => return Err(foreign()),
        }
        let objects: i64 = self
            .connection
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
            .map_err(|error| database("reading the schema", &error))?;
        if objects != 0 {
            return Err(foreign());
        }
        self.connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| database("enabling the write-ahead log", &error))?;
        let kind = self.kind;
        let _open = crate::work_counts::OpenTransaction::begin();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| database("starting the schema", &error))?;
        create(&transaction, kind)?;
        transaction
            .pragma_update(None, "application_id", own)
            .map_err(|error| database("marking the store", &error))?;
        version(transaction)
    }
}

fn create(transaction: &Transaction<'_>, kind: Kind) -> Result<(), StoreError> {
    transaction
        .execute_batch(SCHEMA)
        .map_err(|error| database("creating the schema", &error))?;
    if kind == Kind::Private {
        transaction
            .execute_batch(SEARCH_SCHEMA)
            .map_err(|error| database("creating the search index", &error))?;
    }
    Ok(())
}

fn version(transaction: Transaction<'_>) -> Result<(), StoreError> {
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| database("versioning the store", &error))?;
    transaction
        .commit()
        .map_err(|error| database("committing the schema", &error))
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
        let kind = self.kind;
        let _open = crate::work_counts::OpenTransaction::begin();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| database("starting the append", &error))?;
        let mut streams = SqliteStreams(transaction, kind);
        let appended = append_in(&mut streams, request, committed_at)?;
        // SQLite either commits or reports that it did not.
        streams
            .0
            .commit()
            .map_err(|error| database("committing the append", &error))?;
        Ok(appended)
    }

    fn state(&mut self, at: Key<'_>) -> Result<KeyState, StoreError> {
        state(&self.connection, at)
    }

    fn events(&mut self, request: &ReadEvents<'_>) -> Result<(Vec<EventRecord>, bool), StoreError> {
        let at = request.at;
        let (order, limit) = match request.after_version {
            Some(_) => ("ASC", request.limit + 1),
            // The latest page, read newest first and returned in version
            // order.
            None => ("DESC", request.limit),
        };
        let sql = format!(
            "SELECT {EVENT_COLUMNS} FROM store_events WHERE stream = ?1 AND key = ?2 AND version > ?3 ORDER BY version {order} LIMIT ?4"
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| database("reading events", &error))?;
        let mut events = statement
            .query_map(
                params![
                    at.stream,
                    at.key,
                    integer(request.after_version.unwrap_or(0))?,
                    integer(limit as u64)?,
                ],
                |row| {
                    crate::work_counts::count(crate::work_counts::Work::StoreEventRowRead);
                    record_at(row, 0)
                },
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

    /// A page of a stream's keys, newest commit first. The listing steps the
    /// recency index row by row and keeps the types it was asked for itself,
    /// so every row it visits is a row it steps and counts, the ones it
    /// passes over included, and it stops once it holds the page and one more.
    fn keys(&mut self, request: &ListKeys<'_>) -> Result<(Vec<Listed>, bool), StoreError> {
        let cursor = model::decode_cursor(request.cursor)?;
        let sql = keys_query(cursor.is_some());
        let mut values = vec![SqlValue::Text(request.stream.to_owned())];
        if let Some(cursor) = cursor {
            values.push(SqlValue::Text(cursor.committed_at.clone()));
            values.push(SqlValue::Text(cursor.committed_at));
            values.push(SqlValue::Text(cursor.key));
        }
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| database("reading keys", &error))?;
        let mut rows = statement
            .query(rusqlite::params_from_iter(values.iter()))
            .map_err(|error| database("reading keys", &error))?;
        let mut keys = Vec::new();
        while keys.len() <= request.limit {
            let Some(row) = rows
                .next()
                .map_err(|error| database("reading keys", &error))?
            else {
                break;
            };
            crate::work_counts::count(crate::work_counts::Work::StoreKeyRowVisited);
            let event_type: String = row
                .get(3)
                .map_err(|error| database("reading keys", &error))?;
            if !request.types.is_empty() && !request.types.contains(&event_type.as_str()) {
                crate::work_counts::count(crate::work_counts::Work::StoreKeyRowPassedOver);
                continue;
            }
            keys.push(Listed {
                key: row
                    .get(0)
                    .map_err(|error| database("reading keys", &error))?,
                last: record_at(row, 1).map_err(|error| database("reading keys", &error))?,
            });
        }
        let has_more = keys.len() > request.limit;
        keys.truncate(request.limit);
        Ok((keys, has_more))
    }

    fn search(&mut self, request: &SearchKeys<'_>) -> Result<(Vec<Matched>, bool), StoreError> {
        if self.kind != Kind::Private {
            return Err(StoreError::invalid("only private's streams are searched"));
        }
        crate::work_counts::count(crate::work_counts::Work::StoreSearchPage);
        let Some(query) = model::quoted_query(request.query) else {
            return Ok((Vec::new(), false));
        };
        let cursor = model::decode_search_cursor(request.cursor)?;
        // BM25 is lower for a better match; an answer's score is higher.
        let mut sql = format!(
            "SELECT k.key, -bm25(store_search) AS score, {} FROM store_search JOIN store_keys k ON k.rowid = store_search.rowid WHERE store_search MATCH ? AND k.stream = ?",
            EVENT_COLUMNS
                .split(", ")
                .map(|column| format!("k.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut values = vec![
            SqlValue::Text(query),
            SqlValue::Text(request.stream.to_owned()),
        ];
        if !request.types.is_empty() {
            sql.push_str(" AND k.type IN (");
            sql.push_str(&vec!["?"; request.types.len()].join(","));
            sql.push(')');
            values.extend(
                request
                    .types
                    .iter()
                    .map(|value| SqlValue::Text((*value).to_owned())),
            );
        }
        let mut sql = format!("SELECT * FROM ({sql})");
        if let Some(cursor) = cursor {
            sql.push_str(" WHERE score < ? OR (score = ? AND key > ?)");
            values.push(SqlValue::Real(cursor.score));
            values.push(SqlValue::Real(cursor.score));
            values.push(SqlValue::Text(cursor.key));
        }
        sql.push_str(" ORDER BY score DESC, key ASC LIMIT ?");
        values.push(SqlValue::Integer(integer(request.limit as u64 + 1)?));
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| database("searching keys", &error))?;
        let mut matches = statement
            .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok(Matched {
                    key: row.get(0)?,
                    score: row.get(1)?,
                    last: record_at(row, 2)?,
                })
            })
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .map_err(|error| database("searching keys", &error))?;
        let has_more = matches.len() > request.limit;
        matches.truncate(request.limit);
        Ok((matches, has_more))
    }

    fn landed(&mut self, at: Key<'_>, idempotency_key: &str) -> Result<Option<String>, StoreError> {
        Ok(existing(&self.connection, at, idempotency_key)?.map(|record| record.digest))
    }
}

struct SqliteStreams<'a>(Transaction<'a>, Kind);

impl Streams for SqliteStreams<'_> {
    fn state(&mut self, at: Key<'_>) -> Result<KeyState, StoreError> {
        state(&self.0, at)
    }

    fn existing(
        &mut self,
        at: Key<'_>,
        idempotency_key: &str,
    ) -> Result<Option<EventRecord>, StoreError> {
        existing(&self.0, at, idempotency_key)
    }

    fn commit_event(
        &mut self,
        at: Key<'_>,
        record: &EventRecord,
        state: &KeyState,
    ) -> Result<(), StoreError> {
        insert_event(&self.0, at, record)?;
        upsert_key(&self.0, at, record, state)?;
        if self.1 == Kind::Private {
            // The key's entry is its newest event: an append replaces it.
            let rowid: i64 = self
                .0
                .query_row(
                    "SELECT rowid FROM store_keys WHERE stream = ?1 AND key = ?2",
                    params![at.stream, at.key],
                    |row| row.get(0),
                )
                .map_err(|error| database("finding the key to index", &error))?;
            self.0
                .execute("DELETE FROM store_search WHERE rowid = ?1", params![rowid])
                .map_err(|error| database("replacing the key's index entry", &error))?;
            self.0
                .execute(
                    "INSERT INTO store_search (rowid, text) VALUES (?1, ?2)",
                    params![rowid, model::searchable_text(&record.data)],
                )
                .map_err(|error| database("indexing the key", &error))?;
        }
        Ok(())
    }
}

fn data_json(record: &EventRecord) -> Result<String, StoreError> {
    serde_json::to_string(&record.data)
        .map_err(|error| StoreError::invalid(format!("encoding the event: {error}")))
}

fn writer_columns(record: &EventRecord) -> (&str, &str) {
    (
        record.writer.package.as_str(),
        record.writer.digest.as_str(),
    )
}

fn insert_event(
    connection: &Connection,
    at: Key<'_>,
    record: &EventRecord,
) -> Result<(), StoreError> {
    let (writer_package, writer_digest) = writer_columns(record);
    connection
        .execute(
            "INSERT INTO store_events (stream, key, version, idempotency_key, id, type, digest, data, committed_at, writer_package, writer_digest) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                at.stream,
                at.key,
                integer(record.version)?,
                record.idempotency_key,
                record.id,
                record.event_type,
                record.digest,
                data_json(record)?,
                record.committed_at,
                writer_package,
                writer_digest,
            ],
        )
        .map_err(|error| database("inserting the event", &error))?;
    Ok(())
}

fn upsert_key(
    connection: &Connection,
    at: Key<'_>,
    record: &EventRecord,
    state: &KeyState,
) -> Result<(), StoreError> {
    let (writer_package, writer_digest) = writer_columns(record);
    connection
        .execute(
            "INSERT INTO store_keys (stream, key, version, id, type, digest, idempotency_key, data, committed_at, writer_package, writer_digest, chain_digest) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT (stream, key) DO UPDATE SET version = excluded.version, id = excluded.id, type = excluded.type, digest = excluded.digest, idempotency_key = excluded.idempotency_key, data = excluded.data, committed_at = excluded.committed_at, writer_package = excluded.writer_package, writer_digest = excluded.writer_digest, chain_digest = excluded.chain_digest",
            params![
                at.stream,
                at.key,
                integer(record.version)?,
                record.id,
                record.event_type,
                record.digest,
                record.idempotency_key,
                data_json(record)?,
                record.committed_at,
                writer_package,
                writer_digest,
                state.chain_digest,
            ],
        )
        .map_err(|error| database("moving the key", &error))?;
    Ok(())
}

fn state(connection: &Connection, at: Key<'_>) -> Result<KeyState, StoreError> {
    let row = connection
        .query_row(
            &format!("SELECT chain_digest, {EVENT_COLUMNS} FROM store_keys WHERE stream = ?1 AND key = ?2"),
            params![at.stream, at.key],
            |row| Ok((row.get::<_, String>(0)?, record_at(row, 1)?)),
        )
        .optional()
        .map_err(|error| database("reading the key", &error))?;
    let Some((chain_digest, last)) = row else {
        return model::empty_state();
    };
    Ok(KeyState {
        version: last.version,
        last: Some(last),
        chain_digest,
    })
}

fn existing(
    connection: &Connection,
    at: Key<'_>,
    idempotency_key: &str,
) -> Result<Option<EventRecord>, StoreError> {
    connection
        .query_row(
            &format!("SELECT {EVENT_COLUMNS} FROM store_events WHERE stream = ?1 AND key = ?2 AND idempotency_key = ?3"),
            params![at.stream, at.key, idempotency_key],
            |row| record_at(row, 0),
        )
        .optional()
        .map_err(|error| database("reading the event under the key", &error))
}

/// The event whose [`EVENT_COLUMNS`] start at `offset`.
/// The query a page of a stream's keys steps, newest commit first, after a
/// cursor when it has one. It filters nothing and limits nothing, so the
/// listing that steps it sees every row it visits.
pub(super) fn keys_query(after_cursor: bool) -> String {
    let mut sql = format!("SELECT key, {EVENT_COLUMNS} FROM store_keys WHERE stream = ?");
    if after_cursor {
        sql.push_str(" AND (committed_at < ? OR (committed_at = ? AND key > ?))");
    }
    sql.push_str(" ORDER BY committed_at DESC, key ASC");
    sql
}

fn record_at(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<EventRecord> {
    let corrupt = |index: usize, error: Box<dyn std::error::Error + Send + Sync>| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, error)
    };
    let data = row.get::<_, String>(offset + 6)?;
    let data = serde_json::from_str::<JsonObject>(&data)
        .map_err(|error| corrupt(offset + 6, Box::new(error)))?;
    let writer = Writer {
        package: row.get(offset + 7)?,
        digest: row.get(offset + 8)?,
    };
    Ok(EventRecord {
        id: row.get(offset)?,
        version: u64::try_from(row.get::<_, i64>(offset + 1)?)
            .map_err(|error| corrupt(offset + 1, Box::new(error)))?,
        event_type: row.get(offset + 2)?,
        digest: row.get(offset + 3)?,
        idempotency_key: row.get(offset + 4)?,
        committed_at: row.get(offset + 5)?,
        data,
        writer,
    })
}

fn integer(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::invalid("a number exceeds the store's range"))
}

fn database(operation: &str, error: &rusqlite::Error) -> StoreError {
    StoreError::unavailable(format!("the SQLite store failed {operation}: {error}"))
}
