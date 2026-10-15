//! A command its driver runs and follows: `command.execute` made by a run's
//! call is recorded as a process of that call, of kind `command`, under the
//! lock its driver holds while it runs. Its output is written to the
//! process's log as it is read, so it is read by cursor while it runs, and a
//! stop sent to the process stops it. It takes no input, and its exit is
//! recorded when its call settles.

use std::fs::File;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use runx_contracts::{ProcessExit, ProcessExitReason, ProcessKind, ProcessStream};

use super::control::{Control, Mailbox};
use super::log::{LogWriter, Pending};
use super::record::{ProcessEvent, Record, Started};
use super::{ProcessError, Processes, Start, lock, process_id, record};
use crate::kernel::store::KernelStoreError;

pub(crate) struct Followed {
    processes: Processes,
    record: Mutex<Record>,
    log: Mutex<LogWriter>,
    /// What each stream holds back, by stream.
    pending: Mutex<[Pending; 3]>,
    mailbox: Mutex<Mailbox>,
    stopping: AtomicBool,
    /// Held while the command runs, released when this is dropped.
    _lock: File,
}

impl std::fmt::Debug for Followed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Followed")
            .field("stopping", &self.stopping)
            .finish_non_exhaustive()
    }
}

impl Processes {
    /// Follow the command `start` runs, as a process of its call; none when
    /// its call already has one.
    pub(crate) fn follow(self, start: Start<'_>) -> Result<Option<Followed>, ProcessError> {
        let id = process_id(&start.call.key);
        if self.record(&id)?.is_some() {
            return Ok(None);
        }
        let directory = self.directory(&id)?;
        std::fs::create_dir_all(&directory)?;
        self.prune();
        let Some(held) = lock::hold(&directory)? else {
            return Ok(None);
        };
        let started = Started {
            kind: ProcessKind::Command,
            call: start.call.key.clone(),
            actor: start.call.actor.clone(),
            command: start.command,
            args: start.args,
            cwd: start.cwd.to_string_lossy().into_owned(),
            tty: None,
            max_seconds: start.max_seconds,
            created_at: crate::time::now_iso8601(),
        };
        match record::append(
            &self.store,
            &id,
            0,
            &ProcessEvent::Started(Box::new(started)),
        ) {
            Err(KernelStoreError::Conflict { .. }) => return Ok(None),
            appended => appended?,
        }
        let record = self.known(&id)?;
        let log = LogWriter::create(&directory.join("output"))?;
        let mailbox = Mailbox::open(&directory)?;
        Ok(Some(Followed {
            processes: self,
            record: Mutex::new(record),
            log: Mutex::new(log),
            pending: Mutex::new(Default::default()),
            mailbox: Mutex::new(mailbox),
            stopping: AtomicBool::new(false),
            _lock: held,
        }))
    }
}

impl Followed {
    /// Record how the command ended: as its supervision says, or failed for
    /// why it could not be supervised.
    pub(crate) fn settle(
        &self,
        ran: Result<&crate::process::ProcessOutcome, String>,
    ) -> Result<(), ProcessError> {
        self.finish(match ran {
            Ok(outcome) => {
                let reason = if outcome.timed_out {
                    ProcessExitReason::TimedOut
                } else if outcome.stopped {
                    ProcessExitReason::Stopped
                } else {
                    ProcessExitReason::Exited
                };
                record::exit_of(&outcome.status, reason)
            }
            Err(message) => record::failed(message),
        })
    }

    /// Record how the command ended, after the last of its output.
    fn finish(&self, exit: ProcessExit) -> Result<(), ProcessError> {
        for stream in [
            ProcessStream::Stdout,
            ProcessStream::Stderr,
            ProcessStream::Terminal,
        ] {
            let rest = self
                .pending
                .lock()
                .map(|mut pending| pending[index(stream)].rest())
                .unwrap_or_default();
            self.write(stream, &rest);
        }
        let record = self
            .record
            .lock()
            .map_err(|_| ProcessError::State("the command's record is poisoned".to_owned()))?;
        Ok(self.processes.exited(&record, exit)?)
    }

    fn write(&self, stream: ProcessStream, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Ok(mut log) = self.log.lock() {
            let _ = log.append(stream, bytes);
        }
    }
}

impl crate::process::Follower for Followed {
    fn spawned(&self, pid: u32) {
        if let Ok(mut record) = self.record.lock()
            && let Ok(confirmed) = self.processes.confirmed(record.clone(), pid)
        {
            *record = confirmed;
        }
    }

    fn output(&self, stream: ProcessStream, bytes: &[u8]) {
        let whole = self
            .pending
            .lock()
            .map(|mut pending| pending[index(stream)].whole(bytes))
            .unwrap_or_default();
        self.write(stream, &whole);
    }

    fn stop_requested(&self) -> bool {
        if self.stopping.load(Ordering::Relaxed) {
            return true;
        }
        let stop = self
            .mailbox
            .lock()
            .is_ok_and(|mut mailbox| mailbox.take().unwrap_or_default().contains(&Control::Stop));
        if stop {
            self.stopping.store(true, Ordering::Relaxed);
        }
        stop
    }
}

fn index(stream: ProcessStream) -> usize {
    match stream {
        ProcessStream::Stdout => 0,
        ProcessStream::Stderr => 1,
        ProcessStream::Terminal => 2,
    }
}
