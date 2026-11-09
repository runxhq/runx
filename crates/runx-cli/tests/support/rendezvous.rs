//! A point a test's script reaches, and the test waits on, with no clock.
//!
//! The script writes one line to a named pipe when it gets there, and reads
//! a line from a gate that holds it there until the test lets it go. The test
//! holds both ends of each pipe open until the process it watches exits, so
//! no open of a pipe waits on its other side, and a wait on a script that
//! stopped short ends when its process does, never on time.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

/// A named pipe a script writes or reads.
pub struct Pipe {
    path: PathBuf,
}

impl Pipe {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let made = Command::new("mkfifo").arg(&path).status()?;
        if !made.success() {
            return Err(io::Error::other(format!(
                "mkfifo {} failed",
                path.display()
            )));
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The line the script writes to the pipe; none when the process `pid`
    /// exits before it writes one.
    pub fn reached(&self, pid: u32) -> io::Result<Option<String>> {
        self.watch(pid)?.line()
    }

    /// The pipe's read end, for a script that writes it a line on getting
    /// somewhere and may hold it open for the rest of its life.
    pub fn watch(&self, pid: u32) -> io::Result<Watched> {
        let (reader, writer) = self.both_ends()?;
        let ours = Arc::new(Mutex::new(Some(writer)));
        when_exited(pid, {
            let ours = Arc::clone(&ours);
            move || drop(take(&ours))
        });
        Ok(Watched {
            reader: BufReader::new(reader),
            ours,
        })
    }

    /// Let a script waiting on this gate, or yet to reach it, go on by
    /// writing it a line. Never waits.
    pub fn release(&self, pid: u32) -> io::Result<()> {
        let (reader, mut writer) = self.both_ends()?;
        writer.write_all(b"go\n")?;
        when_exited(pid, move || drop((reader, writer)));
        Ok(())
    }

    /// Both ends of the pipe, opened without waiting on each other; the
    /// reader blocks on its reads.
    fn both_ends(&self) -> io::Result<(fs::File, fs::File)> {
        let nonblocking = rustix::fs::OFlags::NONBLOCK;
        let reader = OpenOptions::new()
            .read(true)
            .custom_flags(nonblocking.bits() as i32)
            .open(&self.path)?;
        rustix::fs::fcntl_setfl(&reader, rustix::fs::fcntl_getfl(&reader)? - nonblocking)?;
        let writer = OpenOptions::new()
            .write(true)
            .custom_flags(nonblocking.bits() as i32)
            .open(&self.path)?;
        Ok((reader, writer))
    }
}

/// A pipe's read end. Until [`Watched::rest`], the test also holds a write
/// end, until the watched process exits, so a script's open of the pipe
/// never waits and a script that stopped short leaves no read waiting.
pub struct Watched {
    reader: BufReader<fs::File>,
    ours: Arc<Mutex<Option<fs::File>>>,
}

impl Watched {
    /// The next line the script writes; none when the watched process exits
    /// before it writes one.
    pub fn line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        Ok((!line.is_empty()).then_some(line))
    }

    /// The rest of what the pipe is written, once every process holding it
    /// open has closed it, as a process does when it dies. The test lets its
    /// own write end go first.
    pub fn rest(mut self) -> io::Result<String> {
        drop(take(&self.ours));
        let mut rest = String::new();
        self.reader.read_to_string(&mut rest)?;
        Ok(rest)
    }
}

fn take(slot: &Mutex<Option<fs::File>>) -> Option<fs::File> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// Do `then` once the process `pid` exits.
fn when_exited(pid: u32, then: impl FnOnce() + Send + 'static) {
    thread::spawn(move || {
        exited(pid);
        then();
    });
}

/// Blocks until `pid` exits, leaving it to be reaped by its own wait.
pub fn exited(pid: u32) {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return;
    };
    let _ = rustix::process::waitid(
        rustix::process::WaitId::Pid(pid),
        rustix::process::WaitIdOptions::EXITED | rustix::process::WaitIdOptions::NOWAIT,
    );
}
