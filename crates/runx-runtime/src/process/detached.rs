//! Detached processes: a dev server, a watcher or an interactive program,
//! started by one call and read, written and stopped by later ones, in the
//! session that started it or any later session of the same actor in the
//! project. Each runs under a supervisor of its own, the runx binary run
//! again in its own session (see `supervisor`), so it outlives the driver
//! that started it.
//!
//! A start never orphans or doubles. Its record, keyed by the call that
//! started it, commits before its supervisor is spawned, and the supervisor
//! confirms it under a lock it holds for its life. A record no supervisor
//! confirmed is settled by that lock: held, it is still starting; free once
//! the start is older than the confirmation window, the start failed. The
//! same call sent again finds the same record and returns the same process.

mod control;
mod followed;
mod launch;
mod lock;
mod log;
mod record;
mod supervisor;
#[cfg(unix)]
mod terminal;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use runx_contracts::{
    ProcessExit, ProcessKind, ProcessList, ProcessOutput, ProcessOutputChunk, RunxProcess,
    TerminalSize,
};
use thiserror::Error;

use self::control::Control;
use self::record::{ProcessEvent, Record, Started};
use crate::kernel::store::{KernelStore, KernelStoreError};
use crate::paths::RunxPaths;
use crate::tool_catalogs::ToolCall;

pub(crate) use self::followed::Followed;
pub use self::supervisor::{SUPERVISOR_FLAG, supervise_from_args};

/// How long a start waits for its supervisor to confirm it, and how long a
/// record no supervisor confirmed counts as starting.
const CONFIRM_WINDOW: Duration = Duration::from_secs(10);
/// The exited processes whose logs a project keeps, newest first.
const KEPT_LOGS: usize = 32;
/// The most processes a listing returns, most recently changed first.
const LISTED: usize = 200;
/// The most output one read returns.
pub(crate) const MAX_READ_BYTES: usize = 256 * 1024;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("{0} is not a process id")]
    InvalidId(String),
    #[error("there is no process {0}")]
    Unknown(String),
    #[error("process {0} has exited")]
    Exited(String),
    #[error("process {0} has no terminal")]
    NoTerminal(String),
    #[error("{0}")]
    Refused(String),
    #[error("process state is unavailable: {0}")]
    State(String),
}

impl From<KernelStoreError> for ProcessError {
    fn from(error: KernelStoreError) -> Self {
        Self::State(error.to_string())
    }
}

impl From<std::io::Error> for ProcessError {
    fn from(error: std::io::Error) -> Self {
        Self::State(error.to_string())
    }
}

/// Whose processes a caller reaches.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Reach<'a> {
    /// A run's: the processes its actor's runs started (an operator's runs,
    /// with no actor, share theirs).
    Actor(Option<&'a str>),
    /// The person's: every process in the project.
    Person,
}

/// A process to start, as its call admitted it.
pub(crate) struct Start<'a> {
    pub(crate) call: &'a ToolCall,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    /// The child's whole environment.
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) tty: Option<TerminalSize>,
    pub(crate) max_seconds: u64,
}

/// A project's detached processes.
pub(crate) struct Processes {
    store: Arc<KernelStore>,
    /// The project's runx directory, as its kernel store resolves it.
    project: PathBuf,
}

impl Processes {
    pub(crate) fn of(paths: &RunxPaths) -> Result<Self, ProcessError> {
        Self::at(paths.project())
    }

