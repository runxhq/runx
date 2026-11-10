//! The workspace's lock (vocabulary-and-loops.md, Stage 4's concurrency;
//! D40). A workspace's files are what nothing else in a run owns, so what
//! may change them takes the workspace's lock. A command, a tool step that
//! runs a process and an operation a process carries out that writes hold it
//! alone, since each can change anything and states no precondition; a native
//! file change holds it alone for its digest check and its write, so two
//! runs' changes to one file can't both pass the check. A native read shares
//! it, and so does an operation a process carries out that reads, its `read`
//! its author's promise that it changes nothing (D33). Every run on the
//! workspace takes the same lock, `.runx/workspace.lock`, from this process
//! or another, on a handle no process runx starts inherits.
//!
//! A command that holds the lock may start runx again, which mustn't wait on
//! the lock its own command holds. So a hold alone writes a fresh token to a
//! record beside the lock and hands it to the processes it runs in their
//! environment, and a runx started under one joins the hold, taking nothing,
//! only while the record still names that token. A process that outlives its
//! command, or runx started after the command returned, finds another token
//! or none, and takes the lock as any run does: a late detached process holds
//! nothing and gets no free pass.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::PathBuf;

use crate::RuntimeError;
use crate::paths::RunxPaths;

/// The variable a process run under a hold alone finds its token in.
pub(crate) const RUNX_WORKSPACE_LOCK_TOKEN_ENV: &str = "RUNX_WORKSPACE_LOCK_TOKEN";

/// How a step takes the workspace's lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockMode {
    /// A native read, or a process-backed read: alongside other reads.
    Shared,
    /// A command, a process step, a process-backed write or a native file
    /// change: alone.
    Alone,
}

/// A step's place in the workspace's lock, given up when dropped.
#[derive(Debug)]
pub(crate) struct WorkspaceLock {
    /// The locked handle; none for a runx that joined the hold of the
    /// command it runs under.
    file: Option<File>,
    /// The record a hold alone named its token in, cleared as it ends.
    record: Option<PathBuf>,
    /// The token a hold alone hands the processes it runs.
    token: Option<String>,
}

impl WorkspaceLock {
    /// Take the workspace's lock as `mode` says, waiting for whoever holds
    /// it, or join the hold of the command this runx runs under, when its
    /// environment, `env`, carries the token the record still names.
    pub(crate) fn take(
        paths: &RunxPaths,
        env: &BTreeMap<String, String>,
        mode: LockMode,
    ) -> Result<Self, RuntimeError> {
        let failed = |source: io::Error| RuntimeError::io("taking the workspace's lock", source);
        let directory = paths.project();
        let record = directory.join("workspace.lock.token");
        if let Some(token) = env.get(RUNX_WORKSPACE_LOCK_TOKEN_ENV)
            && read_record(&record).map_err(failed)?.as_deref() == Some(token.as_str())
        {
            return Ok(Self {
                file: None,
                record: None,
                token: None,
            });
        }
        fs::create_dir_all(directory).map_err(failed)?;
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("workspace.lock"))
            .map_err(failed)?;
        let taken = match mode {
            LockMode::Shared => file.try_lock_shared(),
            LockMode::Alone => file.try_lock(),
        };
        match taken {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                #[cfg(feature = "crash-points")]
                crate::crash::reached("workspace_lock_wait");
                match mode {
                    LockMode::Shared => file.lock_shared(),
                    LockMode::Alone => file.lock(),
                }
                .map_err(failed)?;
            }
            Err(TryLockError::Error(source)) => return Err(failed(source)),
        }
        let mut lock = Self {
            file: Some(file),
            record: None,
            token: None,
        };
        if mode == LockMode::Alone {
            let token = fresh_token()?;
            write_record(&record, &token).map_err(failed)?;
            lock.record = Some(record);
            lock.token = Some(token);
        }
        Ok(lock)
    }

    /// `env` for a process this hold runs: with its token, when it holds
    /// the lock alone, so a runx the process starts joins it.
    pub(crate) fn environment(&self, env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut env = env.clone();
        if let Some(token) = &self.token {
            env.insert(RUNX_WORKSPACE_LOCK_TOKEN_ENV.to_owned(), token.clone());
        }
        env
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        if let Some(record) = &self.record {
            let _cleared = fs::remove_file(record);
        }
        if let Some(file) = &self.file {
            let _released = file.unlock();
        }
    }
}

/// The token the record names; none when no hold alone holds the lock.
fn read_record(record: &std::path::Path) -> io::Result<Option<String>> {
    match fs::read_to_string(record) {
        Ok(token) => Ok(Some(token)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Name `token` in the record whole, so a runx reading it never sees half.
fn write_record(record: &std::path::Path, token: &str) -> io::Result<()> {
    let written = record.with_extension("token.new");
    fs::write(&written, token)?;
    fs::rename(&written, record)
}

/// 128 random bits, in hex.
fn fresh_token() -> Result<String, RuntimeError> {
    use ring::rand::SecureRandom as _;
    let mut random = [0_u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| {
            RuntimeError::effect_state(
                "taking the workspace's lock",
                "the system's random source could not be read",
            )
        })?;
    Ok(runx_contracts::hex_lower(&random))
}
