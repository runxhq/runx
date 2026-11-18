//! The local model the always-on runx keeps up (the loop contract §15). The
//! configured model at its loopback endpoint, with `agent.serve`, the command
//! that serves it there, is asked for one token when runx comes on and every
//! few minutes, since a server that lists its models can still fail to
//! answer. Nothing listening, runx starts the command as its own child; its
//! child exits or fails, runx starts it again, waiting longer each time it
//! keeps failing. A model slow to answer is busy, never down, so runx never
//! restarts what may be serving someone else's request, and a model served by
//! a process runx didn't start is never runx's to restart. `runx off` stops
//! runx's child, and so does a new binary taking over, which starts it again.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::{TcpStream, ToSocketAddrs as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};

/// How often a model that answers is asked again.
const CHECK_EVERY: Duration = Duration::from_secs(300);
/// How often a model runx started is asked while it comes up.
const STARTING_CHECK: Duration = Duration::from_secs(5);
/// How long a model runx started has to answer before it's started again.
const STARTING_MOST: Duration = Duration::from_secs(900);
/// How long one ask waits for its token before the model is judged busy.
const ASK_WAIT: Duration = Duration::from_secs(120);
/// The wait before starting a failed model again, at first and at most,
/// doubling between.
const RESTART_WAIT: (Duration, Duration) = (Duration::from_secs(30), Duration::from_secs(600));
/// How long a stopped child has to end before it's killed.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// What one ask of the model found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    /// It gave its token.
    Answers,
    /// Nothing listens at its endpoint.
    Silent,
    /// It took longer than an ask waits: busy, never down.
    Busy,
    /// It answered with a failure, or with no answer a model gives.
    Fails(String),
}

/// What runx serves, as configured now: the model, its endpoint and the
/// command that serves it there.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Served {
    endpoint: String,
    model: String,
    command: String,
}

/// The model's state as `runx status` reads it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ModelState {
    /// answers, busy, starting, fails, waiting or none.
    pub state: String,
    pub endpoint: Option<String>,
    /// The process runx started, when it serves the model.
    pub pid: Option<u32>,
    /// Whether runx started what serves it.
    pub runx_serves: bool,
    pub why: Option<String>,
    pub at: String,
}

/// The keeper's thread, and the word that stops it.
pub(crate) struct Keeper {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Keeper {
    /// Keep the configured model up from now, reading the configuration at
    /// each check, so a change to it is taken without a restart.
    pub(crate) fn start(dir: &Path, env: BTreeMap<String, String>, cwd: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let dir = dir.to_path_buf();
        let thread = std::thread::spawn(move || keep(&dir, &env, &cwd, &stopped));
        Self {
            stop,
            thread: Some(thread),
        }
    }

