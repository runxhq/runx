//! The always-on runx (the loop contract §15): one per runx home, holding a
//! lock on a file in it, serving every workspace with a started loop, each
//! where it lies and each loop on its own, so one's long wake never holds
//! another's due work. It sleeps until the earliest time anything is due,
//! and wakes sooner when a command here rings it, on a loopback port, after
//! changing something. A timed wait doesn't count the time a machine sleeps,
//! so it reads the wall clock again at least every minute: a sleeping
//! machine runs nothing, and on waking runx runs what came due, marked late.
//! Each feed a loop takes events from is read here on its own thread, held
//! open as long as its provider allows, and what arrives marks that loop due
//! (the loop contract's E4). Its port, kept in the home so its address stays
//! the same, also takes each loop's webhooks (§5.2): a request carrying a
//! loop's token puts its payload in that loop's inbox before it's answered.
//! `runx on` starts it, and asks once whether it comes on at login
//! (`at_login`); `runx off` asks it to stop, after which no loop claims a
//! wake and a wake in hand has a grace to end; `runx status` says what it's
//! doing. While it's on it keeps the configured local model up
//! (`local_model`).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use fs2::FileExt;
use runx_loops::{Feed, LoopName};
use runx_runtime::{WorkspaceEnv, WorkspaceLocation};
use serde::{Deserialize, Serialize};

