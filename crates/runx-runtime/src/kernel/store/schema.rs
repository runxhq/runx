//! Opening, identifying and versioning the kernel store database.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use super::{BUSY_TIMEOUT, KernelStoreError};

/// `RUNK`: marks a kernel store. A `store` connector's database never carries it.
const APPLICATION_ID: i64 = 0x5255_4e4b;
const SCHEMA_VERSION: i64 = 2;

/// The acts whose provider effects runx 0.10 left in flight in its store.
/// 0.11 reads none of that log, and its own idempotency keys differ, so a
/// re-run of one of these acts could send the effect twice. Only the stream
/// heads are read: an effect stream settles with a `finalized` or `rejected`
/// head, and any other head is an attempt whose outcome is not settled.
pub(super) fn retired_effects_in_flight(retired: &Path) -> Result<Vec<String>, KernelStoreError> {
    let database = |operation: &'static str| {
        move |source| KernelStoreError::Database {
            path: retired.to_path_buf(),
            operation,
            source,
        }
    };
    if !retired.is_file() {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(retired, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(database("opening the runx 0.10 store"))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(database("setting the busy timeout"))?;
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(database("reading the application id"))?;
    if application_id != APPLICATION_ID {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT key FROM kernel_streams
             WHERE kind = 'effect' AND event_type NOT IN ('finalized', 'rejected')
             ORDER BY key",
        )
        .map_err(database("reading effect heads"))?;
    let keys = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(database("reading effect heads"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database("reading effect heads"))?;
    Ok(keys)
}

const SCHEMA: &str = "
CREATE TABLE kernel_streams (
  kind TEXT NOT NULL,
  key TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version > 0),
  event_type TEXT NOT NULL,
  chain TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (kind, key)
) WITHOUT ROWID;

CREATE TABLE kernel_events (
  kind TEXT NOT NULL,
  key TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version > 0),
  event_type TEXT NOT NULL,
  data BLOB NOT NULL,
  chain TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  PRIMARY KEY (kind, key, version)
) WITHOUT ROWID;

CREATE INDEX kernel_streams_recent ON kernel_streams (kind, updated_at, key);

CREATE TABLE kernel_blobs (
  digest TEXT NOT NULL PRIMARY KEY,
  data BLOB NOT NULL
);
";

pub(super) fn open(path: &Path) -> Result<Connection, KernelStoreError> {
    let database = |operation: &'static str| {
        move |source| KernelStoreError::Database {
            path: path.to_path_buf(),
            operation,
            source,
        }
    };
    let mut connection = Connection::open(path).map_err(database("opening"))?;
    restrict(path)?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(database("setting the busy timeout"))?;
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(database("reading the application id"))?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(database("reading the schema version"))?;
    match (application_id, version) {
        (APPLICATION_ID, SCHEMA_VERSION) => {}
        (0, 0) => initialize(&mut connection, path)?,
        _ => {
            return Err(KernelStoreError::Foreign {
                path: path.to_path_buf(),
            });
        }
    }
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(database("enabling WAL"))?;
    // Durable before acknowledged: a commit survives power loss.
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(database("requiring durable commits"))?;
    Ok(connection)
}

fn initialize(connection: &mut Connection, path: &Path) -> Result<(), KernelStoreError> {
    let database = |operation: &'static str| {
        move |source| KernelStoreError::Database {
            path: path.to_path_buf(),
            operation,
            source,
        }
    };
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database("locking initialization"))?;
    let tables: i64 = transaction
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |row| row.get(0),
        )
        .map_err(database("inspecting tables"))?;
    let application_id: i64 = transaction
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(database("rechecking the application id"))?;
    if application_id == APPLICATION_ID {
        // Another process initialized it while this one waited for the lock.
        return transaction
            .commit()
            .map_err(database("closing initialization"));
    }
    if tables != 0 {
        return Err(KernelStoreError::Foreign {
            path: path.to_path_buf(),
        });
    }
    transaction
        .execute_batch(SCHEMA)
        .map_err(database("creating the schema"))?;
    transaction
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(database("marking the store"))?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(database("sealing the schema version"))?;
    transaction
        .commit()
        .map_err(database("committing the schema"))
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<(), KernelStoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        KernelStoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<(), KernelStoreError> {
    Ok(())
}
