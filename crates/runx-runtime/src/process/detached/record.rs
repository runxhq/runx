//! A detached process's record: one kernel stream per process, keyed by its
//! id, holding what was started, its supervisor's confirmation, and how it
//! exited.

use std::sync::Arc;

use runx_contracts::{
    ProcessExit, ProcessExitReason, ProcessKind, ProcessState, RunxProcess, TerminalSize,
};
use serde::{Deserialize, Serialize};

use crate::kernel::store::{Append, Expected, KernelStore, KernelStoreError, StreamEvent};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(super) enum ProcessEvent {
    Started(Box<Started>),
    Confirmed(Confirmed),
    Exited(ProcessExit),
}

impl StreamEvent for ProcessEvent {
    const KIND: &'static str = "process";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Started(_) => "started",
            Self::Confirmed(_) => "confirmed",
            Self::Exited(_) => "exited",
        }
    }
}

/// What was started, by which call, for which actor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Started {
    pub(super) kind: ProcessKind,
    /// The call that started it, as its run names it.
    pub(super) call: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) actor: Option<String>,
    pub(super) command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) args: Vec<String>,
    pub(super) cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) tty: Option<TerminalSize>,
    pub(super) max_seconds: u64,
    pub(super) created_at: String,
}

/// The supervisor has the child.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Confirmed {
    pub(super) pid: u32,
    pub(super) started_at: String,
}

/// A process as its stream records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub(super) id: String,
    pub(super) version: u64,
    pub(super) started: Started,
    pub(super) confirmed: Option<Confirmed>,
    pub(super) exit: Option<ProcessExit>,
}

impl Record {
    /// The process as its callers read it.
    pub(super) fn view(&self) -> RunxProcess {
        let started = &self.started;
        RunxProcess {
            process_id: self.id.clone(),
            kind: started.kind,
            call: started.call.clone(),
            command: started.command.clone(),
            args: started.args.clone(),
            cwd: started.cwd.clone(),
            tty: started.tty,
            max_seconds: started.max_seconds,
            actor: started.actor.clone(),
            created_at: started.created_at.clone(),
            state: match (&self.confirmed, &self.exit) {
                (_, Some(_)) => ProcessState::Exited,
                (Some(_), None) => ProcessState::Running,
                (None, None) => ProcessState::Starting,
            },
            pid: self.confirmed.as_ref().map(|confirmed| confirmed.pid),
            started_at: self
                .confirmed
                .as_ref()
                .map(|confirmed| confirmed.started_at.clone()),
            exit: self.exit.clone(),
        }
    }
}

/// Process `id`'s record, when it has one.
pub(super) fn read(store: &KernelStore, id: &str) -> Result<Option<Record>, KernelStoreError> {
    let mut record: Option<Record> = None;
    for recorded in store.read::<ProcessEvent>(id)? {
        match (recorded.event, record.as_mut()) {
            (ProcessEvent::Started(started), None) => {
                record = Some(Record {
                    id: id.to_owned(),
                    version: recorded.version,
                    started: *started,
                    confirmed: None,
                    exit: None,
                });
            }
            (ProcessEvent::Confirmed(confirmed), Some(record)) => {
                record.confirmed = Some(confirmed);
                record.version = recorded.version;
            }
            (ProcessEvent::Exited(exit), Some(record)) => {
                record.exit = Some(exit);
                record.version = recorded.version;
            }
            _ => {
                return Err(KernelStoreError::Codec(format!(
                    "process {id}'s record is out of order"
                )));
            }
        }
    }
    Ok(record)
}

/// Record `event` on process `id`, whose stream is at `version`.
pub(super) fn append(
    store: &Arc<KernelStore>,
    id: &str,
    version: u64,
    event: &ProcessEvent,
) -> Result<(), KernelStoreError> {
    let expected = match version {
        0 => Expected::Absent,
        version => Expected::Version(version),
    };
    store
        .append(vec![Append::kernel(id, expected, event)?])
        .map(|_| ())
}

/// The processes the project records, most recently changed first.
pub(super) fn recent(store: &KernelStore, limit: usize) -> Result<Vec<Record>, KernelStoreError> {
    let mut records = Vec::new();
    for head in store.recent_heads::<ProcessEvent>(None, limit)? {
        if let Some(record) = read(store, &head.key)? {
            records.push(record);
        }
    }
    Ok(records)
}

/// A process that never ran, or whose supervisor ended without recording it.
pub(super) fn failed(message: String) -> ProcessExit {
    ProcessExit {
        reason: ProcessExitReason::Failed,
        code: None,
        signal: None,
        message: Some(message),
        at: crate::time::now_iso8601(),
    }
}

/// How a process that exited with `status` ended, for `reason`.
pub(super) fn exit_of(status: &std::process::ExitStatus, reason: ProcessExitReason) -> ProcessExit {
    ProcessExit {
        reason,
        code: status.code(),
        signal: signal_of(status),
        message: None,
        at: crate::time::now_iso8601(),
    }
}

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn signal_of(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}
