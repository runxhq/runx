//! The protected kernel store: one SQLite database per project workspace.
//!
//! Every record is an event in a stream addressed by `(kind, key)`. A batch
//! appends to several streams in one transaction, each against the version
//! its writer read, all or nothing; this is how a fact and its consequence
//! are never committed apart. Commits are durable before they return. Each
//! stream carries a hash chain, so a rewritten or replayed history is
//! detected on read.
//!
//! Kernel streams (effects, approvals, resolutions, instances) are written
//! only through crate-private APIs.
//!
//! Values too large or too shared for an event live in content-addressed
//! blobs. A blob is written in the same transaction as the event that names
//! it and is verified against its digest on read, so a stream never refers to
//! bytes that were not committed with it.
//!
//! The file carries its own SQLite `application_id`. A `store` connector
//! binding cannot open it, and this store refuses any other database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

mod schema;

/// The store's file name inside the project `.runx` directory.
pub const KERNEL_STORE_FILE: &str = "kernel-v2.sqlite";

const BUSY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_KEY_BYTES: usize = 512;
const MAX_EVENT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
const MAX_BATCH: usize = 1024;

/// A typed event in one stream kind. The event's tag is stored beside its
/// bytes so heads can be filtered without decoding.
pub trait StreamEvent: Serialize + DeserializeOwned {
    /// The stream kind this event belongs to.
    const KIND: &'static str;

    /// The event's variant name, recorded as its type.
    fn event_type(&self) -> &'static str;
}

/// What a writer read before appending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    /// The stream must not exist yet.
    Absent,
    /// The stream's current version.
    Version(u64),
}

impl Expected {
    fn version(self) -> u64 {
        match self {
            Self::Absent => 0,
            Self::Version(version) => version,
        }
    }
}

/// Content-addressed bytes an event refers to by digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    digest: String,
    data: Vec<u8>,
}

impl Blob {
    pub fn new(data: Vec<u8>) -> Result<Self, KernelStoreError> {
        if data.len() > MAX_BLOB_BYTES {
            return Err(KernelStoreError::BlobTooLarge { bytes: data.len() });
        }
        Ok(Self {
            digest: runx_contracts::sha256_prefixed(&data),
            data,
        })
    }

    pub fn json<T: Serialize + ?Sized>(value: &T) -> Result<Self, KernelStoreError> {
        Self::new(serde_json::to_vec(value).map_err(KernelStoreError::encode)?)
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// One append in a batch.
pub struct Append {
    kind: String,
    key: String,
    expected: Expected,
    event_type: &'static str,
    data: Vec<u8>,
    blobs: Vec<Blob>,
    receipts: Vec<IndexedReceipt>,
}

/// A receipt a commit seals, by its id: the run it belongs to and the blob
/// that holds it, so a reader with only the id finds it in the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedReceipt {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) digest: String,
}

impl Append {
    /// Commit these blobs with the event that names them.
    #[must_use]
    pub fn with_blobs(mut self, blobs: impl IntoIterator<Item = Blob>) -> Self {
        self.blobs.extend(blobs);
        self
    }

    /// Index these receipts, each among this append's blobs, by their ids,
    /// in the commit that seals them.
    #[must_use]
    pub(crate) fn with_receipts(
        mut self,
        receipts: impl IntoIterator<Item = IndexedReceipt>,
    ) -> Self {
        self.receipts.extend(receipts);
        self
    }

    /// A kernel append. Crate-private so hosts cannot write kernel streams.
    pub(crate) fn kernel<E: StreamEvent>(
        key: impl Into<String>,
        expected: Expected,
        event: &E,
    ) -> Result<Self, KernelStoreError> {
        Self::new(E::KIND.to_owned(), key.into(), expected, event)
    }

    fn new<E: StreamEvent>(
        kind: String,
        key: String,
        expected: Expected,
        event: &E,
    ) -> Result<Self, KernelStoreError> {
        if key.is_empty() || key.len() > MAX_KEY_BYTES || key.chars().any(char::is_control) {
            return Err(KernelStoreError::InvalidKey { kind, key });
        }
        let data = serde_json::to_vec(event).map_err(KernelStoreError::encode)?;
        if data.len() > MAX_EVENT_BYTES {
            return Err(KernelStoreError::EventTooLarge { kind, key });
        }
        Ok(Self {
            kind,
            key,
            expected,
            event_type: event.event_type(),
            data,
            blobs: Vec::new(),
            receipts: Vec::new(),
        })
    }
}

/// One stored event, decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded<E> {
    pub version: u64,
    pub committed_at: String,
    pub event: E,
}

