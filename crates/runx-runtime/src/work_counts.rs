//! The work a runx process does, counted: what performance.md's journeys
//! budget structurally, measured by counting and never by timing. Counting is
//! a relaxed atomic add, always on; only which package identities were
//! verified is kept, in a map, and only while a probe asks for the counts. A
//! process writes its counts at exit only when `RUNX_PERF_COUNTS_PATH` names
//! a file, which only a probe sets; a workspace's `.env` can't set a `RUNX_`
//! name (`services/env.rs`).
//!
//! The counts are the process's, so a test reads them in a process that runs
//! that test alone ([`alone`]).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

/// The variable naming the file a process writes its counts to at exit.
pub const RUNX_PERF_COUNTS_PATH_ENV: &str = "RUNX_PERF_COUNTS_PATH";

/// The counted work.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Work {
    /// A child process the runtime spawned.
    ProcessSpawn,
    /// A request sent over the network.
    NetworkRequest,
    /// A package's source assembled for use: read from disk, or from a
    /// registry copy's fields.
    PackageRead,
    /// A write transaction the kernel store committed.
    StoreWrite,
    /// A write made durable before anything relies on it: a store's commit
    /// synced to the disk, or a file synced where it lies.
    DurableWrite,
    /// An operation dispatched to its provider.
    ProviderCall,
    /// A run taken whole: its record replayed from its stream, or every one
    /// of its calls sealed into its receipt.
    RunMaterialization,
    /// A row the kernel store read: an event, a stream's head or a blob.
    StoreRowRead,
    /// An entry of the receipt directory listed.
    ReceiptDirectoryEntryRead,
    /// An entry of the receipt index read back.
    ReceiptIndexEntryRead,
    /// An entry written to the receipt index, by an append or a rebuild.
    ReceiptIndexEntryWritten,
    /// A row a key listing of the `private` store visited.
    StoreKeyRowVisited,
    /// A row a key listing visited and passed over: its newest event of a
    /// type it wasn't asked for, or ended before the date it lists at.
    StoreKeyRowPassedOver,
    /// One page of a stream's search in the `private` store.
    StoreSearchPage,
    /// One event read from a key's history in the `private` store.
    StoreEventRowRead,
    /// A script run from its beginning on a worker.
    ScriptPass,
    /// A settlement handed to a script, recorded or fresh.
    ScriptSettlementDelivered,
}

static PROCESS_SPAWNS: AtomicU64 = AtomicU64::new(0);
static NETWORK_REQUESTS: AtomicU64 = AtomicU64::new(0);
static PACKAGES_READ: AtomicU64 = AtomicU64::new(0);
static PACKAGE_VERIFICATIONS: AtomicU64 = AtomicU64::new(0);
static STORE_WRITES: AtomicU64 = AtomicU64::new(0);
static DURABLE_WRITES: AtomicU64 = AtomicU64::new(0);
static PROVIDER_CALLS: AtomicU64 = AtomicU64::new(0);
static TRANSACTIONS_SPANNING_PROVIDER_CALLS: AtomicU64 = AtomicU64::new(0);
static RUN_MATERIALIZATIONS: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_BYTES_ADMITTED: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_STORE_BYTES_READ: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_BYTES_TO_WORKER: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_BYTES_TO_RESULT: AtomicU64 = AtomicU64::new(0);
static STORE_ROWS_READ: AtomicU64 = AtomicU64::new(0);
static RECEIPT_DIRECTORY_ENTRIES_READ: AtomicU64 = AtomicU64::new(0);
static RECEIPT_INDEX_ENTRIES_READ: AtomicU64 = AtomicU64::new(0);
static RECEIPT_INDEX_ENTRIES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static STORE_KEY_ROWS_VISITED: AtomicU64 = AtomicU64::new(0);
static STORE_KEY_ROWS_PASSED_OVER: AtomicU64 = AtomicU64::new(0);
static STORE_SEARCH_PAGES: AtomicU64 = AtomicU64::new(0);
static STORE_EVENT_ROWS_READ: AtomicU64 = AtomicU64::new(0);
static SCRIPT_PASSES: AtomicU64 = AtomicU64::new(0);
static SCRIPT_SETTLEMENTS_DELIVERED: AtomicU64 = AtomicU64::new(0);

