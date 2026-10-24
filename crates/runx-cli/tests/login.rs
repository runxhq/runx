//! Browser login owns a challenge on stderr and one final result on stdout.
//! All HTTP responses and credentials in this journey are local fixtures.
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::support::{temp_root, unsigned_runx_command_at};

const FIXTURE_TIMEOUT: Duration = Duration::from_secs(10);
const CHALLENGE: &str = "https://runx.test/connect/login_fixture";
const LOGIN_TOKEN: &str = "fixture_poll_ticket";
const API_TOKEN: &str = "rxk_fixture_login";

#[test]
fn json_login_exposes_challenge_before_completion() -> Result<(), Box<dyn std::error::Error>> {
    browser_login_journey(true)
}

#[test]
fn human_login_exposes_challenge_before_completion() -> Result<(), Box<dyn std::error::Error>> {
    browser_login_journey(false)
}

fn browser_login_journey(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let root = ScratchRoot::new()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base_url = format!("http://{}", listener.local_addr()?);
    let (challenge_seen, challenge_ready) = mpsc::channel();
    let server = thread::spawn(move || -> Result<(), String> {
        let started = serde_json::json!({
            "status": "pending",
            "session_id": "login_fixture",
            "login_token": LOGIN_TOKEN,
            "authorization_url": CHALLENGE,
            "poll_after_ms": 0
        });
        serve_json(&listener, "/v1/login/sessions", &started)?;
        // Success is impossible until the operator has received the challenge.
        challenge_ready
            .recv_timeout(FIXTURE_TIMEOUT)
            .map_err(|_| "browser challenge was not observed before completion".to_owned())?;
        serve_json(
            &listener,
            "/v1/login/sessions/login_fixture/complete",
            &serde_json::json!({
                "status": "success",
                "session_id": "login_fixture",
                "principal_id": "user_fixture",
                "credential_id": "cred_fixture",
                "token": API_TOKEN
            }),
        )
    });
    let mut command = unsigned_runx_command_at(&root.0);
    command.args(["login", "--api-base-url", &base_url, "--allow-local-api"]);
    if json {
        command.arg("--json");
    }
    let mut child = ChildGuard(command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?);
    let stdout = child.0.stdout.take().ok_or("missing child stdout")?;
    let stderr = child.0.stderr.take().ok_or("missing child stderr")?;
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || -> Result<Vec<u8>, String> {
        let mut reader = BufReader::new(stderr);
        let mut output = Vec::new();
        loop {
            let mut line = Vec::new();
            if reader.read_until(b'\n', &mut line).map_err(|e| e.to_string())? == 0 {
                return Ok(output);
            }
            if line.strip_suffix(b"\n") == Some(CHALLENGE.as_bytes()) {
                let _ignored = challenge_seen.send(());
            }
            output.extend_from_slice(&line);
        }
    });
    // Bound the child independently of login's production timeout.
    let status = wait_for_child(&mut child.0)?;
    let stdout = stdout_reader.join().map_err(|_| "stdout reader panicked")??;
    let stderr = stderr_reader.join().map_err(|_| "stderr reader panicked")??;
    server.join().map_err(|_| "login fixture server panicked")??;
    assert!(status.success(), "login fixture did not complete successfully");
    let prompt = String::from_utf8(stderr)?;
    assert_eq!(
        prompt,
        format!("Open this URL to sign in to runx:\n{CHALLENGE}\n\nWaiting for public API login...\n")
    );
    let stdout_text = std::str::from_utf8(&stdout)?;
    if json {
        // from_slice rejects additional JSON documents and trailing prose.
        let result: serde_json::Value = serde_json::from_slice(&stdout)?;
        assert_eq!(result, serde_json::json!({
            "status": "success",
            "principal_id": "user_fixture",
            "credential_id": "cred_fixture"
        }));
    } else {
        assert!(stdout_text.contains("login  success"));
        assert!(stdout_text.contains("user_fixture"));
        assert!(stdout_text.contains("cred_fixture"));
    }
    assert!(!stdout_text.contains(CHALLENGE));
    for secret in [LOGIN_TOKEN, API_TOKEN] {
        assert!(!prompt.contains(secret));
        assert!(!stdout_text.contains(secret));
    }
    let config = fs::read_to_string(root.0.join("home/config.json"))?;
    assert!(config.contains("api_token_ref"));
    assert!(!config.contains(API_TOKEN));
    assert!(!config.contains(LOGIN_TOKEN));
    Ok(())
}

