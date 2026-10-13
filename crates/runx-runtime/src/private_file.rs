// Module rationale: one discipline for the files runx keeps from other users.
// A private file is created closed to others and never exists open, is read
// only after its directory and the opened file are checked, and lives in a
// directory this user owns that no one else can write. Where a platform gives
// runx no owner-only protection it can set and check, nothing private is
// written or read.
use std::io::{self, Read, Write};
use std::path::Path;

use thiserror::Error;

/// Why a private file cannot be used.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PrivateFileFault {
    #[error("is a link")]
    Link,
    #[error("is not a regular file")]
    NotAFile,
    #[error("is not owned by the current user")]
    Owner,
    #[error("is open to other users")]
    Permissions,
    #[error("is larger than a private file runx writes")]
    TooLarge,
    #[error("already exists")]
    Exists,
    #[error("cannot be restricted to its owner on this platform")]
    Unprotectable,
    #[error("is in {directory}, which {problem}")]
    Directory {
        directory: String,
        problem: DirectoryProblem,
    },
    #[error("could not be read or written: {0}")]
    Io(String),
}

/// Why a directory cannot hold private files.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum DirectoryProblem {
    #[error("is a link")]
    Link,
    #[error("is not a directory")]
    NotADirectory,
    #[error("is not owned by the current user")]
    Owner,
    #[error("other users can write to; restrict it with chmod 700")]
    Writable,
}

pub(crate) fn io_fault(error: &io::Error) -> PrivateFileFault {
    PrivateFileFault::Io(error.kind().to_string())
}

/// The contents of a private file, at most `limit` bytes; `None` when there
/// is none.
pub(crate) fn read_private(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, PrivateFileFault> {
    let Some(mut file) = platform::open_private(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    Read::take(&mut file, limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_fault(&error))?;
    if bytes.len() as u64 > limit {
        return Err(PrivateFileFault::TooLarge);
    }
    Ok(Some(bytes))
}

/// Write `contents` as `path`, replacing what is there: written closed to
/// others under a staged name and renamed into place, so no one else can read
/// it at any moment.
pub(crate) fn replace_private(path: &Path, contents: &[u8]) -> Result<(), PrivateFileFault> {
    let directory = path.parent().ok_or(PrivateFileFault::NotAFile)?;
    let staged = staged_path(path)?;
    let written = write_new(&staged, contents)
        .and_then(|()| std::fs::rename(&staged, path).map_err(|error| io_fault(&error)));
    if written.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    written?;
    platform::sync_directory(directory)
}

/// Create `path` with `contents` unless it already exists, in which case
/// nothing is written and `false` is returned.
pub(crate) fn create_private_new(path: &Path, contents: &[u8]) -> Result<bool, PrivateFileFault> {
    match platform::create_private(path) {
        Ok(mut file) => {
            file.write_all(contents)
                .and_then(|()| file.sync_all())
                .map_err(|error| io_fault(&error))?;
            Ok(true)
        }
        Err(PrivateFileFault::Exists) => Ok(false),
        Err(fault) => Err(fault),
    }
}

/// Write a new private file at `path`, failing if one exists.
pub(crate) fn write_new(path: &Path, contents: &[u8]) -> Result<(), PrivateFileFault> {
    let mut file = platform::create_private(path)?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_fault(&error))
}

/// A sibling name no other writer of `path` will choose.
pub(crate) fn staged_path(path: &Path) -> Result<std::path::PathBuf, PrivateFileFault> {
    use ring::rand::SecureRandom;
    let mut nonce = [0_u8; 8];
    ring::rand::SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| PrivateFileFault::Io("no system randomness".to_owned()))?;
    let name = path
        .file_name()
        .ok_or(PrivateFileFault::NotAFile)?
        .to_string_lossy();
    let suffix = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(path.with_file_name(format!(".{name}.{suffix}.tmp")))
}

pub(crate) use platform::{ensure_private_directory, sync_directory};

#[cfg(unix)]
mod platform {
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    use std::path::Path;

    use super::{DirectoryProblem, PrivateFileFault, io_fault};

