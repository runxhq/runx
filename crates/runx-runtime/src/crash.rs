//! Crash points, for the crash tests alone: compiled only with the
//! `crash-points` feature, which only a test build enables. A point aborts
//! the process where it stands, with no unwinding, no destructors and no
//! writes of its own, so recovery reads exactly what a kill there leaves.
//!
//! `RUNX_CRASH_AT=<boundary>@<call>` chooses the one point that aborts, where
//! `<call>` is a call's address in its run, such as `prepare/1`, or a fixed
//! step's, such as `read`.

/// The environment variable that names the point to abort at.
const CRASH_AT: &str = "RUNX_CRASH_AT";

/// Abort if `RUNX_CRASH_AT` names `boundary` of `call`.
pub(crate) fn at(boundary: &str, call: &str) {
    let Ok(named) = std::env::var(CRASH_AT) else {
        return;
    };
    if named.split_once('@') == Some((boundary, call)) {
        std::process::abort();
    }
}