/// A stream's current position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub key: String,
    pub version: u64,
    pub event_type: String,
    pub updated_at: String,
    /// Its newest event's place in the store's commit order.
    pub sequence: u64,
}

/// One event of a stream tree: the stream it's in, and its place in the
/// store's commit order.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeEvent<E> {
    pub key: String,
    pub sequence: u64,
    pub recorded: Recorded<E>,
}

#[derive(Debug, Error)]
pub enum KernelStoreError {
    #[error("kernel store stream {kind}/{key} is at version {actual}, expected {expected}")]
    Conflict {
        kind: String,
        key: String,
        expected: u64,
        actual: u64,
    },
    #[error("kernel store key {kind}/{key:?} is invalid")]
    InvalidKey { kind: String, key: String },
    #[error("kernel store event for {kind}/{key} exceeds the size limit")]
    EventTooLarge { kind: String, key: String },
    #[error("kernel store blob of {bytes} bytes exceeds the size limit")]
    BlobTooLarge { bytes: usize },
    #[error("kernel store blob {digest} is missing or does not match its digest")]
    BlobIntegrity { digest: String },
    #[error("kernel store batch must hold one to {MAX_BATCH} appends to distinct streams")]
    InvalidBatch,
    #[error("kernel store stream {kind}/{key} failed its integrity chain at version {version}")]
    Integrity {
        kind: String,
        key: String,
        version: u64,
    },
    #[error("kernel store event could not be encoded or decoded: {0}")]
    Codec(String),
    #[error("kernel store at {path} is not a runx kernel store")]
    Foreign { path: PathBuf },
    #[error(
        "kernel store at {path} is from an earlier runx, which this one doesn't read; move it aside to start a new store"
    )]
    BeforeThisRunx { path: PathBuf },
    #[error("kernel store at {path} failed while {operation}: {source}")]
    Database {
        path: PathBuf,
        operation: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("kernel store at {path} is unavailable: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl KernelStoreError {
    fn encode(error: serde_json::Error) -> Self {
        Self::Codec(error.to_string())
    }
}

/// How durable a commit is when its append returns. A commit nothing
/// outside the process learns of before the next durable one is ordered: a
/// run's progress between the points it reaches the world and those it
/// answers its host, which are durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Durability {
    /// On the disk: SQLite syncs its log at the commit, with the full flush.
    Durable,
    /// In order: written to the log after every earlier commit, so a power
    /// cut loses it only with every later one, and durable with the next
    /// durable commit, which syncs the log through it.
    Ordered,
}

impl Durability {
    fn synchronous(self) -> &'static str {
        match self {
            Self::Durable => "FULL",
            Self::Ordered => "NORMAL",
        }
    }
}

/// The protected store for one project workspace. Open it through [`KernelStore::open`]
/// or [`KernelStore::for_project`], which share one connection per database
/// within the process while any holder keeps it open; the last holder's drop
/// closes it. A workspace is one kernel: its instances, standing
/// approvals and effect custody travel together even if receipt storage moves.
pub struct KernelStore {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for KernelStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("KernelStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn open_stores() -> &'static Mutex<HashMap<PathBuf, Weak<KernelStore>>> {
    static STORES: OnceLock<Mutex<HashMap<PathBuf, Weak<KernelStore>>>> = OnceLock::new();
    STORES.get_or_init(|| Mutex::new(HashMap::new()))
}

impl KernelStore {
    /// The store of the project directory. Receipt destinations are output
    /// configuration, not the identity of standing kernel state.
    pub fn for_project(paths: &crate::paths::RunxPaths) -> Result<Arc<Self>, KernelStoreError> {
        Self::open(paths.project())
    }