/// The points an artifact's bytes are copied at, from the store to a worker
/// and to a result.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ArtifactCopy {
    /// A source's bytes copied into the artifact store.
    Admitted,
    /// Bytes read from the artifact store: a snapshot's chunks, and the
    /// manifest that names them.
    StoreRead,
    /// An artifact's bytes handed to a worker in a page.
    ToWorker,
    /// An artifact's bytes in a page a read returns.
    ToResult,
}

/// Count `bytes` an artifact copy point copied.
pub(crate) fn count_artifact_bytes(copy: ArtifactCopy, bytes: u64) {
    let counter = match copy {
        ArtifactCopy::Admitted => &ARTIFACT_BYTES_ADMITTED,
        ArtifactCopy::StoreRead => &ARTIFACT_STORE_BYTES_READ,
        ArtifactCopy::ToWorker => &ARTIFACT_BYTES_TO_WORKER,
        ArtifactCopy::ToResult => &ARTIFACT_BYTES_TO_RESULT,
    };
    counter.fetch_add(bytes, Ordering::Relaxed);
}

/// Each package identity verified, with how many times.
type VerifiedIdentities = Mutex<BTreeMap<[u8; 32], u64>>;

/// The identities verified, kept while a probe asks for the counts, so a
/// long-lived process holds nothing it won't write.
static VERIFIED_IDENTITIES: OnceLock<Option<VerifiedIdentities>> = OnceLock::new();

thread_local! {
    /// The store transactions this thread holds open. A transaction is
    /// synchronous, so the thread that opens one is the thread that holds it.
    static OPEN_TRANSACTIONS: Cell<u32> = const { Cell::new(0) };
}

/// Count one unit of `work`.
pub(crate) fn count(work: Work) {
    count_many(work, 1);
}

/// Count `units` units of `work`.
pub(crate) fn count_many(work: Work, units: usize) {
    let counter = match work {
        Work::ProcessSpawn => &PROCESS_SPAWNS,
        Work::NetworkRequest => &NETWORK_REQUESTS,
        Work::PackageRead => &PACKAGES_READ,
        Work::StoreWrite => &STORE_WRITES,
        Work::DurableWrite => &DURABLE_WRITES,
        Work::ProviderCall => &PROVIDER_CALLS,
        Work::RunMaterialization => &RUN_MATERIALIZATIONS,
        Work::StoreRowRead => &STORE_ROWS_READ,
        Work::ReceiptDirectoryEntryRead => &RECEIPT_DIRECTORY_ENTRIES_READ,
        Work::ReceiptIndexEntryRead => &RECEIPT_INDEX_ENTRIES_READ,
        Work::ReceiptIndexEntryWritten => &RECEIPT_INDEX_ENTRIES_WRITTEN,
        Work::StoreKeyRowVisited => &STORE_KEY_ROWS_VISITED,
        Work::StoreKeyRowPassedOver => &STORE_KEY_ROWS_PASSED_OVER,
        Work::StoreSearchPage => &STORE_SEARCH_PAGES,
        Work::StoreEventRowRead => &STORE_EVENT_ROWS_READ,
        Work::ScriptPass => &SCRIPT_PASSES,
        Work::ScriptSettlementDelivered => &SCRIPT_SETTLEMENTS_DELIVERED,
    };
    counter.fetch_add(u64::try_from(units).unwrap_or(u64::MAX), Ordering::Relaxed);
}

/// Count a spawn that succeeded, and pass its result on.
pub(crate) fn counted_spawn<T>(spawned: std::io::Result<T>) -> std::io::Result<T> {
    if spawned.is_ok() {
        count(Work::ProcessSpawn);
    }
    spawned
}