#[test]
fn json_login_rejects_missing_or_blank_challenge() -> Result<(), Box<dyn std::error::Error>> {
    for authorization_url in [serde_json::Value::Null, serde_json::json!("   ")] {
        let root = ScratchRoot::new()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = thread::spawn(move || {
            serve_json(&listener, "/v1/login/sessions", &serde_json::json!({
                "status": "pending",
                "session_id": "login_fixture",
                "login_token": LOGIN_TOKEN,
                "authorization_url": authorization_url
            }))
        });
        // No completion request is served: invalid challenges must fail before polling.
        let mut child = ChildGuard(unsigned_runx_command_at(&root.0)
            .args(["login", "--api-base-url", &base_url, "--allow-local-api", "--json"])
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?);
        let stdout = child.0.stdout.take().ok_or("missing child stdout")?;
        let stderr = child.0.stderr.take().ok_or("missing child stderr")?;
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr_reader = thread::spawn(move || read_all(stderr));
        let status = wait_for_child(&mut child.0)?;
        let stdout = stdout_reader.join().map_err(|_| "stdout reader panicked")??;
        let stderr = stderr_reader.join().map_err(|_| "stderr reader panicked")??;
        server.join().map_err(|_| "login fixture server panicked")??;
        assert!(!status.success());
        assert!(stderr.is_empty());
        let result: serde_json::Value = serde_json::from_slice(&stdout)?;
        assert_eq!(result["status"], "failure");
        assert_eq!(result["error"]["code"], "login_failed");
        assert!(result["error"]["message"].as_str().ok_or("missing error message")?
            .contains("browser sign-in URL"));
        assert!(!root.0.join("home/config.json").exists());
    }
    Ok(())
}

fn read_all(mut input: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn wait_for_child(child: &mut Child) -> Result<std::process::ExitStatus, std::io::Error> {
    let deadline = Instant::now() + FIXTURE_TIMEOUT * 2;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "login fixture child timed out"));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn serve_json(listener: &TcpListener, path: &str, body: &serde_json::Value) -> Result<(), String> {
    let mut stream = accept_bounded(listener)?;
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(FIXTURE_TIMEOUT)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(FIXTURE_TIMEOUT)).map_err(|e| e.to_string())?;
    // Consume the entire request so closing the socket does not reset unread data.
    {
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if line != format!("POST {path} HTTP/1.1\r\n") {
            return Err("unexpected login fixture request path or method".to_owned());
        }
        let mut length = 0usize;
        let mut header_bytes = line.len();
        loop {
            line.clear();
            if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("incomplete login fixture request headers".to_owned());
            }
            header_bytes += line.len();
            if header_bytes > 16_384 {
                return Err("oversized login fixture request headers".to_owned());
            }
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse::<usize>().map_err(|e| e.to_string())?;
                }
            }
        }
        if length > 16_384 {
            return Err("oversized login fixture request body".to_owned());
        }
        let mut request_body = vec![0; length];
        reader.read_exact(&mut request_body).map_err(|e| e.to_string())?;
    }
    let body = body.to_string();
    write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body)
        .map_err(|e| e.to_string())
}

fn accept_bounded(listener: &TcpListener) -> Result<TcpStream, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + FIXTURE_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("login fixture request timed out".to_owned());
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ignored = self.0.kill();
        let _ignored = self.0.wait();
    }
}

struct ScratchRoot(PathBuf);

impl ScratchRoot {
    fn new() -> Result<Self, std::io::Error> {
        let root = temp_root("runx-login-journey");
        fs::create_dir_all(&root)?;
        Ok(Self(root))
    }
}

impl Drop for ScratchRoot {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.0);
    }
}
