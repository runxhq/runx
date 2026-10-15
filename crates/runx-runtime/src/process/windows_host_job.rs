use std::io;
use std::sync::OnceLock;

use win32job::{ExtendedLimitInfo, Job};

static HOST_JOB: OnceLock<Result<Job, String>> = OnceLock::new();

/// Place the Runx host in a kill-on-close Job Object before it spawns adapters.
///
/// Per-execution jobs own normal timeout/cancellation. This outer job is the
/// crash boundary: Windows closes its handle if Runx is killed or aborts, and
/// the kernel then terminates every still-nested adapter process.
pub(crate) fn ensure_windows_host_job() -> io::Result<()> {
    match HOST_JOB.get_or_init(create) {
        Ok(_) => Ok(()),
        Err(message) => Err(io::Error::other(message.clone())),
    }
}

fn create() -> Result<Job, String> {
    let mut limits = ExtendedLimitInfo::new();
    // A detached process's supervisor breaks away, so it outlives the host.
    limits.limit_kill_on_job_close().limit_breakaway_ok();
    let job = Job::create_with_limit_info(&limits)
        .map_err(|error| format!("creating Runx host Job Object: {error}"))?;
    job.assign_current_process()
        .map_err(|error| format!("assigning Runx to host Job Object: {error}"))?;
    Ok(job)
}