/// Count one verification of the package whose source digests to `identity`.
pub(crate) fn package_verified(identity: [u8; 32]) {
    PACKAGE_VERIFICATIONS.fetch_add(1, Ordering::Relaxed);
    let tracked = VERIFIED_IDENTITIES.get_or_init(|| {
        std::env::var_os(RUNX_PERF_COUNTS_PATH_ENV).map(|_| Mutex::new(BTreeMap::new()))
    });
    if let Some(identities) = tracked {
        let mut identities = identities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *identities.entry(identity).or_insert(0) += 1;
    }
}

/// A store transaction this thread holds open, from `begin` until it drops.
#[must_use = "a transaction is held open only while its guard lives"]
pub(crate) struct OpenTransaction(());

impl OpenTransaction {
    pub(crate) fn begin() -> Self {
        OPEN_TRANSACTIONS.with(|open| open.set(open.get().saturating_add(1)));
        Self(())
    }
}

impl Drop for OpenTransaction {
    fn drop(&mut self) {
        OPEN_TRANSACTIONS.with(|open| open.set(open.get().saturating_sub(1)));
    }
}

fn in_a_transaction() -> bool {
    OPEN_TRANSACTIONS.with(Cell::get) > 0
}

/// Dispatch one operation to its provider through `call`, counting it, and
/// counting it again as spanning a transaction when this thread held a store
/// transaction open as it began or as it returned.
pub(crate) fn provider_call<T>(call: impl FnOnce() -> T) -> T {
    count(Work::ProviderCall);
    let open_before = in_a_transaction();
    let returned = call();
    if open_before || in_a_transaction() {
        TRANSACTIONS_SPANNING_PROVIDER_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    returned
}

/// What this process has counted so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct WorkCounts {
    /// Child processes the runtime spawned: workers, process tools and
    /// detached supervisors.
    pub process_spawns: u64,
    /// Requests sent over the network, each attempt counted.
    pub network_requests: u64,
    /// Package sources assembled for use, each assembly counted.
    pub packages_read: u64,
    /// Package verifications, each one counted.
    pub package_verifications: u64,
    /// The most times any one package identity was verified, while a probe
    /// asks for the counts; otherwise 0.
    pub most_verifications_of_one_package: u64,
    /// Write transactions the kernel store committed.
    pub store_writes: u64,
    /// Writes made durable before anything relied on them: store commits
    /// synced to the disk, and files synced where they lie.
    pub durable_writes: u64,
    /// Operations dispatched to their providers.
    pub provider_calls: u64,
    /// Provider calls a store transaction was held open across.
    pub transactions_spanning_provider_calls: u64,
    /// Runs taken whole: a record replayed from its stream, or every call
    /// sealed into a receipt.
    pub run_materializations: u64,
    /// Bytes copied into the artifact store from their sources.
    pub artifact_bytes_admitted: u64,
    /// Bytes read from the artifact store: chunks and manifests.
    pub artifact_store_bytes_read: u64,
    /// Artifact bytes handed to workers in pages.
    pub artifact_bytes_to_worker: u64,
    /// Artifact bytes in the pages reads returned.
    pub artifact_bytes_to_result: u64,
    /// Rows the kernel store read: events, stream heads and blobs.
    pub store_rows_read: u64,
    /// Entries of the receipt directory listed, which only an index rebuild
    /// and its coverage check do.
    pub receipt_directory_entries_read: u64,
    /// Entries of the receipt index read back.
    pub receipt_index_entries_read: u64,
    /// Entries written to the receipt index.
    pub receipt_index_entries_written: u64,
    /// Rows the `private` store's key listings visited, as SQLite's recency
    /// index stepped them.
    pub store_key_rows_visited: u64,
    /// Of those, the rows a listing passed over: their newest event of a type
    /// it wasn't asked for, or ended before the date it lists at.
    pub store_key_rows_passed_over: u64,
    /// Pages of the `private` store's stream searches.
    pub store_search_pages: u64,
    /// Events read from keys' histories in the `private` store.
    pub store_event_rows_read: u64,
    /// Scripts run from their beginning on a worker: one for a script a live
    /// worker runs to its end, and one more each time a lost worker is
    /// rebuilt from the record.
    pub script_passes: u64,
    /// Settlements handed to scripts, recorded ones replayed included.
    pub script_settlements_delivered: u64,
}

