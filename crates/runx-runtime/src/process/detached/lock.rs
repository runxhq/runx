//! The lock a supervisor holds for its process's life. The OS releases it
//! however the supervisor ends, so a process whose lock is free has no
//! supervisor.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

const LOCK: &str = "lock";

/// A reader that checks the lock holds it for an instant; a supervisor
/// starting then waits this long for it.
const CONTENDED: Duration = Duration::from_secs(1);

/// Hold the lock of the process in `directory`; none while another
/// supervisor holds it.
pub(super) fn hold(directory: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(LOCK))?;
    let until = Instant::now() + CONTENDED;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) if Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// Whether a supervisor holds the lock of the process in `directory`.
pub(super) fn held(directory: &Path) -> io::Result<bool> {
    let file = match OpenOptions::new().write(true).open(directory.join(LOCK)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        file => file?,
    };
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error),
    }
}