    /// Open, creating if needed, the store inside `root`.
    pub fn open(root: &Path) -> Result<Arc<Self>, KernelStoreError> {
        std::fs::create_dir_all(root).map_err(|source| KernelStoreError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let root = root.canonicalize().map_err(|source| KernelStoreError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let path = root.join(KERNEL_STORE_FILE);
        let mut stores = open_stores()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(store) = stores.get(&path).and_then(Weak::upgrade) {
            return Ok(store);
        }
        stores.retain(|_, store| store.strong_count() > 0);
        let connection = schema::open(&path)?;
        let store = Arc::new(Self {
            path: path.clone(),
            connection: Mutex::new(connection),
        });
        stores.insert(path, Arc::downgrade(&store));
        Ok(store)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The authoritative project state directory this store was opened in.
    #[must_use]
    pub(crate) fn project_dir(&self) -> PathBuf {
        let mut project = self.path.clone();
        project.pop();
        project
    }

    /// Append every event or none, durably: the commit is on the disk before
    /// this returns. Each stream must still be at the version its writer
    /// expected; any mismatch refuses the whole batch.
    pub(crate) fn append(&self, batch: Vec<Append>) -> Result<Vec<u64>, KernelStoreError> {
        self.append_as(batch, Durability::Durable)
    }

    /// Append every event or none, as `durability` says.
    pub(crate) fn append_as(
        &self,
        batch: Vec<Append>,
        durability: Durability,
    ) -> Result<Vec<u64>, KernelStoreError> {
        if batch.is_empty() || batch.len() > MAX_BATCH {
            return Err(KernelStoreError::InvalidBatch);
        }
        for (index, append) in batch.iter().enumerate() {
            if batch[..index]
                .iter()
                .any(|other| other.kind == append.kind && other.key == append.key)
            {
                return Err(KernelStoreError::InvalidBatch);
            }
        }
        let committed_at = crate::time::now_iso8601();
        let mut connection = self.lock();
        // A connection's sync level holds for each commit it makes after,
        // and can't change inside a transaction.
        connection
            .pragma_update(None, "synchronous", durability.synchronous())
            .map_err(|source| self.database("choosing the commit's durability", source))?;
        let _open = crate::work_counts::OpenTransaction::begin();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| self.database("starting a batch", source))?;
        // The batch commits after every earlier one, so its events follow
        // every committed event, in the batch's order.
        let mut sequence = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM kernel_events",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|source| self.database("reading the commit sequence", source))?
            as u64;
        let mut versions = Vec::with_capacity(batch.len());
        for append in &batch {
            let head: Option<(u64, String)> = transaction
                .query_row(
                    "SELECT version, chain FROM kernel_keys WHERE stream = ?1 AND key = ?2",
                    params![append.kind, append.key],
                    |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?)),
                )
                .optional()
                .map_err(|source| self.database("reading a stream head", source))?;
            let (actual, previous_chain) = head.unwrap_or((0, String::new()));
            if actual != append.expected.version() {
                return Err(KernelStoreError::Conflict {
                    kind: append.kind.clone(),
                    key: append.key.clone(),
                    expected: append.expected.version(),
                    actual,
                });
            }
            let version = actual + 1;
            sequence += 1;
            let chain = chain_link(&previous_chain, append.event_type, &append.data);
            transaction
                .execute(
                    "INSERT INTO kernel_events
                       (stream, key, version, event_type, data, chain, committed_at, sequence, tree)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        append.kind,
                        append.key,
                        version as i64,
                        append.event_type,
                        append.data,
                        chain,
                        committed_at,
                        sequence as i64,
                        tree_of(&append.key)
                    ],
                )
                .map_err(|source| self.database("appending an event", source))?;
            transaction
                .execute(
                    "INSERT INTO kernel_keys (stream, key, version, event_type, chain, updated_at, sequence)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT (stream, key) DO UPDATE SET
                       version = excluded.version, event_type = excluded.event_type,
                       chain = excluded.chain, updated_at = excluded.updated_at,
                       sequence = excluded.sequence",
                    params![
                        append.kind,
                        append.key,
                        version as i64,
                        append.event_type,
                        chain,
                        committed_at,
                        sequence as i64
                    ],
                )
                .map_err(|source| self.database("advancing a stream head", source))?;
            for blob in &append.blobs {
                transaction
                    .execute(
                        "INSERT INTO kernel_blobs (digest, data) VALUES (?1, ?2)
                         ON CONFLICT (digest) DO NOTHING",
                        params![blob.digest, blob.data],
                    )
                    .map_err(|source| self.database("storing a blob", source))?;
            }
            for receipt in &append.receipts {
                transaction
                    .execute(
                        "INSERT INTO kernel_receipts (receipt_id, tree, digest) VALUES (?1, ?2, ?3)
                         ON CONFLICT (receipt_id) DO NOTHING",
                        params![receipt.id, tree_of(&receipt.run_id), receipt.digest],
                    )
                    .map_err(|source| self.database("indexing a receipt", source))?;
            }
            versions.push(version);
        }
        transaction
            .commit()
            .map_err(|source| self.database("committing a batch", source))?;
        crate::work_counts::count(crate::work_counts::Work::StoreWrite);
        if durability == Durability::Durable {
            crate::work_counts::count(crate::work_counts::Work::DurableWrite);
            #[cfg(feature = "crash-points")]
            crate::crash::store_synced(&self.path);
        }
        Ok(versions)
    }

