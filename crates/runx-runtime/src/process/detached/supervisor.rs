//! A detached process's supervisor: the runx binary run again as
//! `runx --runx-supervisor <project directory> <process id>`, in a session of
//! its own, holding the process's lock for its life. It runs the child its
//! record names with the environment it was itself given, writes the child's
//! output to its log as it arrives, applies what its mailbox brings, stops
//! the child at `max_seconds`, and records how it exited.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use runx_contracts::{ProcessExitReason, ProcessStream, TerminalSize};

use super::Processes;
use super::control::{Control, Mailbox};
use super::log::{LogWriter, Pending};
use super::record::Started;
use crate::process::ProcessSignal;

/// The first argument that runs the binary as a supervisor.
pub const SUPERVISOR_FLAG: &str = "--runx-supervisor";

const TICK: Duration = Duration::from_millis(20);
/// How long a terminated process has to exit before it is killed.
pub(super) const STOP_GRACE: Duration = Duration::from_secs(5);

/// Supervise when `args`, the binary's arguments after its name, ask for it,
/// and return the exit code; none when they ask for anything else.
pub fn supervise_from_args(args: &[OsString]) -> Option<i32> {
    let (flag, rest) = args.split_first()?;
    #[cfg(unix)]
    if flag.as_os_str() == super::terminal::TERMINAL_FLAG {
        return Some(super::terminal::become_command(rest));
    }
    let ([project, id], true) = (rest, flag.as_os_str() == SUPERVISOR_FLAG) else {
        return None;
    };
    // A session of its own: no terminal's hangup or interrupt reaches it.
    #[cfg(unix)]
    let _ = rustix::process::setsid();
    let supervised = id
        .to_str()
        .ok_or_else(|| "the process id is not text".to_owned())
        .and_then(|id| supervise(Path::new(project), id));
    Some(i32::from(supervised.is_err()))
}

/// Supervise process `id` of the project at `project`, unless another
/// supervisor holds it or it already started.
pub(super) fn supervise(project: &Path, id: &str) -> Result<(), String> {
    let processes = Processes::at(project).map_err(|error| error.to_string())?;
    let directory = processes.directory(id).map_err(|error| error.to_string())?;
    let Some(_lock) = super::lock::hold(&directory).map_err(|error| error.to_string())? else {
        return Ok(());
    };
    let record = match processes.record(id).map_err(|error| error.to_string())? {
        Some(record) if record.confirmed.is_none() && record.exit.is_none() => record,
        _ => return Ok(()),
    };
    let log = Arc::new(Mutex::new(
        LogWriter::create(&directory.join("output")).map_err(|error| error.to_string())?,
    ));
    let mut mailbox = Mailbox::open(&directory).map_err(|error| error.to_string())?;
    let mut child = match Supervised::spawn(&record.started, &log) {
        Ok(child) => child,
        Err(error) => {
            let exit = super::record::failed(format!("it could not be started: {error}"));
            return processes
                .exited(&record, exit)
                .map_err(|error| error.to_string());
        }
    };
    let record = processes
        .confirmed(record, child.pid)
        .map_err(|error| error.to_string())?;
    let (status, reason) = child.run(&mut mailbox, record.started.max_seconds);
    child.finish();
    let exit = match status {
        Ok(status) => super::record::exit_of(&status, reason),
        Err(error) => super::record::failed(format!("its supervisor lost it: {error}")),
    };
    processes
        .exited(&record, exit)
        .map_err(|error| error.to_string())
}

/// A child under supervision: its pid, how input reaches it, and the threads
/// that copy its output to the log.
struct Supervised {
    pid: u32,
    root: Root,
    input: mpsc::Sender<Vec<u8>>,
    readers: Vec<JoinHandle<()>>,
}

enum Root {
    Piped(crate::process::owned_process::OwnedProcess),
    #[cfg(unix)]
    Terminal(std::process::Child, super::terminal::Terminal),
}

impl Supervised {
    fn spawn(started: &Started, log: &Arc<Mutex<LogWriter>>) -> std::io::Result<Self> {
        match started.tty {
            Some(size) => Self::spawn_terminal(started, size, log),
            None => Self::spawn_piped(started, log),
        }
    }