/// The longest it waits before reading the wall clock again.
const MOST_WAIT: StdDuration = StdDuration::from_secs(60);
/// How long a feed's read may be held open while nothing arrives: within
/// the time a provider's call is given.
const FEED_WAIT: StdDuration = StdDuration::from_secs(50);
/// The least time between a feed's reads that found nothing, for a provider
/// that answers at once rather than holding the read.
const FEED_PACE: StdDuration = StdDuration::from_secs(30);
/// How long a feed that failed waits before it's read again, at first and
/// at most, doubling between.
const FEED_RETRY: (StdDuration, StdDuration) =
    (StdDuration::from_secs(30), StdDuration::from_secs(600));

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlwaysOnPlan {
    /// Start it in the background, or with `foreground`, be it; with
    /// `at_login`, whether it comes on at login too.
    On {
        foreground: bool,
        at_login: Option<bool>,
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
        ["on"] => Ok(AlwaysOnPlan::On {
            foreground: false,
            at_login: None,
        }),
        ["on", "--foreground"] => Ok(AlwaysOnPlan::On {
            foreground: true,
            at_login: None,
        }),
        ["on", "--at-login"] => Ok(AlwaysOnPlan::On {
            foreground: false,
            at_login: Some(true),
        }),
        ["on", "--not-at-login"] => Ok(AlwaysOnPlan::On {
            foreground: false,
            at_login: Some(false),
        }),
        ["off"] => Ok(AlwaysOnPlan::Off),
        ["status"] => Ok(AlwaysOnPlan::Status),
        _ => Err(
            "usage: runx on [--foreground | --at-login | --not-at-login] | runx off | runx status"
                .to_owned(),
        ),
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

/// The workspaces it serves, each where it lies.
fn workspaces(dir: &Path) -> Vec<WorkspaceLocation> {
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
    // Where it lies, its own state and receipt roots among it, so the host
    // loads it there and never where the host was started.
    let mut location = workspace.location();
    location.workspace = location
        .workspace
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
    if !served.contains(&location) {
        served.push(location);
        let bytes = serde_json::to_vec_pretty(&served).map_err(|error| error.to_string())?;
        write_atomic(&dir.join("workspaces.json"), &bytes)?;
    }
    drop(lock);
    ring(workspace);
    Ok(())
}

/// Tell the always-on runx something changed, when it's on.
pub(crate) fn ring(workspace: &WorkspaceEnv) {
    send(&dir(workspace), "ring");
}

/// Ask the always-on runx to `word`, `ring` or `off`, on its port.
fn send(dir: &Path, word: &str) -> bool {
    let Some(serving) = read_serving(dir) else {
        return false;
    };
    TcpStream::connect((Ipv4Addr::LOCALHOST, serving.port))
        .and_then(|mut stream| {
            stream.write_all(
                format!("POST /{word} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n")
                    .as_bytes(),
            )
        })
        .is_ok()
}

/// The address loop webhooks are taken at: `http://127.0.0.1:<port>`, once
/// the always-on runx has listened.
pub(crate) fn address(workspace: &WorkspaceEnv) -> Option<String> {
    kept_port(&dir(workspace)).map(|port| format!("http://127.0.0.1:{port}"))
}

/// The port the always-on runx listens on, kept from the first time it did.
fn kept_port(dir: &Path) -> Option<u16> {
    std::fs::read_to_string(dir.join("port"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Listen on the loopback at the port kept in `dir`, or, the first time, at
/// one the system gives, kept from then on: the address a tunnel in front of
/// a webhook, or a paired browser, knows stays the same.
fn listen(dir: &Path) -> Result<TcpListener, String> {
    if let Some(port) = kept_port(dir) {
        return TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
            format!(
                "listening on the loopback at its port {port}: {error}. Free the port, or remove {} for a new one, which changes the address its webhooks are sent to",
                dir.join("port").display()
            )
        });
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("listening on the loopback: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    write_atomic(&dir.join("port"), port.to_string().as_bytes())?;
    Ok(listener)
}

/// This runx home's login unit, on a system runx writes one for.
fn login_unit(workspace: &WorkspaceEnv) -> Option<crate::at_login::Unit> {
    let os = crate::at_login::Os::current()?;
    let user = crate::at_login::UserDirs::from_env(workspace.env())?;
    let binary = std::env::current_exe().ok()?;
    Some(crate::at_login::unit(
        os,
        &user,
        workspace.paths().home(),
        &binary,
    ))
}

/// A line saying whether runx is on, and when it is, how the local model it
/// keeps up stands; then whether it comes on at login.
pub(crate) fn status_line(workspace: &WorkspaceEnv) -> String {
    let dir = dir(workspace);
    let on = match read_serving(&dir).filter(|_| running(&dir)) {
        Some(serving) => {
            let model = crate::local_model::read_state(&dir)
                .and_then(|state| crate::local_model::status_line(&state))
                .map(|line| format!("\n{line}"))
                .unwrap_or_default();
            format!(
                "runx is on (process {}, since {}), serving {} workspace(s){model}",
                serving.pid,
                serving.started_at,
                workspaces(&dir).len()
            )
        }
        None => "runx is off: `runx on` runs your loops while this machine is awake".to_owned(),
    };
    let binary = std::env::current_exe().unwrap_or_default();
    let troubles = troubles(workspace)
        .iter()
        .map(|trouble| format!("\n  {}", trouble.line()))
        .collect::<String>();
    format!(
        "{on}{troubles}\n{}",
        crate::at_login::status_line(&dir, login_unit(workspace).as_ref(), &binary)
    )
}

pub fn run_native_always_on(plan: AlwaysOnPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let result = match plan {
        AlwaysOnPlan::On {
            foreground: true, ..
        } => serve(workspace),
        AlwaysOnPlan::On {
            foreground: false,
            at_login,
        } => start(workspace, at_login),
        AlwaysOnPlan::Off => stop(workspace),
        AlwaysOnPlan::Status => Ok(status_line(workspace) + "\n"),
    };
    match result {
        Ok(said) => crate::cli_io::write_stdout_code(&said, 0),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx: {error}\n"), 1),
    }
}

/// Start the always-on runx in the background, its output in its log, after
/// settling whether it comes on at login.
fn start(workspace: &WorkspaceEnv, at_login: Option<bool>) -> Result<String, String> {
    let dir = dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|error| format!("making {}: {error}", dir.display()))?;
    let login = crate::at_login::settle(&dir, at_login, login_unit(workspace))?
        .map(|line| format!("{line}\n"))
        .unwrap_or_default();
    if running(&dir) {
        return Ok(login + &status_line(workspace) + "\n");
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
            return Ok(login + &status_line(workspace) + "\n");
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
    /// A command here changed something: every loop not in a pass is passed.
    rung: bool,
    /// A loop's pass ended: what it found is taken.
    ended: bool,
    /// Loops a feed put events in the inbox of: each is passed.
    due: BTreeSet<Key>,
    off: bool,
    /// What it couldn't do, until it does, by workspace, loop and feed,
    /// kept in `troubles.json` in its directory as it changes.
    troubles: BTreeMap<(String, Option<String>, Option<String>), Trouble>,
    home: PathBuf,
}

/// What the always-on runx couldn't do for a workspace, one of its loops
/// or a loop's feed, and why, until it does: `runx status` and `runx loops`
/// show it, so nothing it can't do is in its log alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Trouble {
    pub workspace: PathBuf,
    #[serde(rename = "loop", default, skip_serializing_if = "Option::is_none")]
    pub loop_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed: Option<String>,
    pub why: String,
    pub since: String,
}

impl Trouble {
    /// The trouble as a line of `runx status`.
    fn line(&self) -> String {
        let workspace = self.workspace.display();
        let since = &self.since;
        match (&self.loop_name, &self.feed) {
            (Some(name), Some(feed)) => format!(
                "{name} in {workspace} can't read feed {feed} (since {since}): {}",
                self.why
            ),
            (Some(name), None) => format!(
                "{name} in {workspace} can't be served (since {since}): {}",
                self.why
            ),
            _ => format!("{workspace} can't be served (since {since}): {}", self.why),
        }
    }
}

/// Say what the host couldn't do for `workspace` (and `name` and `feed`),
/// or, with no `why`, that it can again.
fn troubled(
    bell: &Ringer,
    workspace: &Path,
    name: Option<&LoopName>,
    feed: Option<&Feed>,
    why: Option<String>,
) {
    let Ok(mut state) = bell.0.lock() else {
        return;
    };
    let key = (
        workspace.display().to_string(),
        name.map(ToString::to_string),
        feed.map(ToString::to_string),
    );
    let changed = match why {
        Some(why) => {
            let since = state.troubles.get(&key).map_or_else(
                || Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                |held| held.since.clone(),
            );
            let trouble = Trouble {
                workspace: workspace.to_path_buf(),
                loop_name: key.1.clone(),
                feed: key.2.clone(),
                why,
                since,
            };
            state.troubles.insert(key, trouble.clone()) != Some(trouble)
        }
        None => state.troubles.remove(&key).is_some(),
    };
    if changed {
        let troubles = state.troubles.values().collect::<Vec<_>>();
        if let Ok(bytes) = serde_json::to_vec_pretty(&troubles) {
            let _ = write_atomic(&state.home.join(TROUBLES), &bytes);
        }
    }
}

const TROUBLES: &str = "troubles.json";

/// What the always-on runx serving `workspace` can't do now, when it's on.
pub(crate) fn troubles(workspace: &WorkspaceEnv) -> Vec<Trouble> {
    let dir = dir(workspace);
    if !running(&dir) {
        return Vec::new();
    }
    std::fs::read(dir.join(TROUBLES))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
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
    let listener = listen(&dir)?;
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
    let mut model = Some(crate::local_model::Keeper::start(
        &dir,
        workspace.env().clone(),
        workspace.cwd().to_path_buf(),
    ));
    // A host starting knows of no trouble until it meets one.
    let _ = std::fs::remove_file(dir.join(TROUBLES));
    let bell = Arc::new((
        Mutex::new(Bell {
            home: dir.clone(),
            ..Bell::default()
        }),
        Condvar::new(),
    ));
    let ringer = Arc::clone(&bell);
    let home = dir.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (bell, dir) = (Arc::clone(&ringer), home.clone());
            std::thread::spawn(move || answer(stream, &bell, &dir));
        }
    });
    let binary = Binary::now();
    let mut served: BTreeMap<Key, Served> = BTreeMap::new();
    // Each feed being read, by its loop, and the word that stops it.
    let mut readers: BTreeMap<(Key, Feed), Arc<AtomicBool>> = BTreeMap::new();
    // What each loop's pass found, sent before it rings the bell.
    let (found, ended) = std::sync::mpsc::channel::<(Key, Option<DateTime<Utc>>)>();
    let take_ended = |served: &mut BTreeMap<Key, Served>| {
        let now = Utc::now();
        for (key, next) in ended.try_iter() {
            if let Some(entry) = served.get_mut(&key) {
                entry.ended(next, now);
            }
        }
    };
    let in_pass = |served: &BTreeMap<Key, Served>| served.values().any(|entry| entry.in_pass);
    let mut rung = true;
    loop {
        take_ended(&mut served);
        if off(&bell) {
            let deadline = std::time::Instant::now() + OFF_GRACE;
            while in_pass(&served) && std::time::Instant::now() < deadline {
                std::thread::sleep(StdDuration::from_millis(100));
                take_ended(&mut served);
            }
            if in_pass(&served) {
                log("off: a wake still running is left for recovery");
            }
            break;
        }
        // A new runx binary takes over once no pass is in hand, so no wake
        // runs on a half-upgraded runx; waiting runs are records it drives on.
        let upgrading = binary.changed();
        if upgrading && !in_pass(&served) {
            let _ = std::fs::remove_file(dir.join("state.json"));
            log("a new runx binary is in place: starting it");
            // The model runx serves stops, and the new runx starts it.
            if let Some(model) = model.take() {
                model.stop();
            }
            drop(lock);
            return Err(binary.replace());
        }
        if !upgrading {
            let now = Utc::now();
            let listed = listed(&dir, &bell);
            served.retain(|key, entry| entry.in_pass || listed.contains_key(key));
            // Each feed a served loop takes events from is read on its own;
            // a feed no loop takes any longer stops after its read.
            let feeding = listed
                .iter()
                .flat_map(|(key, feeds)| feeds.iter().map(|feed| (key.clone(), feed.clone())))
                .collect::<BTreeSet<_>>();
            readers.retain(|reading, stop: &mut Arc<AtomicBool>| {
                let keep = feeding.contains(reading);
                if !keep {
                    stop.store(true, Ordering::Relaxed);
                }
                keep
            });
            for reading in feeding {
                if !readers.contains_key(&reading) {
                    let stop = Arc::new(AtomicBool::new(false));
                    readers.insert(reading.clone(), Arc::clone(&stop));
                    read_feed(reading, Arc::clone(&bell), stop);
                }
            }
            // Each loop on its own: one's long wake never holds another's
            // due work. A loop not yet passed is due now.
            for key in listed.into_keys() {
                let entry = served.entry(key.clone()).or_insert(Served {
                    in_pass: false,
                    next: now,
                });
                if !entry.in_pass && (rung || entry.next <= now) {
                    entry.in_pass = true;
                    pass(key, Arc::clone(&bell), found.clone());
                }
            }
        }
        let earliest = served
            .values()
            .filter(|entry| !entry.in_pass)
            .map(|entry| entry.next)
            .min();
        let wait = earliest
            .map(|next| (next - Utc::now()).to_std().unwrap_or(StdDuration::ZERO))
            .unwrap_or(MOST_WAIT)
            .min(MOST_WAIT);
        let (state, signal) = &*bell;
        let held = state.lock().map_err(|_| "its bell broke".to_owned())?;
        let mut held = signal
            .wait_timeout_while(held, wait, |bell| {
                !bell.rung && !bell.ended && !bell.off && bell.due.is_empty()
            })
            .map_err(|_| "its bell broke".to_owned())?
            .0;
        rung = std::mem::take(&mut held.rung);
        held.ended = false;
        let now = Utc::now();
        for key in std::mem::take(&mut held.due) {
            if let Some(entry) = served.get_mut(&key) {
                entry.next = entry.next.min(now);
            }
        }
    }
    if let Some(model) = model.take() {
        model.stop();
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

/// A loop the host serves, by where its workspace lies and its name.
type Key = (WorkspaceLocation, LoopName);

/// A loop the host serves: whether a pass of it is in hand, and when it's
/// next due.
struct Served {
    in_pass: bool,
    next: DateTime<Utc>,
}

impl Served {
    /// Its pass ended, finding `next`: when nothing is next, or the pass
    /// failed, it's looked at again within the longest wait.
    fn ended(&mut self, next: Option<DateTime<Utc>>, now: DateTime<Utc>) {
        self.in_pass = false;
        self.next = next.unwrap_or_else(|| {
            now + chrono::Duration::from_std(MOST_WAIT).unwrap_or(chrono::Duration::MAX)
        });
    }
}

/// Every loop of every workspace served, by where it lies, with the feeds
/// it takes events from.
fn listed(dir: &Path, bell: &Ringer) -> BTreeMap<Key, Vec<Feed>> {
    let mut listed = BTreeMap::new();
    for location in workspaces(dir) {
        let loops = WorkspaceEnv::load_located(&location)
            .map_err(|error| error.to_string())
            .and_then(|workspace| crate::loops::open(&workspace));
        let found = loops.and_then(|loops| {
            loops
                .names()
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|name| {
                    let feeds = loops.feeds(&name).map_err(|error| error.to_string())?;
                    Ok(((location.clone(), name), feeds))
                })
                .collect::<Result<Vec<_>, String>>()
        });
        match found {
            Ok(found) => {
                troubled(bell, &location.workspace, None, None, None);
                listed.extend(found);
            }
            Err(error) => {
                log(&format!("{}: {error}", location.workspace.display()));
                troubled(bell, &location.workspace, None, None, Some(error));
            }
        }
    }
    listed
}

/// Read one loop's feed on its own thread until `stop` or `off`: each page
/// held open as long as its provider allows, the loop marked due when one
/// put events in its inbox, a page that found nothing followed by the next
/// no sooner than the pace, and a read that failed tried again later, said
/// once until it reads again.
fn read_feed(reading: (Key, Feed), bell: Ringer, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let ((location, name), feed) = &reading;
        let stopped = || stop.load(Ordering::Relaxed) || off(&bell);
        let mut loops = None;
        let mut failing: Option<(String, StdDuration)> = None;
        while !stopped() {
            let started = std::time::Instant::now();
            let read = match loops.take() {
                Some(loops) => Ok(loops),
                None => WorkspaceEnv::load_located(location)
                    .map_err(|error| error.to_string())
                    .and_then(|workspace| crate::loops::open(&workspace)),
            }
            .and_then(|opened: runx_loops::Loops| {
                let polled = opened
                    .poll(name, feed, FEED_WAIT)
                    .map_err(|error| error.to_string());
                loops = Some(opened);
                polled
            });
            match read {
                Ok(polled) => {
                    if failing.take().is_some() {
                        log(&format!("{name} reads feed {feed} again"));
                        troubled(&bell, &location.workspace, Some(name), Some(feed), None);
                    }
                    if polled.taken > 0 {
                        log(&format!("{name}: {} new from feed {feed}", polled.taken));
                        let (state, signal) = &*bell;
                        if let Ok(mut state) = state.lock() {
                            state.due.insert((location.clone(), name.clone()));
                        }
                        signal.notify_all();
                    }
                    if !polled.more {
                        pause(FEED_PACE.saturating_sub(started.elapsed()), &stopped);
                    }
                }
                Err(why) => {
                    let wait = match &failing {
                        Some((said, wait)) if *said == why => (*wait * 2).min(FEED_RETRY.1),
                        _ => {
                            log(&format!("{name} couldn't read feed {feed}: {why}"));
                            troubled(
                                &bell,
                                &location.workspace,
                                Some(name),
                                Some(feed),
                                Some(why.clone()),
                            );
                            FEED_RETRY.0
                        }
                    };
                    failing = Some((why, wait));
                    loops = None;
                    pause(wait, &stopped);
                }
            }
        }
    });
}