#[must_use]
pub fn snapshot() -> WorkCounts {
    let most_verifications_of_one_package = VERIFIED_IDENTITIES
        .get()
        .and_then(Option::as_ref)
        .and_then(|identities| {
            identities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .max()
                .copied()
        })
        .unwrap_or(0);
    WorkCounts {
        process_spawns: PROCESS_SPAWNS.load(Ordering::Relaxed),
        network_requests: NETWORK_REQUESTS.load(Ordering::Relaxed),
        packages_read: PACKAGES_READ.load(Ordering::Relaxed),
        package_verifications: PACKAGE_VERIFICATIONS.load(Ordering::Relaxed),
        most_verifications_of_one_package,
        store_writes: STORE_WRITES.load(Ordering::Relaxed),
        durable_writes: DURABLE_WRITES.load(Ordering::Relaxed),
        provider_calls: PROVIDER_CALLS.load(Ordering::Relaxed),
        transactions_spanning_provider_calls: TRANSACTIONS_SPANNING_PROVIDER_CALLS
            .load(Ordering::Relaxed),
        run_materializations: RUN_MATERIALIZATIONS.load(Ordering::Relaxed),
        artifact_bytes_admitted: ARTIFACT_BYTES_ADMITTED.load(Ordering::Relaxed),
        artifact_store_bytes_read: ARTIFACT_STORE_BYTES_READ.load(Ordering::Relaxed),
        artifact_bytes_to_worker: ARTIFACT_BYTES_TO_WORKER.load(Ordering::Relaxed),
        artifact_bytes_to_result: ARTIFACT_BYTES_TO_RESULT.load(Ordering::Relaxed),
        store_rows_read: STORE_ROWS_READ.load(Ordering::Relaxed),
        receipt_directory_entries_read: RECEIPT_DIRECTORY_ENTRIES_READ.load(Ordering::Relaxed),
        receipt_index_entries_read: RECEIPT_INDEX_ENTRIES_READ.load(Ordering::Relaxed),
        receipt_index_entries_written: RECEIPT_INDEX_ENTRIES_WRITTEN.load(Ordering::Relaxed),
        store_key_rows_visited: STORE_KEY_ROWS_VISITED.load(Ordering::Relaxed),
        store_key_rows_passed_over: STORE_KEY_ROWS_PASSED_OVER.load(Ordering::Relaxed),
        store_search_pages: STORE_SEARCH_PAGES.load(Ordering::Relaxed),
        store_event_rows_read: STORE_EVENT_ROWS_READ.load(Ordering::Relaxed),
        script_passes: SCRIPT_PASSES.load(Ordering::Relaxed),
        script_settlements_delivered: SCRIPT_SETTLEMENTS_DELIVERED.load(Ordering::Relaxed),
    }
}

/// The reads of each kernel stream's events, by `<kind> <key>` and version,
/// while a probe tallies them, or for the whole of a process that writes its
/// counts; none otherwise. Only the benches and the CLI's test build have it.
#[cfg(feature = "probe-fixtures")]
static STREAM_READS: Mutex<Option<BTreeMap<String, BTreeMap<u64, u64>>>> = Mutex::new(None);

/// Count one read of event `version` of the kernel stream `key` of `kind`,
/// while a probe tallies them or the process writes its counts.
#[cfg(feature = "probe-fixtures")]
pub(crate) fn stream_event_read(kind: &str, key: &str, version: u64) {
    let mut reads = STREAM_READS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if reads.is_none() && std::env::var_os(RUNX_PERF_COUNTS_PATH_ENV).is_some() {
        *reads = Some(BTreeMap::new());
    }
    if let Some(reads) = reads.as_mut() {
        *reads
            .entry(format!("{kind} {key}"))
            .or_default()
            .entry(version)
            .or_default() += 1;
    }
}

