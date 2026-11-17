//! The one way runx makes a write durable, on every store it keeps: a full
//! flush to the disk's media. On macOS that is `F_FULLFSYNC`, which std's
//! `sync_all` issues and `fsync(2)` doesn't, since a plain fsync there stops
//! at the drive's cache; on Linux, `fsync`, which flushes the drive's cache;
//! on Windows, `FlushFileBuffers`. A SQLite store syncs its commits and its
//! checkpoints the same way ([`sync_in_full`]), so a commit and a file are
//! durable to the same degree.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replace `path` with `contents` whole and durably: written under a sibling
/// name, flushed, and renamed into place, so a reader, or whatever reads it
/// after a crash, finds the bytes it held or these, never part of either.
pub fn replace_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let staged = staged_path(path)?;
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .and_then(|mut file| {
            file.write_all(contents)?;
            sync_file(&file)
        })
        .and_then(|()| std::fs::rename(&staged, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    written?;
    sync_directory(
        path.parent()
            .filter(|directory| !directory.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
}

/// A sibling name for `path` no other writer of it will choose.
pub(crate) fn staged_path(path: &Path) -> io::Result<PathBuf> {
    use ring::rand::SecureRandom;
    let mut nonce = [0_u8; 8];
    ring::rand::SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| io::Error::other("no system randomness"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "it names no file"))?
        .to_string_lossy();
    let suffix = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(path.with_file_name(format!(".{name}.{suffix}.tmp")))
}

/// Make `file`'s contents durable where it lies, as one durable write.
pub(crate) fn sync_file(file: &File) -> std::io::Result<()> {
    file.sync_all()?;
    crate::work_counts::count(crate::work_counts::Work::DurableWrite);
    Ok(())
}

/// Make the names `directory` holds durable, so a file renamed into it is
/// there after a power cut. Windows can't flush a directory as std opens
/// one, so there the file's own flush is what holds.
pub(crate) fn sync_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let _ = directory;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        File::open(directory)?.sync_all()
    }
}

/// Have SQLite sync `connection`'s commits and checkpoints with the full
/// flush. Its plain sync is `fsync(2)`, which on macOS isn't durable; off
/// macOS these settings change nothing, since there fsync is the full flush.
pub(crate) fn sync_in_full(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    connection.pragma_update(None, "fullfsync", true)?;
    connection.pragma_update(None, "checkpoint_fullfsync", true)
}
