//! The always-on runx (the loop contract §15): one per runx home, holding a
//! lock on a file in it, serving every workspace with a started loop. It
//! sleeps until the earliest time anything of theirs is due, and wakes
//! sooner when a command here rings it, on a loopback port, after changing
//! something. A timed wait doesn't count the time a machine sleeps, so it
//! reads the wall clock again at least every minute: a sleeping machine
//! runs nothing, and on waking runx runs what came due, marked late.
//! `runx on` starts it, `runx off` asks it to stop after the pass it's in,
//! `runx status` says what it's doing.

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use fs2::FileExt;
use runx_runtime::WorkspaceEnv;
use serde::{Deserialize, Serialize};

/// The longest it waits before reading the wall clock again.
const MOST_WAIT: StdDuration = StdDuration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlwaysOnPlan {
    /// Start it in the background, or with `foreground`, be it.
    On {
        foreground: bool,
    },
    Off,
    Status,
}

pub fn parse_always_on_plan(args: &[OsString]) -> Result<AlwaysOnPlan, String> {
    let words = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be UTF-8"))
        .collect::<Result<Vec<_>, _>>()?;
    match words.as_slice() {
        ["on"] => Ok(AlwaysOnPlan::On { foreground: false }),
        ["on", "--foreground"] => Ok(AlwaysOnPlan::On { foreground: true }),
        ["off"] => Ok(AlwaysOnPlan::Off),
        ["status"] => Ok(AlwaysOnPlan::Status),
        _ => Err("usage: runx on [--foreground] | runx off | runx status".to_owned()),
    }
}

/// Where the always-on runx keeps its lock, its address, the workspaces it
/// serves and its log, in the runx home.
fn dir(workspace: &WorkspaceEnv) -> PathBuf {
    workspace.paths().home().join("always-on")
}

#[derive(Debug, Serialize, Deserialize)]
struct Serving {
    pid: u32,
    port: u16,
    started_at: String,
}

fn read_serving(dir: &Path) -> Option<Serving> {
    let bytes = std::fs::read(dir.join("state.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether one holds the home's lock now.
fn running(dir: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("lock"))
    else {
        return false;
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            false
        }
        Err(_) => true,
    }
}

/// The workspaces it serves.
fn workspaces(dir: &Path) -> Vec<PathBuf> {
    std::fs::read(dir.join("workspaces.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let staged = path.with_extension("tmp");
    std::fs::write(&staged, bytes)
        .map_err(|error| format!("writing {}: {error}", staged.display()))?;
    std::fs::rename(&staged, path).map_err(|error| format!("writing {}: {error}", path.display()))
}

/// Serve `workspace`'s loops from now on: a started loop's workspace joins
/// the list, and the always-on runx is rung.
pub(crate) fn serve_workspace(workspace: &WorkspaceEnv) -> Result<(), String> {
    let dir = dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|error| format!("making {}: {error}", dir.display()))?;
    let path = workspace
        .cwd()
        .canonicalize()
        .map_err(|error| format!("reading {}: {error}", workspace.cwd().display()))?;
    // Registration is one change at a time: two starts at once never lose
    // one. The home's workspace ids key a repository, every worktree one,
    // while a loop lives where it was started, so the host keeps its own.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("workspaces.lock"))
        .map_err(|error| format!("opening the workspaces' lock: {error}"))?;
    lock.lock_exclusive()
        .map_err(|error| format!("taking the workspaces' lock: {error}"))?;
    let mut served = workspaces(&dir);
    if !served.contains(&path) {
        served.push(path);
        let bytes = serde_json::to_vec_pretty(&served).map_err(|error| error.to_string())?;
        write_atomic(&dir.join("workspaces.json"), &bytes)?;
    }
    drop(lock);
    ring(workspace);
    Ok(())
}

/// Tell the always-on runx something changed, when it's on.
pub(crate) fn ring(workspace: &WorkspaceEnv) {
    send(&dir(workspace), "wake");
}

fn send(dir: &Path, word: &str) -> bool {
    let Some(serving) = read_serving(dir) else {
        return false;
    };
    TcpStream::connect((Ipv4Addr::LOCALHOST, serving.port))
        .and_then(|mut stream| stream.write_all(format!("{word}\n").as_bytes()))
        .is_ok()
}

/// A line saying whether runx is on.
pub(crate) fn status_line(workspace: &WorkspaceEnv) -> String {
    let dir = dir(workspace);
    match read_serving(&dir).filter(|_| running(&dir)) {
        Some(serving) => format!(
            "runx is on (process {}, since {}), serving {} workspace(s)",
            serving.pid,
            serving.started_at,
            workspaces(&dir).len()
        ),
        None => "runx is off: `runx on` runs your loops while this machine is awake".to_owned(),
    }
}

pub fn run_native_always_on(plan: AlwaysOnPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let result = match plan {
        AlwaysOnPlan::On { foreground: true } => serve(workspace),
        AlwaysOnPlan::On { foreground: false } => start(workspace),
        AlwaysOnPlan::Off => stop(workspace),
        AlwaysOnPlan::Status => Ok(status_line(workspace) + "\n"),
    };
    match result {
        Ok(said) => crate::cli_io::write_stdout_code(&said, 0),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx: {error}\n"), 1),
    }
}

/// Start the always-on runx in the background, its output in its log.
fn start(workspace: &WorkspaceEnv) -> Result<String, String> {
    let dir = dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|error| format!("making {}: {error}", dir.display()))?;
    if running(&dir) {
        return Ok(status_line(workspace) + "\n");
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("log"))
        .map_err(|error| format!("opening its log: {error}"))?;
    let program = std::env::current_exe().map_err(|error| format!("finding runx: {error}"))?;
    let mut command = std::process::Command::new(program);
    command
        .args(["on", "--foreground"])
        .current_dir(workspace.cwd())
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|error| error.to_string())?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Its own process group, so the terminal's signals don't end it.
        command.process_group(0);
    }
    command
        .spawn()
        .map_err(|error| format!("starting runx: {error}"))?;
    for _ in 0..50 {
        if read_serving(&dir).is_some() && running(&dir) {
            return Ok(status_line(workspace) + "\n");
        }
        std::thread::sleep(StdDuration::from_millis(100));
    }
    Err(format!(
        "runx didn't come on; its log is {}",
        dir.join("log").display()
    ))
}

