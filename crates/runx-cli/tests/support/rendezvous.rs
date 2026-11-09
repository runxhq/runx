//! A point a test's script reaches, and the test waits on, with no clock.
//!
//! The script writes one line to a named pipe when it gets there, and reads
//! a line from a gate that holds it there until the test lets it go. The test
//! holds both ends of each pipe open until the process it watches exits, so
//! no open of a pipe waits on its other side, and a wait on a script that
//! stopped short ends when its process does, never on time.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
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
        let (reader, writer) = self.both_ends()?;
        hold_until_exited(pid, writer);
        let mut line = String::new();
        BufReader::new(reader).read_line(&mut line)?;
        Ok((!line.is_empty()).then_some(line))
    }

    /// Let a script waiting on this gate, or yet to reach it, go on by
    /// writing it a line. Never waits.
    pub fn release(&self, pid: u32) -> io::Result<()> {
        let (reader, mut writer) = self.both_ends()?;
        writer.write_all(b"go\n")?;
        hold_until_exited(pid, (reader, writer));
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

/// Keep `held` open until the process `pid` exits.
fn hold_until_exited<T: Send + 'static>(pid: u32, held: T) {
    thread::spawn(move || {
        exited(pid);
        drop(held);
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