    /// Stop keeping it: runx's child, when there is one, is stopped first.
    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The model runx serves, when one is configured to be: a chat completions
/// model at a loopback endpoint, with the command that serves it.
fn served(env: &BTreeMap<String, String>, cwd: &Path) -> Result<Option<Served>, String> {
    let paths = runx_runtime::RunxPaths::resolve(env, cwd);
    let config = runx_runtime::load_runx_config_file(&paths.config_file())
        .map_err(|error| error.to_string())?;
    let Some(command) = config.agent.as_ref().and_then(|agent| agent.serve.clone()) else {
        return Ok(None);
    };
    let Some(drive) =
        runx_runtime::load_drive_config(env, cwd).map_err(|error| error.to_string())?
    else {
        return Err("agent.serve is set, but no model is configured to serve".to_owned());
    };
    if !runx_runtime::is_zero_price(&drive) || !loopback(&drive.endpoint_url) {
        return Err(format!(
            "agent.serve serves only a model on this machine, and the model is at {}",
            drive.endpoint_url
        ));
    }
    if drive.dialect != runx_runtime::ModelDialect::ChatCompletions {
        return Err("agent.serve keeps up only a chat completions model".to_owned());
    }
    Ok(Some(Served {
        endpoint: drive.endpoint_url,
        model: drive.model,
        command,
    }))
}

fn loopback(endpoint: &str) -> bool {
    url::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1"))
}

/// Ask the model at `endpoint` for one token.
pub(crate) fn ask(endpoint: &str, model: &str, wait: Duration) -> Asked {
    let Ok(url) = url::Url::parse(endpoint) else {
        return Asked::Fails(format!("{endpoint} isn't an address"));
    };
    let host = url
        .host_str()
        .unwrap_or("127.0.0.1")
        .trim_matches(['[', ']'])
        .to_owned();
    let port = url.port_or_known_default().unwrap_or(80);
    let Some(address) = (host.as_str(), port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut found| found.next())
    else {
        return Asked::Fails(format!("{host} doesn't resolve"));
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_secs(5)) else {
        return Asked::Silent;
    };
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "Say ok."}],
        "max_tokens": 1,
        "stream": false,
    })
    .to_string();
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.set_read_timeout(Some(wait));
    if stream.write_all(request.as_bytes()).is_err() {
        return Asked::Silent;
    }
    let mut answer = Vec::new();
    match stream.read_to_end(&mut answer) {
        Ok(_) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            return Asked::Busy;
        }
        Err(error) => return Asked::Fails(format!("its answer broke off: {error}")),
    }
    let text = String::from_utf8_lossy(&answer);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    let excerpt = || body.trim().chars().take(300).collect::<String>();
    if !(200..300).contains(&status) {
        return Asked::Fails(format!("it answered {status}: {}", excerpt()));
    }
    // A chunked answer's body is its chunks; the choice is found in them.
    let gave = serde_json::from_str::<serde_json::Value>(body.trim())
        .ok()
        .and_then(|answer| answer.get("choices")?.get(0).cloned())
        .is_some()
        || body.contains("\"choices\"");
    if gave {
        Asked::Answers
    } else {
        Asked::Fails(format!("it gave no answer a model gives: {}", excerpt()))
    }
}

/// runx's own child serving the model.
struct Serving {
    child: Child,
    started: Instant,
}

/// Start `command` as runx's child, its output appended to the model's log.
fn start(command: &str, log: &Path) -> Result<Serving, String> {
    let output = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|error| format!("opening {}: {error}", log.display()))?;
    let mut shell = Command::new("/bin/sh");
    // The command becomes the child, so stopping the child stops the server.
    shell
        .arg("-c")
        .arg(format!("exec {command}"))
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(|error| error.to_string())?)
        .stderr(output);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Its own process group, so what it starts is stopped with it.
        shell.process_group(0);
    }
    let child = shell
        .spawn()
        .map_err(|error| format!("starting `{command}`: {error}"))?;
    Ok(Serving {
        child,
        started: Instant::now(),
    })
}