/// Wait `wait`, or less when `stopped` says so.
fn pause(wait: StdDuration, stopped: &dyn Fn() -> bool) {
    let until = std::time::Instant::now() + wait;
    while !stopped() {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::sleep(left.min(StdDuration::from_secs(1)));
    }
}

/// One pass over one loop, on its own thread: what it found, its earliest
/// next time, is sent, then the bell rung. Going off, it claims no new wake.
fn pass(key: Key, bell: Ringer, found: std::sync::mpsc::Sender<(Key, Option<DateTime<Utc>>)>) {
    std::thread::spawn(move || {
        let next = pass_loop(&key.0, &key.1, &bell);
        let _ = found.send((key, next));
        let (state, signal) = &*bell;
        if let Ok(mut state) = state.lock() {
            state.ended = true;
        }
        signal.notify_all();
    });
}

fn pass_loop(
    location: &WorkspaceLocation,
    name: &LoopName,
    bell: &Ringer,
) -> Option<DateTime<Utc>> {
    let going_off = {
        let bell = Arc::clone(bell);
        Arc::new(move || off(&bell))
    };
    let loops = WorkspaceEnv::load_located(location)
        .map_err(|error| error.to_string())
        .and_then(|workspace| crate::loops::open(&workspace))
        .map(|loops| loops.served(going_off));
    let passed = match loops {
        Ok(loops) => loops
            .pass(name, Utc::now())
            .map_err(|error| error.to_string()),
        Err(error) => Err(error),
    };
    match passed {
        Ok(passed) => {
            troubled(bell, &location.workspace, Some(name), None, None);
            if let Some(run) = &passed.woke {
                log(&format!("{name} woke: {run}"));
            }
            for (run, ending) in &passed.settled {
                log(&format!("{name} settled {run}: {}", ending.outcome()));
            }
            if let Some(why) = &passed.couldnt {
                log(&format!("{name} couldn't run: {why}"));
            }
            passed.next
        }
        Err(error) => {
            log(&format!("{name}: {error}"));
            troubled(bell, &location.workspace, Some(name), None, Some(error));
            None
        }
    }
}