    fn spawn_piped(started: &Started, log: &Arc<Mutex<LogWriter>>) -> std::io::Result<Self> {
        let mut command = Command::new(&started.command);
        command
            .args(&started.args)
            .current_dir(crate::process::child_process_cwd(Path::new(&started.cwd)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = crate::process::owned_process::OwnedProcess::spawn(command)?;
        let missing = || std::io::Error::other("a pipe was not captured");
        let stdin = process.take_stdin().ok_or_else(missing)?;
        let stdout = process.take_stdout().ok_or_else(missing)?;
        let stderr = process.take_stderr().ok_or_else(missing)?;
        Ok(Self {
            pid: process.id(),
            root: Root::Piped(process),
            input: feed(stdin),
            readers: vec![
                copy(stdout, ProcessStream::Stdout, log),
                copy(stderr, ProcessStream::Stderr, log),
            ],
        })
    }

    #[cfg(unix)]
    fn spawn_terminal(
        started: &Started,
        size: TerminalSize,
        log: &Arc<Mutex<LogWriter>>,
    ) -> std::io::Result<Self> {
        let (terminal, child) = super::terminal::Terminal::spawn(started, size)?;
        Ok(Self {
            pid: child.id(),
            input: feed(terminal.writer()?),
            readers: vec![copy(terminal.reader()?, ProcessStream::Terminal, log)],
            root: Root::Terminal(child, terminal),
        })
    }

    #[cfg(not(unix))]
    fn spawn_terminal(
        _started: &Started,
        _size: TerminalSize,
        _log: &Arc<Mutex<LogWriter>>,
    ) -> std::io::Result<Self> {
        Err(std::io::Error::other("a terminal needs a Unix host"))
    }

    /// Run until the child exits, applying what the mailbox brings and
    /// stopping it at `max_seconds`.
    fn run(
        &mut self,
        mailbox: &mut Mailbox,
        max_seconds: u64,
    ) -> (std::io::Result<ExitStatus>, ProcessExitReason) {
        let deadline = Instant::now() + Duration::from_secs(max_seconds);
        let mut stopping: Option<(Instant, ProcessExitReason)> = None;
        let mut written = BTreeSet::new();
        loop {
            match self.try_wait() {
                Ok(Some(status)) => {
                    return (
                        Ok(status),
                        stopping.map_or(ProcessExitReason::Exited, |(_, reason)| reason),
                    );
                }
                Ok(None) => {}
                Err(error) => return (Err(error), ProcessExitReason::Failed),
            }
            for control in mailbox.take().unwrap_or_default() {
                match control {
                    // A write sent again under its call is applied once.
                    Control::Write { call, input } => {
                        if call.is_none_or(|call| written.insert(call)) {
                            let _ = self.input.send(input.into_bytes());
                        }
                    }
                    Control::Resize { rows, cols } => self.resize(TerminalSize { rows, cols }),
                    Control::Stop if stopping.is_none() => {
                        self.signal(ProcessSignal::Terminate);
                        stopping = Some((Instant::now() + STOP_GRACE, ProcessExitReason::Stopped));
                    }
                    Control::Stop => {}
                }
            }
            let now = Instant::now();
            if stopping.is_none() && now >= deadline {
                self.signal(ProcessSignal::Terminate);
                stopping = Some((now + STOP_GRACE, ProcessExitReason::TimedOut));
            }
            if let Some((kill_at, _)) = stopping
                && now >= kill_at
            {
                self.signal(ProcessSignal::Force);
            }
            thread::sleep(TICK);
        }
    }

    /// Once the child has exited, its process group goes with it, and its
    /// output is copied to the end.
    fn finish(self) {
        let Self {
            pid,
            root,
            input,
            readers,
        } = self;
        drop(input);
        crate::process::signal_process_group_id(pid, ProcessSignal::Terminate);
        let copied = |readers: &[JoinHandle<()>], within: Duration| {
            let until = Instant::now() + within;
            while Instant::now() < until {
                if readers.iter().all(JoinHandle::is_finished) {
                    return true;
                }
                thread::sleep(TICK);
            }
            false
        };
        if !copied(&readers, STOP_GRACE) {
            crate::process::signal_process_group_id(pid, ProcessSignal::Force);
            copied(&readers, Duration::from_secs(1));
        }
        drop(root);
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        match &mut self.root {
            Root::Piped(process) => process.wait_timeout(Duration::ZERO),
            #[cfg(unix)]
            Root::Terminal(child, _) => child.try_wait(),
        }
    }

    fn signal(&mut self, signal: ProcessSignal) {
        match &mut self.root {
            Root::Piped(process) => {
                let _ = process.signal(signal);
            }
            #[cfg(unix)]
            Root::Terminal(child, _) => {
                if !crate::process::signal_process_group_id(child.id(), signal) {
                    let _ = child.kill();
                }
            }
        }
    }

    fn resize(&self, size: TerminalSize) {
        #[cfg(unix)]
        if let Root::Terminal(_, terminal) = &self.root {
            let _ = terminal.resize(size);
        }
        #[cfg(not(unix))]
        let _ = size;
    }
}

/// A thread that feeds what arrives to the child's input, so a child that
/// stops reading never blocks its supervisor.
fn feed(mut input: impl Write + Send + 'static) -> mpsc::Sender<Vec<u8>> {
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        for bytes in receiver {
            if input
                .write_all(&bytes)
                .and_then(|()| input.flush())
                .is_err()
            {
                return;
            }
        }
    });
    sender
}

/// A thread that copies one stream of the child's output to the log as it
/// arrives, holding back a character split across reads until it is whole.
fn copy(
    mut output: impl Read + Send + 'static,
    stream: ProcessStream,
    log: &Arc<Mutex<LogWriter>>,
) -> JoinHandle<()> {
    let log = Arc::clone(log);
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut pending = Pending::default();
        loop {
            let count = match output.read(&mut buffer) {
                Ok(0) | Err(_) => 0,
                Ok(count) => count,
            };
            let bytes = match count {
                0 => pending.rest(),
                count => pending.whole(&buffer[..count]),
            };
            if !bytes.is_empty()
                && let Ok(mut log) = log.lock()
            {
                let _ = log.append(stream, &bytes);
            }
            if count == 0 {
                return;
            }
        }
    })
}