/// What `measure` returned, and how many times it read each event of each
/// kernel stream, by `<kind> <key>` and version.
#[cfg(feature = "probe-fixtures")]
pub(crate) fn stream_reads<T>(
    measure: impl FnOnce() -> T,
) -> (T, BTreeMap<String, BTreeMap<u64, u64>>) {
    let tally = || {
        STREAM_READS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    *tally() = Some(BTreeMap::new());
    let measured = measure();
    let reads = tally().take().unwrap_or_default();
    (measured, reads)
}

/// Write this process's counts to the file `RUNX_PERF_COUNTS_PATH` names,
/// when it names one. A build that tallies stream reads adds the most times
/// it read any one kernel stream event, 1 where it read each stream once, and
/// how many events it read more than once.
///
/// # Errors
///
/// Returns why the file couldn't be written.
pub fn record_from_process_env() -> std::io::Result<()> {
    let Some(path) = std::env::var_os(RUNX_PERF_COUNTS_PATH_ENV) else {
        return Ok(());
    };
    #[cfg_attr(not(feature = "probe-fixtures"), allow(unused_mut))]
    let mut counts = serde_json::to_value(snapshot()).map_err(std::io::Error::other)?;
    #[cfg(feature = "probe-fixtures")]
    if let serde_json::Value::Object(counts) = &mut counts {
        let reads = STREAM_READS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|reads| {
                reads
                    .values()
                    .flat_map(BTreeMap::values)
                    .copied()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        counts.insert(
            "most_reads_of_one_stream_event".to_owned(),
            serde_json::Value::from(reads.iter().copied().max().unwrap_or(0)),
        );
        counts.insert(
            "stream_events_read_again".to_owned(),
            serde_json::Value::from(reads.iter().filter(|read| **read > 1).count()),
        );
    }
    let counts = serde_json::to_vec(&counts).map_err(std::io::Error::other)?;
    std::fs::write(path, counts)
}

/// The variable naming the one test a process of a test binary runs alone.
#[cfg(test)]
const ALONE_ENV: &str = "RUNX_WORK_COUNTS_ALONE";

/// The variable naming the file a test run alone writes what it measured to.
#[cfg(test)]
const ALONE_PATH_ENV: &str = "RUNX_WORK_COUNTS_ALONE_PATH";

/// What `measure` returns, measured in a process of this test binary that
/// runs only the test at `test`, as `module_path!()` and its name spell it,
/// so no test running beside it moves the counts `measure` reads. The process
/// that runs it alone runs `measure` and writes what it returned where the
/// one that launched it reads it back.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) fn alone<T>(test: &str, measure: impl FnOnce() -> T) -> T
where
    T: Serialize + serde::de::DeserializeOwned,
{
    let name = test.split_once("::").map_or(test, |(_, name)| name);
    if std::env::var(ALONE_ENV).as_deref() == Ok(name) {
        let measured = measure();
        let path = std::env::var_os(ALONE_PATH_ENV).expect("where to write what it measured");
        std::fs::write(
            path,
            serde_json::to_vec(&measured).expect("what a test measured is JSON"),
        )
        .expect("writing what a test measured");
        return measured;
    }
    let directory = tempfile::tempdir().expect("a directory for what a test measures");
    let path = directory.path().join("measured.json");
    let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", name, "--test-threads=1"])
        .env(ALONE_ENV, name)
        .env(ALONE_PATH_ENV, &path)
        .output()
        .expect("running a test alone");
    assert!(
        output.status.success(),
        "{name}, run alone:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&std::fs::read(&path).expect("what a test run alone measured"))
        .expect("what a test measured reads back")
}

#[cfg(test)]
mod tests {
    use super::{OpenTransaction, provider_call, snapshot};

    /// A provider call made while this thread holds a store transaction is
    /// counted as spanning it; one made after the transaction closes is not.
    /// Only the calling thread's own transactions decide it, so a test
    /// running beside it moves the count only by calling a provider inside a
    /// transaction of its own.
    #[test]
    fn a_provider_call_inside_an_open_transaction_is_counted_as_spanning_it() {
        let before = snapshot().transactions_spanning_provider_calls;
        {
            let _open = OpenTransaction::begin();
            provider_call(|| ());
        }
        provider_call(|| ());
        assert_eq!(snapshot().transactions_spanning_provider_calls, before + 1);
    }
}
