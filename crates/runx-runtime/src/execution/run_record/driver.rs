//! A run's driver lock: whether a process is driving a run now.
//!
//! The process that drives a named run holds an exclusive lock on
//! `.runx/drivers/<digest>.lock`, which the OS releases however the process
//! ends. A driver that ends normally removes the file while it still holds
//! the lock; one that is killed or interrupted leaves it unlocked, and the
//! run's next driver takes it. A driver that locked a file another driver
//! had just removed holds nothing, so after locking it checks that the path
//! still names the file it locked, and takes the path again if not: two
//! drivers can never hold one run across a handoff. The lock is the liveness
//! signal a host waits on after a restart; the run record's stream versions
//! remain what refuses a second writer.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

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
        loop {
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(&failed)?;
            match file.try_lock() {
                Ok(()) if names(&path, &file).map_err(&failed)? => {
                    return Ok(Self { file, path });
                }
                // The file was removed after this open: lock what the path
                // names now.
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => {
                    return Err(RunRecordError::Driven {
                        run_id: run_id.to_owned(),
                    });
                }
                Err(TryLockError::Error(error)) => return Err(failed(error)),
            }
        }
    }
}

/// Whether `path` still names the open `file`.
fn names(path: &Path, file: &File) -> io::Result<bool> {
    let held = same_file::Handle::from_file(file.try_clone()?)?;
    match same_file::Handle::from_path(path) {
        Ok(named) => Ok(named == held),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
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

    #[test]
    fn a_lock_on_a_removed_file_is_taken_again() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let paths = RunxPaths::resolve(&Default::default(), root.path());
        let path = lock_path(&paths, "run_a");
        // A driver opened the file just before the one ending removed it,
        // and a third driver made the path anew.
        fs::create_dir_all(path.parent().ok_or("a parent")?)?;
        let stale = File::create(&path)?;
        fs::remove_file(&path)?;
        let lock = DriverLock::acquire(&paths, "run_a")?;
        assert!(names(&path, &lock.file)?);
        assert!(!names(&path, &stale)?, "the removed file is held by no one");
        assert!(matches!(
            DriverLock::acquire(&paths, "run_a"),
            Err(RunRecordError::Driven { .. })
        ));
        Ok(())
    }
}
