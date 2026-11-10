//! Crash points, for the crash tests alone: compiled only with the
//! `crash-points` feature, which only a test build enables. A point aborts
//! the process where it stands, with no unwinding, no destructors and no
//! writes of its own, so recovery reads exactly what a kill there leaves.
//!
//! `RUNX_CRASH_AT=<boundary>@<call>` chooses the one point that aborts, where
//! `<call>` is a call's address in its run, such as `prepare/1`, or a fixed
//! step's, such as `read`.
//!
//! The same build says when a run reaches a point a concurrency test waits
//! on, such as waiting on the workspace's lock: `RUNX_REACHED=<point>@<pipe>`
//! names the point and the named pipe it's said on.

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

/// The environment variable that names the point to say is reached, and the
/// pipe to say it on.
const REACHED: &str = "RUNX_REACHED";

/// Say on its pipe that the run reached `point`, if `RUNX_REACHED` names it:
/// the write waits until the test opens the pipe to read it.
pub(crate) fn reached(point: &str) {
    let Ok(named) = std::env::var(REACHED) else {
        return;
    };
    if let Some((named, pipe)) = named.split_once('@')
        && named == point
    {
        let _said = std::fs::OpenOptions::new()
            .write(true)
            .open(pipe)
            .and_then(|mut pipe| std::io::Write::write_all(&mut pipe, b"x"));
    }
}