    /// Every event in a kernel stream, verified against its chain.
    pub(crate) fn read<E: StreamEvent>(
        &self,
        key: &str,
    ) -> Result<Vec<Recorded<E>>, KernelStoreError> {
        self.read_kind(E::KIND, key)
    }

    /// The current event for a hot-path projection. The head and its
    /// predecessor link are checked; use `read` to audit the whole chain.
    pub(crate) fn latest<E: StreamEvent>(
        &self,
        key: &str,
    ) -> Result<Option<Recorded<E>>, KernelStoreError> {
        self.latest_kind(E::KIND, key)
    }

    /// The event that opened a kernel stream, checked against its chain link.
    pub(crate) fn first<E: StreamEvent>(
        &self,
        key: &str,
    ) -> Result<Option<Recorded<E>>, KernelStoreError> {
        let row = self
            .lock()
            .query_row(
                "SELECT event_type, data, chain, committed_at FROM kernel_events
                 WHERE stream = ?1 AND key = ?2 AND version = 1",
                params![E::KIND, key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|source| self.database("reading the first event", source))?;
        let Some((event_type, data, chain, committed_at)) = row else {
            return Ok(None);
        };
        crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
        #[cfg(feature = "probe-fixtures")]
        crate::work_counts::stream_event_read(E::KIND, key, 1);
        let invalid = || KernelStoreError::Integrity {
            kind: E::KIND.to_owned(),
            key: key.to_owned(),
            version: 1,
        };
        if chain_link("", &event_type, &data) != chain {
            return Err(invalid());
        }
        let event: E = serde_json::from_slice(&data).map_err(KernelStoreError::encode)?;
        if event.event_type() != event_type {
            return Err(invalid());
        }
        Ok(Some(Recorded {
            version: 1,
            committed_at,
            event,
        }))
    }

    /// The current version of a kernel stream; zero when it does not exist.
    pub(crate) fn version<E: StreamEvent>(&self, key: &str) -> Result<u64, KernelStoreError> {
        self.version_of(E::KIND, key)
    }

    /// Stream heads of one kernel kind, in key order after `after`.
    pub(crate) fn heads<E: StreamEvent>(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Head>, KernelStoreError> {
        self.heads_of(E::KIND, after, limit)
    }

    /// The heads of `root`'s stream and of every stream below it, keyed
    /// `<root>/...`, in key order. A root key holds no `/`, so the range
    /// `[<root>/, <root>0)` holds exactly its descendants.
    pub(crate) fn tree_heads<E: StreamEvent>(
        &self,
        root: &str,
    ) -> Result<Vec<Head>, KernelStoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare_cached(
                "SELECT key, version, event_type, updated_at, sequence FROM kernel_keys
                 WHERE stream = ?1 AND (key = ?2 OR (key >= ?3 AND key < ?4)) ORDER BY key",
            )
            .map_err(|source| self.database("preparing a stream tree listing", source))?;
        statement
            .query_map(
                params![E::KIND, root, format!("{root}/"), format!("{root}0")],
                head_row,
            )
            .and_then(Iterator::collect)
            .map_err(|source| self.database("listing a stream tree", source))
    }

    /// The keys of `root`'s stream and the streams below it that hold an
    /// event of `event_type`, in key order.
    pub(crate) fn tree_keys_with<E: StreamEvent>(
        &self,
        root: &str,
        event_type: &str,
    ) -> Result<Vec<String>, KernelStoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare_cached(
                "SELECT DISTINCT key FROM kernel_events
                 WHERE stream = ?1 AND event_type = ?2 AND (key = ?3 OR (key >= ?4 AND key < ?5))
                 ORDER BY key",
            )
            .map_err(|source| self.database("preparing a stream tree search", source))?;
        statement
            .query_map(
                params![
                    E::KIND,
                    event_type,
                    root,
                    format!("{root}/"),
                    format!("{root}0")
                ],
                |row| {
                    crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
                    row.get::<_, String>(0)
                },
            )
            .and_then(Iterator::collect)
            .map_err(|source| self.database("searching a stream tree", source))
    }

