//! The pseudo-terminal a `tty` process runs in. The supervisor keeps the
//! master side and never takes the terminal itself; the child is the runx
//! binary run once more as `runx --runx-terminal <command> [args]` with the
//! terminal as its stdio, which leads a new session whose controlling
//! terminal it is and then becomes the command. So the command owns its
//! terminal from its first instruction: a key such as Ctrl-C reaches it as a
//! signal, and the master reads to its end once the command's side closes.

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use runx_contracts::TerminalSize;
use rustix::fs::OFlags;
use rustix::io::{FdFlags, fcntl_setfd};
use rustix::process::ioctl_tiocsctty;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcsetwinsize};

use super::record::Started;

/// The first argument that runs the binary as a terminal's command.
pub(super) const TERMINAL_FLAG: &str = "--runx-terminal";

/// The terminal's master side, which the supervisor reads and writes.
pub(super) struct Terminal {
    master: File,
    /// The terminal's own side, by name: what a resize sizes.
    name: PathBuf,
}

impl Terminal {
    /// Run `started`'s command in a new terminal of `size`.
    pub(super) fn spawn(started: &Started, size: TerminalSize) -> io::Result<(Self, Child)> {
        let master = File::from(
            openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).map_err(step("opening a terminal"))?,
        );
        // `posix_openpt` takes no close-on-exec flag everywhere; the child
        // must not inherit the master side.
        fcntl_setfd(&master, FdFlags::CLOEXEC).map_err(step("closing it on exec"))?;
        grantpt(&master).map_err(step("granting the terminal"))?;
        unlockpt(&master).map_err(step("unlocking the terminal"))?;
        let name = PathBuf::from(OsStr::from_bytes(
            ptsname(&master, Vec::new())
                .map_err(step("naming the terminal"))?
                .as_bytes(),
        ));
        let terminal = open(&name)?;
        tcsetwinsize(&terminal, winsize(size)).map_err(step("sizing the terminal"))?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg(TERMINAL_FLAG)
            .arg(&started.command)
            .args(&started.args)
            .current_dir(crate::process::child_process_cwd(Path::new(&started.cwd)))
            .stdin(Stdio::from(terminal.try_clone()?))
            .stdout(Stdio::from(terminal.try_clone()?))
            .stderr(Stdio::from(terminal));
        let child = crate::process::with_spawn_lock(|| command.spawn())?;
        Ok((Self { master, name }, child))
    }

    /// The master side, to read the terminal's output from.
    pub(super) fn reader(&self) -> io::Result<File> {
        self.master.try_clone()
    }

    /// The master side, to type into the terminal.
    pub(super) fn writer(&self) -> io::Result<File> {
        self.master.try_clone()
    }

    /// Resize the terminal; its foreground is told with `SIGWINCH`.
    pub(super) fn resize(&self, size: TerminalSize) -> io::Result<()> {
        tcsetwinsize(&open(&self.name)?, winsize(size)).map_err(step("resizing the terminal"))
    }
}

/// Run as a terminal's command: `args` are the command and its arguments,
/// and stdin is the terminal. Leads a new session, takes the terminal as its
/// controlling terminal (its process group then holds the foreground), and
/// becomes the command; returns only when it cannot.
pub(super) fn become_command(args: &[OsString]) -> i32 {
    let Some((program, args)) = args.split_first() else {
        return 64;
    };
    let taken = rustix::process::setsid()
        .map(|_| ())
        .and_then(|()| ioctl_tiocsctty(io::stdin().as_fd()));
    // What fails is written to the terminal, where its log keeps it.
    if let Err(error) = taken {
        let _ = writeln!(
            io::stderr(),
            "runx: taking the terminal: {}",
            io::Error::from(error)
        );
        return 70;
    }
    let error = Command::new(program).args(args).exec();
    let _ = writeln!(io::stderr(), "runx: {}: {error}", program.to_string_lossy());
    127
}

/// The terminal's own side, opened without making it the caller's
/// controlling terminal.
fn open(name: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(OFlags::NOCTTY.bits() as i32)
        .open(name)
}

fn winsize(size: TerminalSize) -> Winsize {
    Winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Name the step of setting up a terminal that failed.
fn step(doing: &'static str) -> impl Fn(rustix::io::Errno) -> io::Error {
    move |errno| {
        let error = io::Error::from(errno);
        io::Error::new(error.kind(), format!("{doing}: {error}"))
    }
}
