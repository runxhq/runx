//! A run's driver lock: whether a process is driving a run now.
//!
//! The process that drives a named run holds an exclusive lock on
//! `.runx/drivers/<digest>.lock`, which the OS releases however the process
//! ends. A driver that ends normally removes the file; one that is killed or
//! interrupted leaves it unlocked, and the run's next driver removes it. The
//! lock is the liveness signal a host waits on after a restart; the run
//! record's stream versions remain what refuses a second writer.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::PathBuf;

use super::RunRecordError;
use crate::paths::RunxPaths;

pub struct DriverLock {
    file: File,
    path: PathBuf,
}

impl DriverLock {
    /// Hold `run_id`'s driver lock, or refuse while another process holds it.
    pub fn acquire(paths: &RunxPaths, run_id: &str) -> Result<Self, RunRecordError> {
        let path = lock_path(paths, run_id);
        let failed = unavailable(run_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(&failed)?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(&failed)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file, path }),
            Err(TryLockError::WouldBlock) => Err(RunRecordError::Driven {
                run_id: run_id.to_owned(),
            }),
            Err(TryLockError::Error(error)) => Err(failed(error)),
        }
    }
}

impl Drop for DriverLock {
    fn drop(&mut self) {
        let _removed = fs::remove_file(&self.path);
        let _released = self.file.unlock();
    }
}

/// Whether a process holds `run_id`'s driver lock now.
pub fn driven(paths: &RunxPaths, run_id: &str) -> Result<bool, RunRecordError> {
    let failed = unavailable(run_id);
    let file = match File::open(lock_path(paths, run_id)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(failed(error)),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(failed(error)),
    }
}

/// A run id may hold characters a file name cannot, so the name is a digest.
fn lock_path(paths: &RunxPaths, run_id: &str) -> PathBuf {
    let digest = runx_contracts::sha256_hex(run_id.as_bytes());
    paths
        .project()
        .join("drivers")
        .join(format!("{}.lock", &digest[..32]))
}

fn unavailable(run_id: &str) -> impl Fn(io::Error) -> RunRecordError + '_ {
    move |source| RunRecordError::DriverLock {
        run_id: run_id.to_owned(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_reads_as_driven_and_refuses_a_second_driver() -> Result<(), RunRecordError> {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let paths = RunxPaths::resolve(&Default::default(), root.path());
        assert!(!driven(&paths, "run_a")?);
        let lock = DriverLock::acquire(&paths, "run_a")?;
        assert!(driven(&paths, "run_a")?);
        assert!(!driven(&paths, "run_b")?);
        assert!(matches!(
            DriverLock::acquire(&paths, "run_a"),
            Err(RunRecordError::Driven { .. })
        ));
        drop(lock);
        assert!(!driven(&paths, "run_a")?);
        assert!(!lock_path(&paths, "run_a").exists());
        Ok(())
    }
}
