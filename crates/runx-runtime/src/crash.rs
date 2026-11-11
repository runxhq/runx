//! Crash points, for the crash tests alone: compiled only with the
//! `crash-points` feature, which only a test build enables. A point aborts
//! the process where it stands, with no unwinding, no destructors and no
//! writes of its own, so recovery reads exactly what a kill there leaves.
//!
//! `RUNX_CRASH_AT=<boundary>@<call>` chooses the one point that aborts, where
//! `<call>` is a call's address in its run, such as `prepare/1`, or a fixed
//! step's, such as `read`.
//!
//! A power cut is a kill that also loses what no sync made durable. With
//! `RUNX_POWER_LOSS_IMAGE` naming a directory, each sync keeps there what a
//! cut would leave of what it synced, so a test can put the disk back as a
//! cut at the crash point would have left it.
//!
//! The same build says when a run reaches a point a concurrency test waits
//! on, such as waiting on the workspace's lock: `RUNX_REACHED=<point>@<pipe>`
//! names the point and the named pipe it's said on.

use std::path::{Path, PathBuf};

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

/// The environment variable naming the directory a test keeps its power-loss
/// image in: what a power cut would leave on the disk, as of each sync.
const POWER_LOSS_IMAGE: &str = "RUNX_POWER_LOSS_IMAGE";

/// The store whose database is `database` synced a commit: its database and
/// its log, as they stand now, are what a power cut leaves of it, in place of
/// what the image held.
pub(crate) fn store_synced(database: &Path) {
    image(database, "store", |image, name| {
        let stores = image.join("stores");
        std::fs::create_dir_all(&stores)?;
        std::fs::copy(database, stores.join(name))?;
        match std::fs::copy(log(database), log(&stores.join(name))) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::remove_file(log(&stores.join(name))) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                }
            }
            copied => copied.map(|_| ()),
        }
    });
}

/// `path` was synced where it lies, its name with it: a power cut leaves it
/// as it stands now, whatever is written to it later and not synced.
pub(crate) fn file_synced(path: &Path) {
    image(path, "file", |image, name| {
        let files = image.join("files");
        std::fs::create_dir_all(&files)?;
        std::fs::copy(path, files.join(name)).map(|_| ())
    });
}

/// Keep `path`'s copy in the image, when a test asks for one, and note it
/// in the manifest as a `kind`. A test that asked for an image and can't
/// have it has no result, so the process aborts, saying why in the image.
fn image(path: &Path, kind: &str, copy: impl FnOnce(&Path, &str) -> std::io::Result<()>) {
    let Some(image) = std::env::var_os(POWER_LOSS_IMAGE).map(PathBuf::from) else {
        return;
    };
    let name = image_name(path);
    let kept = copy(&image, &name).and_then(|()| {
        use std::io::Write as _;
        let mut manifest = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(image.join("manifest"))?;
        writeln!(manifest, "{kind}\t{name}\t{}", path.display())
    });
    if let Err(error) = kept {
        let _ignored = std::fs::write(
            image.join("failed"),
            format!("imaging {}: {error}", path.display()),
        );
        std::process::abort();
    }
}

/// A SQLite database's log, beside it.
fn log(database: &Path) -> PathBuf {
    let mut name = database.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// The name a copy takes in the image: its path, as one file name.
fn image_name(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '.' {
                character
            } else {
                '_'
            }
        })
        .collect()
}
