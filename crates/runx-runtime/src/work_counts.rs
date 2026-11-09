//! The work a runx process does, counted: what performance.md's journeys
//! budget structurally, measured by counting and never by timing. Counting is
//! a relaxed atomic add, always on; only which package identities were
//! verified is kept, in a map, and only while a probe asks for the counts. A
//! process writes its counts at exit only when `RUNX_PERF_COUNTS_PATH` names
//! a file, which only a probe sets; a workspace's `.env` can't set a `RUNX_`
//! name (`services/env.rs`).

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
    /// An operation dispatched to its provider.
    ProviderCall,
    /// A run taken whole: its record replayed from its stream, or every one
    /// of its calls sealed into its receipt.
    RunMaterialization,
}

static PROCESS_SPAWNS: AtomicU64 = AtomicU64::new(0);
static NETWORK_REQUESTS: AtomicU64 = AtomicU64::new(0);
static PACKAGES_READ: AtomicU64 = AtomicU64::new(0);
static PACKAGE_VERIFICATIONS: AtomicU64 = AtomicU64::new(0);
static STORE_WRITES: AtomicU64 = AtomicU64::new(0);
static PROVIDER_CALLS: AtomicU64 = AtomicU64::new(0);
static TRANSACTIONS_SPANNING_PROVIDER_CALLS: AtomicU64 = AtomicU64::new(0);
static RUN_MATERIALIZATIONS: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_BYTES_ADMITTED: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_STORE_BYTES_READ: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_BYTES_TO_WORKER: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_BYTES_TO_RESULT: AtomicU64 = AtomicU64::new(0);

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
    let counter = match work {
        Work::ProcessSpawn => &PROCESS_SPAWNS,
        Work::NetworkRequest => &NETWORK_REQUESTS,
        Work::PackageRead => &PACKAGES_READ,
        Work::StoreWrite => &STORE_WRITES,
        Work::ProviderCall => &PROVIDER_CALLS,
        Work::RunMaterialization => &RUN_MATERIALIZATIONS,
    };
    counter.fetch_add(1, Ordering::Relaxed);
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
        provider_calls: PROVIDER_CALLS.load(Ordering::Relaxed),
        transactions_spanning_provider_calls: TRANSACTIONS_SPANNING_PROVIDER_CALLS
            .load(Ordering::Relaxed),
        run_materializations: RUN_MATERIALIZATIONS.load(Ordering::Relaxed),
        artifact_bytes_admitted: ARTIFACT_BYTES_ADMITTED.load(Ordering::Relaxed),
        artifact_store_bytes_read: ARTIFACT_STORE_BYTES_READ.load(Ordering::Relaxed),
        artifact_bytes_to_worker: ARTIFACT_BYTES_TO_WORKER.load(Ordering::Relaxed),
        artifact_bytes_to_result: ARTIFACT_BYTES_TO_RESULT.load(Ordering::Relaxed),
    }
}

/// Write this process's counts to the file `RUNX_PERF_COUNTS_PATH` names,
/// when it names one.
///
/// # Errors
///
/// Returns why the file couldn't be written.
pub fn record_from_process_env() -> std::io::Result<()> {
    let Some(path) = std::env::var_os(RUNX_PERF_COUNTS_PATH_ENV) else {
        return Ok(());
    };
    let counts = serde_json::to_vec(&snapshot()).map_err(std::io::Error::other)?;
    std::fs::write(path, counts)
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