/// The most a request to the always-on runx may carry.
const BODY_MOST: usize = 1 << 20;

/// One request on the always-on runx's port: `ring` and `off` from a command
/// here, or a webhook's payload, put in its loop's inbox before it's
/// answered. Anything else is answered 404.
fn answer(mut stream: TcpStream, bell: &Ringer, dir: &Path) {
    let _ = stream.set_read_timeout(Some(StdDuration::from_secs(10)));
    let (status, said) = match request(&stream) {
        Ok((method, path, headers, body)) => route(&method, &path, &headers, &body, bell, dir),
        Err(why) => (400, why),
    };
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let _ = stream.write_all(
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{said}",
            said.len()
        )
        .as_bytes(),
    );
}

/// A request's method, path, headers and body, the body no longer than its
/// Content-Length says nor than [`BODY_MOST`].
type Request = (String, String, Vec<(String, String)>, Vec<u8>);

fn request(stream: &TcpStream) -> Result<Request, String> {
    let mut reader = BufReader::new(stream.take(64 * 1024 + BODY_MOST as u64));
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| format!("reading the request: {error}"))?;
    let mut words = line.split_whitespace();
    let (Some(method), Some(path), Some(version)) = (words.next(), words.next(), words.next())
    else {
        return Err("a request starts `<method> <path> HTTP/1.1`\n".to_owned());
    };
    if !version.starts_with("HTTP/1.") {
        return Err("a request is HTTP/1.1\n".to_owned());
    }
    let mut length = 0_usize;
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .map_err(|error| format!("reading the request's headers: {error}\n"))?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value
                    .trim()
                    .parse()
                    .map_err(|_| "its Content-Length isn't a number\n".to_owned())?;
            }
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    if length > BODY_MOST {
        return Err(format!("a request carries at most {BODY_MOST} bytes\n"));
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("reading the request's body: {error}\n"))?;
    Ok((method.to_owned(), path.to_owned(), headers, body))
}