fn stop(workspace: &WorkspaceEnv) -> Result<String, String> {
    let dir = dir(workspace);
    if !running(&dir) || !send(&dir, "off") {
        return Ok("runx is off\n".to_owned());
    }
    for _ in 0..600 {
        if !running(&dir) {
            return Ok("runx is off\n".to_owned());
        }
        std::thread::sleep(StdDuration::from_millis(100));
    }
    Ok("runx is finishing the wake it's in, then goes off\n".to_owned())
}

#[derive(Default)]
struct Bell {
    rung: bool,
    off: bool,
}

fn log(line: &str) {
    let _ = crate::cli_io::write_stdout(&format!(
        "{} {line}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
}

/// Be the always-on runx, in this process, until asked to stop.
fn serve(workspace: &WorkspaceEnv) -> Result<String, String> {
    let dir = dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|error| format!("making {}: {error}", dir.display()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("lock"))
        .map_err(|error| format!("opening its lock: {error}"))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(status_line(workspace) + "\n");
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("listening on the loopback: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let serving = Serving {
        pid: std::process::id(),
        port,
        started_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    write_atomic(
        &dir.join("state.json"),
        &serde_json::to_vec_pretty(&serving).map_err(|error| error.to_string())?,
    )?;
    log(&format!("on, process {}, port {port}", serving.pid));
    let bell = Arc::new((Mutex::new(Bell::default()), Condvar::new()));
    let ringer = Arc::clone(&bell);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_err() {
                continue;
            }
            let (state, signal) = &*ringer;
            if let Ok(mut state) = state.lock() {
                state.rung = true;
                state.off |= line.trim() == "off";
            }
            signal.notify_all();
        }
    });
    let binary = Binary::now();
    loop {
        // Each pass runs beside the bell, so `off` is heard while a wake
        // drives: no loop is claimed after it, and a wake still running past
        // the grace is left for recovery.
        let pass = {
            let dir = dir.clone();
            let bell = Arc::clone(&bell);
            std::thread::spawn(move || pass_all(&dir, &bell))
        };
        let Some(next) = finished(pass, &bell)? else {
            break;
        };
        // A new runx binary takes over between passes, so no wake runs on a
        // half-upgraded runx; waiting runs are records it drives on.
        if binary.changed() {
            let _ = std::fs::remove_file(dir.join("state.json"));
            log("a new runx binary is in place: starting it");
            drop(lock);
            return Err(binary.replace());
        }
        let (state, signal) = &*bell;
        let mut held = state.lock().map_err(|_| "its bell broke".to_owned())?;
        if held.off {
            break;
        }
        if !held.rung {
            let wait = next
                .map(|next| (next - Utc::now()).to_std().unwrap_or(StdDuration::ZERO))
                .unwrap_or(MOST_WAIT)
                .min(MOST_WAIT);
            held = signal
                .wait_timeout_while(held, wait, |bell| !bell.rung && !bell.off)
                .map_err(|_| "its bell broke".to_owned())?
                .0;
        }
        if held.off {
            break;
        }
        held.rung = false;
    }
    let _ = std::fs::remove_file(dir.join("state.json"));
    log("off");
    Ok(String::new())
}