    /// Every event of a kernel stream after version `after`, verified against
    /// its chain from there.
    pub(crate) fn read_all_after<E: StreamEvent>(
        &self,
        key: &str,
        after: u64,
    ) -> Result<Vec<Recorded<E>>, KernelStoreError> {
        self.read_range(E::KIND, key, after, None)
    }

    /// At most `limit` events of `root`'s stream tree committed after
    /// `after`, in the order they were committed, each checked against the
    /// link before it in its own stream's chain. What it reads is what the
    /// tree committed since `after`, however much the tree already holds.
    pub(crate) fn tree_after<E: StreamEvent>(
        &self,
        root: &str,
        after: u64,
        limit: usize,
    ) -> Result<Vec<TreeEvent<E>>, KernelStoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare_cached(
                "SELECT e.key, e.version, e.event_type, e.data, e.chain, e.committed_at, e.sequence,
                        (SELECT p.chain FROM kernel_events p
                         WHERE p.stream = e.stream AND p.key = e.key AND p.version = e.version - 1)
                 FROM kernel_events e
                 WHERE e.stream = ?1 AND e.tree = ?2 AND e.sequence > ?3
                 ORDER BY e.sequence LIMIT ?4",
            )
            .map_err(|source| self.database("preparing a stream tree read", source))?;
        let rows = statement
            .query_map(params![E::KIND, root, after as i64, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)? as u64,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(|source| self.database("reading a stream tree", source))?;
        let mut events = Vec::new();
        for row in rows {
            let (key, version, event_type, data, chain, committed_at, sequence, previous) =
                row.map_err(|source| self.database("reading a stream tree", source))?;
            crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
            #[cfg(feature = "probe-fixtures")]
            crate::work_counts::stream_event_read(E::KIND, &key, version);
            let integrity = || KernelStoreError::Integrity {
                kind: E::KIND.to_owned(),
                key: key.clone(),
                version,
            };
            if (version > 1 && previous.is_none())
                || chain_link(previous.as_deref().unwrap_or(""), &event_type, &data) != chain
            {
                return Err(integrity());
            }
            let event: E = serde_json::from_slice(&data).map_err(KernelStoreError::encode)?;
            if event.event_type() != event_type {
                return Err(integrity());
            }
            events.push(TreeEvent {
                key,
                sequence,
                recorded: Recorded {
                    version,
                    committed_at,
                    event,
                },
            });
        }
        Ok(events)
    }

