mod owned_process;
mod signals;
#[cfg(windows)]
mod windows_host_job;

/// Default retained bytes per stdout/stderr stream for general operator
/// processes. The supervisor continues draining and hashing the complete stream;
/// capability-specific contracts may choose a narrower or wider retained body.
#[cfg_attr(test, allow(dead_code))]
pub(crate) const STANDARD_PROCESS_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

mod capture;
pub(crate) mod detached;
mod resource_limits;
mod spec;
mod supervisor;
#[cfg(feature = "mcp")]
mod tokio_supervisor;

pub(crate) use self::capture::CapturedOutput;
pub(crate) use self::signals::{ProcessSignal, configure_process_group, signal_process_group_id};
pub(crate) use self::spec::{ProcessOutcome, ProcessSpec, ProcessStdin, ProcessSupervisorError};
pub(crate) use self::supervisor::run_process;
#[cfg(feature = "mcp")]
pub(crate) use self::tokio_supervisor::{OwnedTokioProcess, TokioProcessSpec, spawn_tokio_process};
#[cfg(windows)]
pub(crate) use self::windows_host_job::ensure_windows_host_job;

/// Return a working directory that child runtimes can consume on Windows.
///
/// `std::fs::canonicalize` preserves long-path safety by returning verbatim
/// paths there. Some child runtimes, including Node.js, cannot initialize from
/// a verbatim working directory. Convert only paths that fit the traditional
/// Windows limit; long paths retain their verbatim form.
#[inline]
pub(crate) fn child_process_cwd(path: &std::path::Path) -> std::borrow::Cow<'_, std::path::Path> {
    #[cfg(not(windows))]
    {
        std::borrow::Cow::Borrowed(path)
    }
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        const MAX_PATH: usize = 260;
        const VERBATIM_PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
        const VERBATIM_UNC_PREFIX: &[u16] = &[
            b'\\' as u16,
            b'\\' as u16,
            b'?' as u16,
            b'\\' as u16,
            b'U' as u16,
            b'N' as u16,
            b'C' as u16,
            b'\\' as u16,
        ];

        let verbatim = path.as_os_str().encode_wide().collect::<Vec<_>>();
        let plain = if verbatim.starts_with(VERBATIM_UNC_PREFIX) {
            let mut plain = vec![b'\\' as u16, b'\\' as u16];
            plain.extend_from_slice(&verbatim[VERBATIM_UNC_PREFIX.len()..]);
            plain
        } else if verbatim.starts_with(VERBATIM_PREFIX) {
            verbatim[VERBATIM_PREFIX.len()..].to_vec()
        } else {
            return std::borrow::Cow::Borrowed(path);
        };

        if plain.len() >= MAX_PATH {
            return std::borrow::Cow::Borrowed(path);
        }
        std::borrow::Cow::Owned(std::path::PathBuf::from(OsString::from_wide(&plain)))
    }
}

pub(crate) fn cleanup_paths_quietly(paths: &[std::path::PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_dir_all(path);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::borrow::Cow;
    use std::path::Path;
    use std::process::Command;

    use super::child_process_cwd;

    #[test]
    fn windows_child_process_cwd_removes_short_verbatim_prefixes() {
        assert_eq!(
            child_process_cwd(Path::new(r"\\?\C:\runx\skill")).as_ref(),
            Path::new(r"C:\runx\skill")
        );
        assert_eq!(
            child_process_cwd(Path::new(r"\\?\UNC\server\share\skill")).as_ref(),
            Path::new(r"\\server\share\skill")
        );
    }

    #[test]
    fn windows_child_process_cwd_keeps_long_verbatim_paths() {
        let path = std::path::PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(260)));
        assert!(matches!(child_process_cwd(&path), Cow::Borrowed(_)));
    }

    #[test]
    fn windows_child_process_cwd_starts_node_from_a_canonical_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary_directory = tempfile::tempdir()?;
        let directory = temporary_directory.path().canonicalize()?;
        assert!(directory.to_string_lossy().starts_with(r"\\?\"));
        let cwd = child_process_cwd(&directory);
        let status = Command::new("node")
            .args(["-e", "process.exit(process.cwd() ? 0 : 1)"])
            .current_dir(cwd.as_ref())
            .status()?;
        assert!(status.success());
        Ok(())
    }
}

/// Keep concurrent launches from inheriting another child's transient pipes.
/// Apple's std pipe implementation sets CLOEXEC after pipe creation; the lock
/// covers pipe setup and spawn only, never a handshake, wait, or invocation.
/// All runtime process owners must share this lock rather than lock per pool.
#[inline]
pub(crate) fn with_spawn_lock<T>(spawn: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    #[cfg(target_vendor = "apple")]
    let _guard = {
        static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        SPAWN_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    spawn()
}

#[cfg(not(windows))]
pub(crate) fn ensure_windows_host_job() -> std::io::Result<()> {
    Ok(())
}