/// Stop runx's child: asked to end, then killed if it hasn't.
fn stop(mut serving: Serving) {
    let pid = serving.child.id();
    let _ = Command::new("kill")
        .args(["-TERM", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if matches!(serving.child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = serving.child.kill();
    let _ = serving.child.wait();
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn say(dir: &Path, state: &ModelState) {
    if let Ok(bytes) = serde_json::to_vec_pretty(state) {
        let path = dir.join("model.json");
        let staged = path.with_extension("tmp");
        if std::fs::write(&staged, bytes).is_ok() {
            let _ = std::fs::rename(staged, path);
        }
    }
}

/// The model's state as last kept, when runx keeps one.
pub(crate) fn read_state(dir: &Path) -> Option<ModelState> {
    serde_json::from_slice(&std::fs::read(dir.join("model.json")).ok()?).ok()
}

/// A line saying how the model runx keeps up stands.
pub(crate) fn status_line(state: &ModelState) -> Option<String> {
    let endpoint = state.endpoint.as_deref().unwrap_or("its endpoint");
    let who = match (state.runx_serves, state.pid) {
        (true, Some(pid)) => format!("runx serves it, process {pid}"),
        _ => "another process serves it".to_owned(),
    };
    let why = state
        .why
        .as_deref()
        .map(|why| format!(": {why}"))
        .unwrap_or_default();
    Some(match state.state.as_str() {
        "answers" => format!(
            "the model at {endpoint} answers ({who}), asked {}",
            state.at
        ),
        "busy" => {
            format!("the model at {endpoint} is busy: it took longer than a check waits ({who})")
        }
        "starting" => format!("runx is starting the model at {endpoint}{why}"),
        "fails" if state.runx_serves => {
            format!("the model at {endpoint} fails{why}; runx starts it again")
        }
        "fails" => format!(
            "the model at {endpoint} fails{why}, and another process serves it, which isn't runx's to restart: stop it, and runx starts the model"
        ),
        "waiting" => {
            format!("the model at {endpoint} failed{why}; runx starts it again after a wait")
        }
        "none" => format!("runx keeps no model up{why}"),
        _ => return None,
    })
}

/// Wait `wait`, or less when stopped or when runx's own child ends, so a
/// model that falls over is started again after its wait, not the check's.
fn pause(wait: Duration, stopped: &AtomicBool, keeping: &mut Keeping) {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline && !stopped.load(Ordering::Relaxed) && !keeping.child_ended() {
        std::thread::sleep(Duration::from_millis(200).min(deadline - Instant::now()));
    }
}

/// What the keeper holds between checks: runx's child, how long the next
/// restart waits, and what it last served.
#[derive(Default)]
struct Keeping {
    serving: Option<Serving>,
    restart_wait: Option<Duration>,
    last: Option<Served>,
}

impl Keeping {
    fn state(&self, name: &str, served: Option<&Served>, why: Option<String>) -> ModelState {
        ModelState {
            state: name.to_owned(),
            endpoint: served.map(|served| served.endpoint.clone()),
            pid: self.serving.as_ref().map(|serving| serving.child.id()),
            runx_serves: self.serving.is_some(),
            why,
            at: now(),
        }
    }

    /// The wait before the next restart, longer each time one follows.
    fn backoff(&mut self) -> Duration {
        let wait = self.restart_wait.unwrap_or(RESTART_WAIT.0);
        self.restart_wait = Some((wait * 2).min(RESTART_WAIT.1));
        wait
    }

    /// Whether runx's own child has ended since it was last checked.
    fn child_ended(&mut self) -> bool {
        self.serving
            .as_mut()
            .is_some_and(|serving| matches!(serving.child.try_wait(), Ok(Some(_))))
    }

    fn stop_child(&mut self) {
        if let Some(child) = self.serving.take() {
            stop(child);
        }
    }

    /// One check of the model `current` names, or of none: what it did is
    /// kept for `runx status`, and the wait before the next check returned.
    fn check(
        &mut self,
        dir: &Path,
        current: Result<Option<Served>, String>,
        ask_wait: Duration,
    ) -> Duration {
        let current = match current {
            Ok(current) => current,
            Err(why) => {
                self.stop_child();
                say(dir, &self.state("none", None, Some(why)));
                return CHECK_EVERY;
            }
        };
        // What runx serves changed: its child for the old one stops.
        if current != self.last {
            self.stop_child();
            self.restart_wait = None;
            self.last.clone_from(&current);
        }
        let Some(served) = current else {
            let _ = std::fs::remove_file(dir.join("model.json"));
            return CHECK_EVERY;
        };
        // Its child ended: it's started again after a wait.
        if let Some(child) = self.serving.as_mut()
            && let Ok(Some(status)) = child.child.try_wait()
        {
            self.serving = None;
            let why = format!("the process runx started ended ({status})");
            say(dir, &self.state("waiting", Some(&served), Some(why)));
            return self.backoff();
        }
        let starting = self
            .serving
            .as_ref()
            .is_some_and(|child| child.started.elapsed() < STARTING_MOST);
        match ask(&served.endpoint, &served.model, ask_wait) {
            Asked::Answers => {
                self.restart_wait = None;
                say(dir, &self.state("answers", Some(&served), None));
                CHECK_EVERY
            }
            Asked::Busy => {
                say(dir, &self.state("busy", Some(&served), None));
                CHECK_EVERY
            }
            // Coming up: asked again soon, until it's had its time.
            Asked::Silent | Asked::Fails(_) if starting => {
                say(dir, &self.state("starting", Some(&served), None));
                STARTING_CHECK
            }
            Asked::Silent if self.serving.is_none() => {
                match start(&served.command, &dir.join("model.log")) {
                    Ok(started) => {
                        self.serving = Some(started);
                        say(dir, &self.state("starting", Some(&served), None));
                        STARTING_CHECK
                    }
                    Err(why) => {
                        say(dir, &self.state("waiting", Some(&served), Some(why)));
                        self.backoff()
                    }
                }
            }
            // runx's own child fails, or never came up in its time: stopped,
            // and started again after a wait.
            Asked::Silent | Asked::Fails(_) if self.serving.is_some() => {
                self.stop_child();
                let why =
                    "the process runx started didn't answer in its time, or failed".to_owned();
                say(dir, &self.state("waiting", Some(&served), Some(why)));
                self.backoff()
            }
            Asked::Silent => {
                say(
                    dir,
                    &self.state("waiting", Some(&served), Some("nothing answers".to_owned())),
                );
                self.backoff()
            }
            // Served by another process: never runx's to restart.
            Asked::Fails(why) => {
                say(dir, &self.state("fails", Some(&served), Some(why)));
                CHECK_EVERY
            }
        }
    }
}

/// Keep the configured model up until stopped.
fn keep(dir: &Path, env: &BTreeMap<String, String>, cwd: &Path, stopped: &AtomicBool) {
    let mut keeping = Keeping::default();
    while !stopped.load(Ordering::Relaxed) {
        let wait = keeping.check(dir, served(env, cwd), ASK_WAIT);
        pause(wait, stopped, &mut keeping);
    }
    keeping.stop_child();
    let _ = std::fs::remove_file(dir.join("model.json"));
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader};
    use std::net::TcpListener;
    use std::sync::mpsc;

    use super::*;

    /// A model at a loopback address that answers one request with `answer`.
    fn answering(
        answer: &'static str,
    ) -> Result<(String, std::thread::JoinHandle<()>), std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
        let thread = std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let _ = reader.get_mut().write_all(answer.as_bytes());
        });
        Ok((endpoint, thread))
    }

    /// An address nothing listens at.
    fn silent() -> Result<String, std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        Ok(format!(
            "http://{}/v1/chat/completions",
            listener.local_addr()?
        ))
    }

    const ANSWERS: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 41\r\nConnection: close\r\n\r\n{\"choices\":[{\"message\":{\"content\":\"o\"}}]}";
    const DIED: &str = "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 35\r\nConnection: close\r\n\r\n{\"error\": \"generation thread died\"}";

    #[test]
    fn a_model_answers_its_token_or_says_how_it_failed() -> Result<(), Box<dyn std::error::Error>> {
        let (endpoint, thread) = answering(ANSWERS)?;
        assert_eq!(ask(&endpoint, "m", ASK_WAIT), Asked::Answers);
        thread.join().map_err(|_| "the model's thread")?;
        // One that lists its models but can't generate, as on 18 November.
        let (endpoint, thread) = answering(DIED)?;
        let Asked::Fails(why) = ask(&endpoint, "m", ASK_WAIT) else {
            return Err("a failure is a failure".into());
        };
        assert!(
            why.contains("404") && why.contains("generation thread died"),
            "{why}"
        );
        thread.join().map_err(|_| "the model's thread")?;
        assert_eq!(ask(&silent()?, "m", ASK_WAIT), Asked::Silent);
        Ok(())
    }

    #[test]
    fn a_model_slower_than_a_check_waits_is_busy_never_down()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}/v1/chat/completions", listener.local_addr()?);
        let (done, finished) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            // Holds the request, answering nothing, until the check is done.
            let held = listener.accept();
            let _ = finished.recv();
            drop(held);
        });
        assert_eq!(ask(&endpoint, "m", Duration::from_millis(200)), Asked::Busy);
        done.send(())?;
        thread.join().map_err(|_| "the model's thread")?;
        Ok(())
    }

    fn served(endpoint: String, command: &str) -> Result<Option<Served>, String> {
        Ok(Some(Served {
            endpoint,
            model: "m".to_owned(),
            command: command.to_owned(),
        }))
    }

    #[test]
    fn nothing_answering_runx_starts_the_command_and_stops_what_it_started()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut keeping = Keeping::default();
        let wait = keeping.check(dir.path(), served(silent()?, "sleep 600"), ASK_WAIT);
        assert_eq!(wait, STARTING_CHECK);
        let state = read_state(dir.path()).ok_or("its state is kept")?;
        assert_eq!(
            (state.state.as_str(), state.runx_serves),
            ("starting", true)
        );
        let pid = state.pid.ok_or("runx's process")?;
        assert!(
            keeping
                .serving
                .as_mut()
                .ok_or("serving")?
                .child
                .try_wait()?
                .is_none(),
            "it runs"
        );
        keeping.stop_child();
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()?;
        assert!(!alive.success(), "what runx started is stopped");
        Ok(())
    }

    #[test]
    fn a_child_that_ends_is_started_again_after_a_wait_that_grows()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let endpoint = silent()?;
        let mut keeping = Keeping::default();
        keeping.check(dir.path(), served(endpoint.clone(), "false"), ASK_WAIT);
        // Wait on its end, then check again.
        keeping.serving.as_mut().ok_or("serving")?.child.wait()?;
        assert_eq!(
            keeping.check(dir.path(), served(endpoint.clone(), "false"), ASK_WAIT),
            RESTART_WAIT.0
        );
        let state = read_state(dir.path()).ok_or("its state is kept")?;
        assert_eq!(state.state, "waiting");
        assert!(
            state
                .why
                .as_deref()
                .is_some_and(|why| why.contains("ended")),
            "{state:?}"
        );
        keeping.check(dir.path(), served(endpoint.clone(), "false"), ASK_WAIT);
        keeping
            .serving
            .as_mut()
            .ok_or("serving again")?
            .child
            .wait()?;
        assert_eq!(
            keeping.check(dir.path(), served(endpoint, "false"), ASK_WAIT),
            RESTART_WAIT.0 * 2
        );
        Ok(())
    }

    #[test]
    fn a_failing_model_another_process_serves_is_never_restarted()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let (endpoint, thread) = answering(DIED)?;
        let mut keeping = Keeping::default();
        assert_eq!(
            keeping.check(dir.path(), served(endpoint, "sleep 600"), ASK_WAIT),
            CHECK_EVERY
        );
        thread.join().map_err(|_| "the model's thread")?;
        assert!(keeping.serving.is_none(), "runx started nothing");
        let state = read_state(dir.path()).ok_or("its state is kept")?;
        assert_eq!((state.state.as_str(), state.runx_serves), ("fails", false));
        let line = status_line(&state).ok_or("a line")?;
        assert!(line.contains("isn't runx's to restart"), "{line}");
        // Nothing configured to serve: nothing kept, nothing said.
        keeping.check(dir.path(), Ok(None), ASK_WAIT);
        assert!(read_state(dir.path()).is_none());
        Ok(())
    }
}
