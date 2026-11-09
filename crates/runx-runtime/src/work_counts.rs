//! The work a runx process does, counted: what performance.md's journeys
//! budget structurally, measured by counting and never by timing. Counting is
//! a relaxed atomic add, always on. A process writes its counts at exit only
//! when `RUNX_PERF_COUNTS_PATH` names a file, which only a probe sets; a
//! workspace's `.env` can't set a `RUNX_` name (`services/env.rs`).

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// The variable naming the file a process writes its counts to at exit.
pub const RUNX_PERF_COUNTS_PATH_ENV: &str = "RUNX_PERF_COUNTS_PATH";

/// The counted work.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Work {
    /// A child process the runtime spawned.
    ProcessSpawn,
}

static PROCESS_SPAWNS: AtomicU64 = AtomicU64::new(0);

/// Count one unit of `work`.
pub(crate) fn count(work: Work) {
    let counter = match work {
        Work::ProcessSpawn => &PROCESS_SPAWNS,
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

/// What this process has counted so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct WorkCounts {
    /// Child processes the runtime spawned: workers, process tools and
    /// detached supervisors.
    pub process_spawns: u64,
}

#[must_use]
pub fn snapshot() -> WorkCounts {
    WorkCounts {
        process_spawns: PROCESS_SPAWNS.load(Ordering::Relaxed),
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