    /// The last commit in `root`'s stream tree; zero when it holds none.
    pub(crate) fn tree_sequence<E: StreamEvent>(
        &self,
        root: &str,
    ) -> Result<u64, KernelStoreError> {
        let sequence = self
            .lock()
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM kernel_events WHERE stream = ?1 AND tree = ?2",
                params![E::KIND, root],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|source| self.database("reading a stream tree's last commit", source))?;
        crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
        Ok(sequence as u64)
    }

    /// At most `limit` events of a kernel stream after version `after`.
    pub(crate) fn read_after<E: StreamEvent>(
        &self,
        key: &str,
        after: u64,
        limit: usize,
    ) -> Result<Vec<Recorded<E>>, KernelStoreError> {
        self.read_range(E::KIND, key, after, Some(limit))
    }

    /// Stream heads of one kernel kind, most recently advanced first by the
    /// store's commit order, strictly before the `before` head when one is
    /// given. Heads advanced in one instant still page once each.
    pub(crate) fn recent_heads<E: StreamEvent>(
        &self,
        before: Option<&Head>,
        limit: usize,
    ) -> Result<Vec<Head>, KernelStoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare_cached(
                "SELECT key, version, event_type, updated_at, sequence FROM kernel_keys
                 WHERE stream = ?1 AND (?2 IS NULL OR sequence < ?2)
                 ORDER BY sequence DESC LIMIT ?3",
            )
            .map_err(|source| self.database("preparing a recent head listing", source))?;
        statement
            .query_map(
                params![
                    E::KIND,
                    before.map(|head| head.sequence as i64),
                    limit.min(1000) as i64
                ],
                head_row,
            )
            .and_then(Iterator::collect)
            .map_err(|source| self.database("listing recent stream heads", source))
    }

    /// The bytes committed under `digest`, verified against it.
    pub(crate) fn blob(&self, digest: &str) -> Result<Vec<u8>, KernelStoreError> {
        let data = self
            .lock()
            .query_row(
                "SELECT data FROM kernel_blobs WHERE digest = ?1",
                params![digest],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(|source| self.database("reading a blob", source))?;
        crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
        match data {
            Some(data) if runx_contracts::sha256_prefixed(&data) == digest => Ok(data),
            _ => Err(KernelStoreError::BlobIntegrity {
                digest: digest.to_owned(),
            }),
        }
    }

    /// Every receipt the record holds of the run tree the receipt `id`
    /// belongs to, by id with the blob that holds it; none when the record
    /// holds no receipt by that id.
    pub(crate) fn receipts_beside(
        &self,
        id: &str,
    ) -> Result<Vec<(String, String)>, KernelStoreError> {
        let connection = self.lock();
        let tree = connection
            .query_row(
                "SELECT tree FROM kernel_receipts WHERE receipt_id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|source| self.database("finding a receipt", source))?;
        crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
        let Some(tree) = tree else {
            return Ok(Vec::new());
        };
        let mut statement = connection
            .prepare("SELECT receipt_id, digest FROM kernel_receipts WHERE tree = ?1 ORDER BY receipt_id")
            .map_err(|source| self.database("listing a run's receipts", source))?;
        let rows = statement
            .query_map(params![tree], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|source| self.database("listing a run's receipts", source))?;
        let mut receipts = Vec::new();
        for row in rows {
            receipts.push(row.map_err(|source| self.database("listing a run's receipts", source))?);
            crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
        }
        Ok(receipts)
    }

    /// A blob decoded as JSON.
    pub(crate) fn blob_json<T: DeserializeOwned>(
        &self,
        digest: &str,
    ) -> Result<T, KernelStoreError> {
        serde_json::from_slice(&self.blob(digest)?).map_err(KernelStoreError::encode)
    }

    fn read_kind<E: StreamEvent>(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<Vec<Recorded<E>>, KernelStoreError> {
        self.read_range(kind, key, 0, None)
    }

    /// The events of a stream after version `after`, at most `limit`, each
    /// checked against the chain from the event at `after`, or from the
    /// stream's start. A ranged read trusts its anchor's stored link, as
    /// `latest` trusts its head's; reading from the start audits the chain.
    fn read_range<E: StreamEvent>(
        &self,
        kind: &str,
        key: &str,
        after: u64,
        limit: Option<usize>,
    ) -> Result<Vec<Recorded<E>>, KernelStoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare_cached(
                "SELECT version, event_type, data, chain, committed_at FROM kernel_events
                 WHERE stream = ?1 AND key = ?2 AND version >= ?3 ORDER BY version LIMIT ?4",
            )
            .map_err(|source| self.database("preparing a stream read", source))?;
        let anchored = after > 0;
        // SQLite reads a negative limit as none; an anchored read also
        // returns its anchor.
        let limit = limit.map_or(-1, |limit| (limit + usize::from(anchored)) as i64);
        let rows = statement
            .query_map(params![kind, key, after.max(1) as i64, limit], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|source| self.database("reading a stream", source))?;
        let integrity = |version| KernelStoreError::Integrity {
            kind: kind.to_owned(),
            key: key.to_owned(),
            version,
        };
        let mut chain = String::new();
        let mut expected = after.max(1);
        let mut events = Vec::new();
        for row in rows {
            let (version, event_type, data, stored_chain, committed_at) =
                row.map_err(|source| self.database("reading a stream", source))?;
            crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
            if version != expected {
                return Err(integrity(version));
            }
            expected += 1;
            // The anchor of a ranged read is checked, never read again.
            if !(anchored && version == after) {
                #[cfg(feature = "probe-fixtures")]
                crate::work_counts::stream_event_read(kind, key, version);
                if chain_link(&chain, &event_type, &data) != stored_chain {
                    return Err(integrity(version));
                }
                let event: E = serde_json::from_slice(&data).map_err(KernelStoreError::encode)?;
                if event.event_type() != event_type {
                    return Err(integrity(version));
                }
                events.push(Recorded {
                    version,
                    committed_at,
                    event,
                });
            }
            chain = stored_chain;
        }
        if anchored && expected == after {
            return Err(integrity(after));
        }
        Ok(events)
    }

    fn latest_kind<E: StreamEvent>(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<Option<Recorded<E>>, KernelStoreError> {
        let connection = self.lock();
        latest_kind_on(&connection, &self.path, kind, key)
    }

    fn version_of(&self, kind: &str, key: &str) -> Result<u64, KernelStoreError> {
        self.lock()
            .query_row(
                "SELECT version FROM kernel_keys WHERE stream = ?1 AND key = ?2",
                params![kind, key],
                |row| {
                    crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
                    row.get::<_, i64>(0)
                },
            )
            .optional()
            .map(|version| version.map_or(0, |version| version as u64))
            .map_err(|source| self.database("reading a stream version", source))
    }

    fn heads_of(
        &self,
        kind: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Head>, KernelStoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare_cached(
                "SELECT key, version, event_type, updated_at, sequence FROM kernel_keys
                 WHERE stream = ?1 AND key > ?2 ORDER BY key LIMIT ?3",
            )
            .map_err(|source| self.database("preparing a head listing", source))?;
        statement
            .query_map(
                params![kind, after.unwrap_or(""), limit.min(1000) as i64],
                head_row,
            )
            .and_then(Iterator::collect)
            .map_err(|source| self.database("listing stream heads", source))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn database(&self, operation: &'static str, source: rusqlite::Error) -> KernelStoreError {
        KernelStoreError::Database {
            path: self.path.clone(),
            operation,
            source,
        }
    }
}

fn head_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Head> {
    crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
    Ok(Head {
        key: row.get(0)?,
        version: row.get::<_, i64>(1)? as u64,
        event_type: row.get(2)?,
        updated_at: row.get(3)?,
        sequence: row.get::<_, i64>(4)? as u64,
    })
}

/// The root key of `key`'s stream tree: its key up to the first `/`.
fn tree_of(key: &str) -> &str {
    key.split_once('/').map_or(key, |(root, _)| root)
}

fn latest_kind_on<E: StreamEvent>(
    connection: &Connection,
    path: &Path,
    kind: &str,
    key: &str,
) -> Result<Option<Recorded<E>>, KernelStoreError> {
    let row = connection
        .query_row(
            "SELECT s.version, s.event_type, s.chain, e.event_type, e.data, e.chain, e.committed_at,
                    (SELECT p.chain FROM kernel_events p
                     WHERE p.stream = s.stream AND p.key = s.key AND p.version = s.version - 1)
             FROM kernel_keys s
             LEFT JOIN kernel_events e ON e.stream = s.stream AND e.key = s.key
                  AND e.version = s.version
             WHERE s.stream = ?1 AND s.key = ?2",
            params![kind, key],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|source| KernelStoreError::Database {
            path: path.to_path_buf(),
            operation: "reading the latest event",
            source,
        })?;
    let Some((
        version,
        event_type,
        head_chain,
        stored_type,
        data,
        event_chain,
        committed_at,
        previous,
    )) = row
    else {
        return Ok(None);
    };
    crate::work_counts::count(crate::work_counts::Work::StoreRowRead);
    #[cfg(feature = "probe-fixtures")]
    crate::work_counts::stream_event_read(kind, key, version);
    let invalid = || KernelStoreError::Integrity {
        kind: kind.to_owned(),
        key: key.to_owned(),
        version,
    };
    let (Some(data), Some(event_chain), Some(committed_at)) = (data, event_chain, committed_at)
    else {
        return Err(invalid());
    };
    if version > 1 && previous.is_none() {
        return Err(invalid());
    }
    if stored_type.as_deref() != Some(event_type.as_str())
        || event_chain != head_chain
        || chain_link(previous.as_deref().unwrap_or(""), &event_type, &data) != head_chain
    {
        return Err(invalid());
    }
    let event: E = serde_json::from_slice(&data).map_err(KernelStoreError::encode)?;
    if event.event_type() != event_type {
        return Err(invalid());
    }
    Ok(Some(Recorded {
        version,
        committed_at,
        event,
    }))
}

/// `sha256(previous || 0 || type || 0 || data)`, hex.
fn chain_link(previous: &str, event_type: &str, data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(previous.as_bytes());
    hasher.update([0]);
    hasher.update(event_type.as_bytes());
    hasher.update([0]);
    hasher.update(data);
    runx_contracts::hex_lower(&hasher.finalize())
}

#[cfg(test)]
mod tests;