    fn at(project: &Path) -> Result<Self, ProcessError> {
        let store = KernelStore::open(project)?;
        let project = store
            .path()
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ProcessError::State("the kernel store has no directory".to_owned()))?;
        Ok(Self { store, project })
    }

    /// Start the process `start` names, or return the one its call already
    /// started.
    pub(crate) fn start(&self, start: Start<'_>) -> Result<RunxProcess, ProcessError> {
        let id = process_id(&start.call.key);
        if let Some(record) = self.record(&id)? {
            return Ok(self.settled(record)?.view());
        }
        self.prune();
        fs::create_dir_all(self.directory(&id)?)?;
        let started = Started {
            kind: ProcessKind::Detached,
            call: start.call.key.clone(),
            actor: start.call.actor.clone(),
            command: start.command,
            args: start.args,
            cwd: start.cwd.to_string_lossy().into_owned(),
            tty: start.tty,
            max_seconds: start.max_seconds,
            created_at: crate::time::now_iso8601(),
        };
        match record::append(
            &self.store,
            &id,
            0,
            &ProcessEvent::Started(Box::new(started)),
        ) {
            // Another driver sent the same call at once: its start stands.
            Err(KernelStoreError::Conflict { .. }) => {
                return self.find(&id, Reach::Person).map(|record| record.view());
            }
            started => started?,
        }
        if let Err(error) = launch::supervisor(&self.project, &id, &start.env) {
            let record = self.known(&id)?;
            self.exited(
                &record,
                record::failed(format!("its supervisor could not be started: {error}")),
            )?;
        }
        let until = Instant::now() + CONFIRM_WINDOW;
        loop {
            let record = self.known(&id)?;
            if record.confirmed.is_some() || record.exit.is_some() || Instant::now() >= until {
                return Ok(self.settled(record)?.view());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// What process `id` wrote after the cursor `after`, about `max_bytes`
    /// of it.
    pub(crate) fn read(
        &self,
        id: &str,
        reach: Reach<'_>,
        after: u64,
        max_bytes: usize,
    ) -> Result<ProcessOutput, ProcessError> {
        self.find(id, reach)?;
        let read = log::read(
            &self.directory(id)?.join("output"),
            after,
            max_bytes.clamp(1, MAX_READ_BYTES),
        )
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::InvalidInput => ProcessError::Refused(error.to_string()),
            _ => ProcessError::from(error),
        })?;
        // Read after the output, so an exited process's output is all there.
        let process = self.find(id, reach)?.view();
        Ok(ProcessOutput {
            process_id: process.process_id,
            state: process.state,
            exit: process.exit,
            from: read.from,
            next: read.next,
            dropped: read.dropped,
            chunks: read
                .chunks
                .into_iter()
                .map(|(stream, bytes)| ProcessOutputChunk {
                    stream,
                    text: String::from_utf8_lossy(&bytes).into_owned(),
                })
                .collect(),
        })
    }

    /// Send `input` to process `id`'s stdin or terminal; the same `call`
    /// sent again is applied once.
    pub(crate) fn write(
        &self,
        id: &str,
        reach: Reach<'_>,
        call: Option<&str>,
        input: String,
    ) -> Result<RunxProcess, ProcessError> {
        let record = self.running(id, reach)?;
        if record.started.kind == ProcessKind::Command {
            return Err(ProcessError::Refused(format!(
                "process {id} is a command: it takes no input"
            )));
        }
        self.send(
            &record,
            &Control::Write {
                call: call.map(str::to_owned),
                input,
            },
        )?;
        Ok(record.view())
    }

    /// Resize process `id`'s terminal.
    pub(crate) fn resize(
        &self,
        id: &str,
        reach: Reach<'_>,
        size: TerminalSize,
    ) -> Result<RunxProcess, ProcessError> {
        let record = self.running(id, reach)?;
        if record.started.tty.is_none() {
            return Err(ProcessError::NoTerminal(id.to_owned()));
        }
        self.send(
            &record,
            &Control::Resize {
                rows: size.rows,
                cols: size.cols,
            },
        )?;
        Ok(record.view())
    }

    /// Stop process `id`: terminate it, then kill it, and return it once its
    /// exit is recorded.
    pub(crate) fn stop(&self, id: &str, reach: Reach<'_>) -> Result<RunxProcess, ProcessError> {
        let record = self.find(id, reach)?;
        if record.exit.is_some() {
            return Ok(record.view());
        }
        self.send(&record, &Control::Stop)?;
        let until = Instant::now() + supervisor::STOP_GRACE + Duration::from_secs(2);
        loop {
            let record = self.find(id, reach)?;
            if record.exit.is_some() || Instant::now() >= until {
                return Ok(record.view());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// The processes `reach` may see, most recently changed first.
    pub(crate) fn list(&self, reach: Reach<'_>) -> Result<ProcessList, ProcessError> {
        let mut processes = Vec::new();
        for record in record::recent(&self.store, LISTED)? {
            if reaches(reach, &record) {
                processes.push(self.settled(record)?.view());
            }
        }
        Ok(ProcessList { processes })
    }

    fn directory(&self, id: &str) -> Result<PathBuf, ProcessError> {
        if !is_process_id(id) {
            return Err(ProcessError::InvalidId(id.to_owned()));
        }
        Ok(self.project.join(crate::paths::PROCESSES_DIR).join(id))
    }

    fn record(&self, id: &str) -> Result<Option<Record>, ProcessError> {
        Ok(record::read(&self.store, id)?)
    }

    fn known(&self, id: &str) -> Result<Record, ProcessError> {
        self.record(id)?
            .ok_or_else(|| ProcessError::Unknown(id.to_owned()))
    }

    /// Process `id`'s record as it stands, when `reach` may see it. One it
    /// may not is unknown to it.
    fn find(&self, id: &str, reach: Reach<'_>) -> Result<Record, ProcessError> {
        self.directory(id)?;
        let record = self
            .record(id)?
            .filter(|record| reaches(reach, record))
            .ok_or_else(|| ProcessError::Unknown(id.to_owned()))?;
        self.settled(record)
    }

    /// Process `id` while it can still take what is sent to it.
    fn running(&self, id: &str, reach: Reach<'_>) -> Result<Record, ProcessError> {
        let record = self.find(id, reach)?;
        if record.exit.is_some() {
            return Err(ProcessError::Exited(id.to_owned()));
        }
        Ok(record)
    }

    fn send(&self, record: &Record, control: &Control) -> Result<(), ProcessError> {
        Ok(control::send(&self.directory(&record.id)?, control)?)
    }

    /// `record`, settled when no supervisor holds it: its supervisor died, or
    /// never started within the confirmation window.
    fn settled(&self, record: Record) -> Result<Record, ProcessError> {
        if record.exit.is_some() || lock::held(&self.directory(&record.id)?)? {
            return Ok(record);
        }
        let message = match record.confirmed {
            Some(_) => "its supervisor ended without recording its exit",
            None if within(&record.started.created_at, CONFIRM_WINDOW) => return Ok(record),
            None => "its supervisor never started",
        };
        match self.exited(&record, record::failed(message.to_owned())) {
            // Or it settled in the meantime.
            Ok(()) | Err(KernelStoreError::Conflict { .. }) => self.known(&record.id),
            Err(error) => Err(error.into()),
        }
    }

    /// Record that process `record`'s child runs as `pid`.
    fn confirmed(&self, mut record: Record, pid: u32) -> Result<Record, KernelStoreError> {
        let confirmed = record::Confirmed {
            pid,
            started_at: crate::time::now_iso8601(),
        };
        record::append(
            &self.store,
            &record.id,
            record.version,
            &ProcessEvent::Confirmed(confirmed.clone()),
        )?;
        record.version += 1;
        record.confirmed = Some(confirmed);
        Ok(record)
    }

    /// Record how process `record` exited.
    fn exited(&self, record: &Record, exit: ProcessExit) -> Result<(), KernelStoreError> {
        record::append(
            &self.store,
            &record.id,
            record.version,
            &ProcessEvent::Exited(exit),
        )
    }

    /// Drop the logs of exited processes beyond the newest the project keeps.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(self.project.join(crate::paths::PROCESSES_DIR)) else {
            return;
        };
        let mut exited = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|id| is_process_id(id))
            .filter_map(|id| {
                let at = self.record(&id).ok()??.exit?.at;
                Some((at, id))
            })
            .collect::<Vec<_>>();
        exited.sort_by(|left, right| right.cmp(left));
        for (_, id) in exited.into_iter().skip(KEPT_LOGS) {
            if let Ok(directory) = self.directory(&id) {
                let _ = fs::remove_dir_all(directory);
            }
        }
    }
}