    pub(crate) fn ensure_private_directory(directory: &Path) -> Result<(), PrivateFileFault> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| io_fault(&error))?;
        check_private_directory(directory)
    }

    /// The directory is a real directory this user owns, and no one else
    /// may add or replace a file in it.
    fn check_private_directory(directory: &Path) -> Result<(), PrivateFileFault> {
        let metadata = fs::symlink_metadata(directory).map_err(|error| io_fault(&error))?;
        let refuse = |problem| {
            Err(PrivateFileFault::Directory {
                directory: directory.display().to_string(),
                problem,
            })
        };
        if metadata.file_type().is_symlink() {
            return refuse(DirectoryProblem::Link);
        }
        if !metadata.is_dir() {
            return refuse(DirectoryProblem::NotADirectory);
        }
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return refuse(DirectoryProblem::Owner);
        }
        // Others may list the directory; none may add or replace a file in it.
        if metadata.mode() & 0o022 != 0 {
            return refuse(DirectoryProblem::Writable);
        }
        Ok(())
    }

    pub(crate) fn create_private(path: &Path) -> Result<File, PrivateFileFault> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => PrivateFileFault::Exists,
                _ => io_fault(&error),
            })
    }

    /// Open the file without following a link, then check the opened file is
    /// the one the path names, owned by this user and closed to others, in a
    /// directory no one else can write.
    pub(crate) fn open_private(path: &Path) -> Result<Option<File>, PrivateFileFault> {
        let named = match fs::symlink_metadata(path) {
            Ok(named) => named,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_fault(&error)),
        };
        if named.file_type().is_symlink() {
            return Err(PrivateFileFault::Link);
        }
        if !named.is_file() {
            return Err(PrivateFileFault::NotAFile);
        }
        check_private_directory(path.parent().ok_or(PrivateFileFault::NotAFile)?)?;
        let file = File::open(path).map_err(|error| io_fault(&error))?;
        let opened = file.metadata().map_err(|error| io_fault(&error))?;
        if (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
            return Err(PrivateFileFault::Link);
        }
        if opened.uid() != rustix::process::geteuid().as_raw() {
            return Err(PrivateFileFault::Owner);
        }
        if opened.mode() & 0o077 != 0 {
            return Err(PrivateFileFault::Permissions);
        }
        Ok(Some(file))
    }

    pub(crate) fn sync_directory(directory: &Path) -> Result<(), PrivateFileFault> {
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io_fault(&error))
    }
}

/// Elsewhere, including Windows: no owner-only protection runx can set and
/// check without platform FFI, so nothing private is written or read, never
/// weakened.
#[cfg(not(unix))]
mod platform {
    use std::fs::File;
    use std::path::Path;

    use super::PrivateFileFault;

    pub(crate) fn ensure_private_directory(_directory: &Path) -> Result<(), PrivateFileFault> {
        Err(PrivateFileFault::Unprotectable)
    }

    pub(crate) fn create_private(_path: &Path) -> Result<File, PrivateFileFault> {
        Err(PrivateFileFault::Unprotectable)
    }

    /// A file in a private file's place is refused rather than read
    /// unprotected.
    pub(crate) fn open_private(path: &Path) -> Result<Option<File>, PrivateFileFault> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => Err(PrivateFileFault::Unprotectable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(super::io_fault(&error)),
        }
    }

    pub(crate) fn sync_directory(_directory: &Path) -> Result<(), PrivateFileFault> {
        Err(PrivateFileFault::Unprotectable)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_replaced_private_file_is_never_open_to_others() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir()?;
        let keys = home.path().join("keys");
        ensure_private_directory(&keys)?;
        let path = keys.join("secret");
        std::fs::write(&path, b"old")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        assert_eq!(read_private(&path, 64), Err(PrivateFileFault::Permissions));

        replace_private(&path, b"new")?;
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(read_private(&path, 64)?, Some(b"new".to_vec()));
        assert_eq!(read_private(&path, 2), Err(PrivateFileFault::TooLarge));
        assert!(!create_private_new(&path, b"other")?);
        assert_eq!(read_private(&path, 64)?, Some(b"new".to_vec()));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_others_can_write_holds_no_private_file() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir()?;
        let keys = home.path().join("keys");
        std::fs::create_dir(&keys)?;
        std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o777))?;
        assert!(matches!(
            ensure_private_directory(&keys),
            Err(PrivateFileFault::Directory {
                problem: DirectoryProblem::Writable,
                ..
            })
        ));
        Ok(())
    }
}
