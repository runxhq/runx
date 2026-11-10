//! Opening, identifying and versioning the kernel store database.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use super::{BUSY_TIMEOUT, KernelStoreError};

/// `RUNK`: marks a kernel store. A `store` connector's database never carries it.
const APPLICATION_ID: i64 = 0x5255_4e4b;
/// 0.12 raised it from 2 when `kernel_streams` and its `kind` became
/// `kernel_keys` and `stream`, so a released runx refuses the store it opens.
/// The store's file is named for it, so each format's file sits beside the
/// last.
pub(super) const SCHEMA_VERSION: i64 = 3;

/// The format runx 0.11 wrote its store in.
const RUNX_0_11_SCHEMA_VERSION: i64 = 2;

/// runx 0.11's store at `path`, opened only to read, when one is there: this
/// runx never writes it and never moves it, so runx 0.11 can still finish
/// what it left there.
fn open_runx_0_11(path: &Path) -> Result<Option<Connection>, KernelStoreError> {
    let database = |operation: &'static str| {
        move |source| KernelStoreError::Database {
            path: path.to_path_buf(),
            operation,
            source,
        }
    };
    if !path.is_file() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(database("opening the runx 0.11 store"))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(database("setting the busy timeout"))?;
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(database("reading the application id"))?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(database("reading the schema version"))?;
    Ok(
        (application_id == APPLICATION_ID && version == RUNX_0_11_SCHEMA_VERSION)
            .then_some(connection),
    )
}

/// The runs runx 0.11 left unsealed in its store at `path`: each top-level
/// run whose stream's head isn't its seal. A child run is its parent's.
pub(super) fn runx_0_11_unsealed_runs(path: &Path) -> Result<Vec<String>, KernelStoreError> {
    let database = |operation: &'static str| {
        move |source| KernelStoreError::Database {
            path: path.to_path_buf(),
            operation,
            source,
        }
    };
    let Some(connection) = open_runx_0_11(path)? else {
        return Ok(Vec::new());
    };
    let mut statement = connection
        .prepare(
            "SELECT key FROM kernel_streams
             WHERE kind = 'run' AND instr(key, '/') = 0 AND event_type <> 'sealed'
             ORDER BY key",
        )
        .map_err(database("reading the runx 0.11 runs"))?;
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(database("reading the runx 0.11 runs"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database("reading the runx 0.11 runs"))
}

/// Each instance runx 0.11 confirmed in its store at `path` and didn't
/// revoke, by id, with its last event's bytes, and each connector's last
/// selection's bytes.
#[allow(clippy::type_complexity)]
pub(super) fn runx_0_11_instances(
    path: &Path,
) -> Result<(Vec<(String, Vec<u8>)>, Vec<Vec<u8>>), KernelStoreError> {
    let database = |operation: &'static str| {
        move |source| KernelStoreError::Database {
            path: path.to_path_buf(),
            operation,
            source,
        }
    };
    let Some(connection) = open_runx_0_11(path)? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let heads = |kind: &str, head: &str| -> Result<Vec<(String, Vec<u8>)>, KernelStoreError> {
        let mut statement = connection
            .prepare(
                "SELECT s.key, e.data FROM kernel_streams s
                 JOIN kernel_events e ON e.kind = s.kind AND e.key = s.key AND e.version = s.version
                 WHERE s.kind = ?1 AND s.event_type = ?2
                 ORDER BY s.key",
            )
            .map_err(database("reading the runx 0.11 instances"))?;
        statement
            .query_map([kind, head], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(database("reading the runx 0.11 instances"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database("reading the runx 0.11 instances"))
    };
    let instances = heads("instance", "confirmed")?;
    let selections = heads("instance_selection", "selected")?
        .into_iter()
        .map(|(_, data)| data)
        .collect();
    Ok((instances, selections))
}

/// Whether runx 0.11's store at `path` recorded run `run_id`.
pub(super) fn runx_0_11_recorded_run(path: &Path, run_id: &str) -> Result<bool, KernelStoreError> {
    let Some(connection) = open_runx_0_11(path)? else {
        return Ok(false);
    };
    connection
        .query_row(
            "SELECT 1 FROM kernel_streams WHERE kind = 'run' AND key = ?1",
            [run_id],
            |_| Ok(()),
        )
        .optional()
        .map(|found| found.is_some())
        .map_err(|source| KernelStoreError::Database {
            path: path.to_path_buf(),
            operation: "looking up a runx 0.11 run",
            source,
        })
}

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

/// `sequence` is the store's commit order: one number per event across every
/// stream, assigned at commit, so it only grows and no two events share one.
/// A key's row holds its newest event's. `tree` is the root key of the event's
/// stream tree, its key up to the first `/`, so a tree's events after a commit
/// are one range of an index.
const SCHEMA: &str = "
CREATE TABLE kernel_keys (
  stream TEXT NOT NULL,
  key TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version > 0),
  event_type TEXT NOT NULL,
  chain TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  PRIMARY KEY (stream, key)
) WITHOUT ROWID;

CREATE TABLE kernel_events (
  stream TEXT NOT NULL,
  key TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version > 0),
  event_type TEXT NOT NULL,
  data BLOB NOT NULL,
  chain TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  sequence INTEGER NOT NULL UNIQUE CHECK (sequence > 0),
  tree TEXT NOT NULL,
  PRIMARY KEY (stream, key, version)
) WITHOUT ROWID;

CREATE INDEX kernel_keys_recent ON kernel_keys (stream, sequence);
CREATE INDEX kernel_events_tree ON kernel_events (stream, tree, sequence);

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
        (APPLICATION_ID, 1 | 2) => {
            return Err(KernelStoreError::BeforeThisRunx {
                path: path.to_path_buf(),
            });
        }
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
    let _open = crate::work_counts::OpenTransaction::begin();
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