fn route(
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
    bell: &Ringer,
    dir: &Path,
) -> (u16, String) {
    if method != "POST" {
        return (405, "only POST is taken here\n".to_owned());
    }
    let (state, signal) = &**bell;
    if path == "/ring" || path == "/off" {
        if let Ok(mut state) = state.lock() {
            state.rung = true;
            state.off |= path == "/off";
        }
        signal.notify_all();
        return (204, String::new());
    }
    let Some(token) = path
        .strip_prefix("/webhook/")
        .filter(|token| !token.is_empty())
    else {
        return (404, "nothing here\n".to_owned());
    };
    let token_digest = runx_loops::digest(token.as_bytes());
    for location in workspaces(dir) {
        let taken = WorkspaceEnv::load_located(&location)
            .map_err(|error| error.to_string())
            .and_then(|workspace| crate::loops::open(&workspace))
            .and_then(|loops| {
                let Some((name, webhook)) = loops
                    .webhook_of(&token_digest)
                    .map_err(|error| error.to_string())?
                else {
                    return Ok(None);
                };
                let new = loops
                    .take_webhook(&name, &webhook, headers, body)
                    .map_err(|error| error.to_string())?;
                Ok(Some((name, webhook, new)))
            });
        match taken {
            Ok(Some((name, webhook, true))) => {
                log(&format!("{name}: a request on webhook {webhook}"));
                if let Ok(mut state) = state.lock() {
                    state.due.insert((location, name));
                }
                signal.notify_all();
                return (202, "taken\n".to_owned());
            }
            Ok(Some((_, _, false))) => return (200, "taken before\n".to_owned()),
            Ok(None) => {}
            Err(why) => return (400, format!("{why}\n")),
        }
    }
    (
        404,
        "no loop here takes requests with that token\n".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::sync::{Arc, Condvar, Mutex};

    use super::{Bell, Ringer, Trouble, answer};

    type Tested<T = ()> = Result<T, Box<dyn std::error::Error>>;

    /// What the host can't do is in its directory, said once with the time
    /// it began however often it's met again, and gone once it works, so
    /// `runx status` and `runx loops` show it, never its log alone.
    #[test]
    fn what_the_host_can_t_do_is_kept_until_it_can() -> Tested {
        let home = tempfile::tempdir()?;
        let bell: Ringer = Arc::new((
            Mutex::new(Bell {
                home: home.path().to_path_buf(),
                ..Bell::default()
            }),
            Condvar::new(),
        ));
        let workspace = std::path::Path::new("/work/acme");
        let name = runx_loops::LoopName::new("outreach")?;
        let feed = runx_loops::Feed {
            connector: "mail".to_owned(),
            feed: "events".to_owned(),
        };
        let kept = || -> Tested<Vec<Trouble>> {
            Ok(serde_json::from_slice(&std::fs::read(
                home.path().join(super::TROUBLES),
            )?)?)
        };
        super::troubled(
            &bell,
            workspace,
            Some(&name),
            Some(&feed),
            Some("no term covers it".to_owned()),
        );
        let first = kept()?;
        assert_eq!(first.len(), 1);
        assert_eq!(
            first[0].line(),
            format!(
                "outreach in /work/acme can't read feed mail/events (since {}): no term covers it",
                first[0].since
            )
        );
        super::troubled(
            &bell,
            workspace,
            Some(&name),
            Some(&feed),
            Some("no term covers it".to_owned()),
        );
        assert_eq!(
            kept()?,
            first,
            "met again, it's the one trouble since it began"
        );
        super::troubled(
            &bell,
            workspace,
            Some(&name),
            None,
            Some("its store is locked".to_owned()),
        );
        assert_eq!(kept()?.len(), 2);
        super::troubled(&bell, workspace, Some(&name), Some(&feed), None);
        super::troubled(&bell, workspace, Some(&name), None, None);
        assert!(kept()?.is_empty(), "what works again is no trouble");
        Ok(())
    }

    /// What the always-on runx answers `request`, sent to it over the
    /// loopback, with the home `dir` serving no workspace.
    fn answered(request: &[u8], bell: &Ringer, dir: &std::path::Path) -> Tested<String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
        client.write_all(request)?;
        let (server, _) = listener.accept()?;
        answer(server, bell, dir);
        let mut said = String::new();
        client.read_to_string(&mut said)?;
        Ok(said)
    }

    /// The bell as the requests left it: rung, and off.
    fn rung(bell: &Ringer) -> Tested<(bool, bool)> {
        let state = bell.0.lock().map_err(|_| "the bell broke")?;
        Ok((state.rung, state.off))
    }

    #[test]
    fn its_port_takes_its_bell_and_refuses_what_it_doesn_t_serve() -> Tested {
        let home = tempfile::tempdir()?;
        let bell: Ringer = Arc::new((Mutex::new(Bell::default()), Condvar::new()));
        let ring = answered(
            b"POST /ring HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n",
            &bell,
            home.path(),
        )?;
        assert!(ring.starts_with("HTTP/1.1 204 "), "{ring}");
        assert_eq!(rung(&bell)?, (true, false));
        answered(
            b"POST /off HTTP/1.1\r\nContent-Length: 0\r\n\r\n",
            &bell,
            home.path(),
        )?;
        assert_eq!(rung(&bell)?, (true, true));

        let unknown = answered(
            b"POST /webhook/no-such-token HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}",
            &bell,
            home.path(),
        )?;
        assert!(unknown.starts_with("HTTP/1.1 404 "), "{unknown}");
        assert!(unknown.ends_with("no loop here takes requests with that token\n"));
        let big = answered(
            format!(
                "POST /webhook/t HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                super::BODY_MOST + 1
            )
            .as_bytes(),
            &bell,
            home.path(),
        )?;
        assert!(big.starts_with("HTTP/1.1 400 "), "{big}");
        let read = answered(b"GET /webhook/t HTTP/1.1\r\n\r\n", &bell, home.path())?;
        assert!(read.starts_with("HTTP/1.1 405 "), "{read}");
        Ok(())
    }
}