/// The runx binary this process runs, as it was on disk when it started.
struct Binary {
    path: Option<PathBuf>,
    stamp: Option<(std::time::SystemTime, u64)>,
}

impl Binary {
    fn stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
        let metadata = std::fs::metadata(path).ok()?;
        Some((metadata.modified().ok()?, metadata.len()))
    }

    fn now() -> Self {
        let path = std::env::current_exe().ok();
        let stamp = path.as_deref().and_then(Self::stamp);
        Self { path, stamp }
    }

    /// Whether the binary on disk is no longer the one this process runs.
    fn changed(&self) -> bool {
        match (&self.path, &self.stamp) {
            (Some(path), Some(stamp)) => Self::stamp(path).is_some_and(|now| now != *stamp),
            _ => false,
        }
    }

    /// Run the new binary in this process's place, as `runx on --foreground`.
    /// Returns only why it couldn't.
    fn replace(&self) -> String {
        let Some(path) = &self.path else {
            return "runx can't find its own binary to start the new one".to_owned();
        };
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            let error = std::process::Command::new(path)
                .args(["on", "--foreground"])
                .exec();
            format!("starting the new runx: {error}")
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            "runx can't take over in place here: run `runx off` and `runx on`".to_owned()
        }
    }
}

/// How long `off` lets the wake a pass is in reach a suspension or its end.
const OFF_GRACE: StdDuration = StdDuration::from_secs(30);

type Ringer = Arc<(Mutex<Bell>, Condvar)>;

/// Whether `off` was asked for.
fn off(bell: &Ringer) -> bool {
    bell.0.lock().map_or(true, |state| state.off)
}

/// The pass's earliest next time once it finishes, or none when `off` came
/// first: the pass then gets its grace, and a wake still running after it is
/// left for recovery from its record.
fn finished(
    pass: std::thread::JoinHandle<Option<DateTime<Utc>>>,
    bell: &Ringer,
) -> Result<Option<Option<DateTime<Utc>>>, String> {
    let (state, signal) = &**bell;
    loop {
        if pass.is_finished() {
            return pass
                .join()
                .map(Some)
                .map_err(|_| "a pass panicked".to_owned());
        }
        let held = state.lock().map_err(|_| "its bell broke".to_owned())?;
        if held.off {
            drop(held);
            let deadline = std::time::Instant::now() + OFF_GRACE;
            while !pass.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(StdDuration::from_millis(100));
            }
            if !pass.is_finished() {
                log("off: a wake still running is left for recovery");
            }
            return Ok(None);
        }
        let _ = signal
            .wait_timeout_while(held, StdDuration::from_millis(250), |bell| !bell.off)
            .map_err(|_| "its bell broke".to_owned())?;
    }
}

/// One pass over every loop of every workspace served, and the earliest time
/// anything is next due. `off` stops it claiming another loop.
fn pass_all(dir: &Path, bell: &Ringer) -> Option<DateTime<Utc>> {
    let mut next: Option<DateTime<Utc>> = None;
    for path in workspaces(dir) {
        if off(bell) {
            return next;
        }
        // Each workspace where it is: never the location this process was
        // started with.
        let workspace = match WorkspaceEnv::load_located(path.clone()) {
            Ok(workspace) => workspace,
            Err(error) => {
                log(&format!("{}: {error}", path.display()));
                continue;
            }
        };
        let loops = match crate::loops::open(&workspace) {
            Ok(loops) => loops,
            Err(error) => {
                log(&format!("{}: {error}", path.display()));
                continue;
            }
        };
        let names = match loops.names() {
            Ok(names) => names,
            Err(error) => {
                log(&format!("{}: {error}", path.display()));
                continue;
            }
        };
        for name in names {
            if off(bell) {
                return next;
            }
            match loops.pass(&name, Utc::now()) {
                Ok(passed) => {
                    if let Some(run) = &passed.woke {
                        log(&format!("{name} woke: {run}"));
                    }
                    for (run, ending) in &passed.settled {
                        log(&format!("{name} settled {run}: {}", ending.outcome()));
                    }
                    if let Some(why) = &passed.couldnt {
                        log(&format!("{name} couldn't run: {why}"));
                    }
                    if let Some(at) = passed.next {
                        next = Some(next.map_or(at, |next| next.min(at)));
                    }
                }
                Err(error) => log(&format!("{name}: {error}")),
            }
        }
    }
    next
}