fn reaches(reach: Reach<'_>, record: &Record) -> bool {
    match reach {
        Reach::Person => true,
        Reach::Actor(actor) => record.started.actor.as_deref() == actor,
    }
}

/// The id of the process the call `key` starts.
fn process_id(key: &str) -> String {
    let digest = runx_contracts::sha256_prefixed(key.as_bytes());
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    format!("proc_{}", &hex[..20])
}

fn is_process_id(id: &str) -> bool {
    id.strip_prefix("proc_").is_some_and(|hex| {
        hex.len() == 20
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Whether the instant `at` is less than `window` ago.
fn within(at: &str, window: Duration) -> bool {
    let seconds = |moment: &str| {
        runx_core::policy::parse_rfc3339_moment(moment)
            .map(|(days, seconds, _)| days * 86_400 + seconds)
    };
    match (seconds(at), seconds(&crate::time::now_iso8601())) {
        (Some(at), Some(now)) => now - at < window.as_secs() as i64,
        _ => false,
    }
}

/// Every process the project's runs in `env` and `cwd` started, or only
/// `actor`'s, most recently changed first.
pub fn list_processes(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    actor: Option<&str>,
) -> Result<ProcessList, ProcessError> {
    let reach = actor.map_or(Reach::Person, |actor| Reach::Actor(Some(actor)));
    Processes::of(&RunxPaths::resolve(env, cwd))?.list(reach)
}

/// What process `id` wrote after the cursor `after`.
pub fn read_process(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    id: &str,
    after: u64,
    max_bytes: usize,
) -> Result<ProcessOutput, ProcessError> {
    Processes::of(&RunxPaths::resolve(env, cwd))?.read(id, Reach::Person, after, max_bytes)
}

/// Stop process `id`.
pub fn stop_process(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    id: &str,
) -> Result<RunxProcess, ProcessError> {
    Processes::of(&RunxPaths::resolve(env, cwd))?.stop(id, Reach::Person)
}
