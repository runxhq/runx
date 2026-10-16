use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::support::isolated_runx_command_with_inherited_cwd;

use runx_runtime::LocalReceiptStore;
use serde_json::{Value, json};

const MCP_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);
const MCP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn mcp_native_binary_dogfoods_streaming_skill_calls_and_receipts()
-> Result<(), Box<dyn std::error::Error>> {
    let receipt_dir = TestTempDir::new("runx-mcp-dogfood-receipts")?;
    let skill_dir = copy_mcp_echo_skill()?;
    let skill_path = skill_dir.path().to_path_buf();
    let mut server = spawn_mcp_server(&[
        skill_path.display().to_string(),
        "--receipt-dir".to_owned(),
        receipt_dir.path().display().to_string(),
    ])?;

    write_frame(server.stdin_mut()?, &initialize_request(1))?;
    let initialize = server.read_response("initialize", MCP_INITIALIZE_TIMEOUT)?;
    assert_eq!(
        path_text(&initialize, &["result", "protocolVersion"])?,
        "2025-06-18"
    );

    write_frame(server.stdin_mut()?, &initialized_notification())?;
    write_frame(server.stdin_mut()?, &request(2, "tools/list", json!({})))?;
    let tools = server.read_response("tools/list", MCP_REQUEST_TIMEOUT)?;
    assert_eq!(
        path_text(&tools, &["result", "tools", "0", "name"])?,
        "mcp-echo"
    );

    let mut receipt_ids = Vec::new();
    for index in 0..6 {
        let message = format!("dogfood message {index}");
        write_frame(
            server.stdin_mut()?,
            &request(
                10 + index,
                "tools/call",
                json!({
                    "name": "mcp-echo",
                    "arguments": {
                        "message": message,
                    },
                }),
            ),
        )?;
        let response = server.read_response(
            &format!("tools/call dogfood message {index}"),
            MCP_REQUEST_TIMEOUT,
        )?;
        assert_eq!(
            path_text(
                &response,
                &["result", "structuredContent", "runx", "status"]
            )?,
            "sealed",
            "unexpected MCP dogfood response: {response}"
        );
        assert_eq!(
            path_text(&response, &["result", "content", "0", "text"])?,
            serde_json::json!({ "message": format!("dogfood message {index}") }).to_string()
        );
        receipt_ids.push(
            path_text(
                &response,
                &["result", "structuredContent", "runx", "receipt_id"],
            )?
            .to_owned(),
        );
    }

    let mut cli = isolated_runx_command_with_inherited_cwd("mcp-dogfood-test-key");
    let cli = cli
        .current_dir(repo_root()?)
        .arg("skill")
        .arg(&skill_path)
        .args(["--input", "message=dogfood message 0", "--receipt-dir"])
        .arg(receipt_dir.path())
        .arg("--json")
        .output()?;
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stdout)
    );
    let cli: Value = serde_json::from_slice(&cli.stdout)?;
    assert_eq!(cli["result"]["message"], "dogfood message 0");
    assert_eq!(cli["closure"]["disposition"], "closed");

    // Admission is a package binding, not permission to execute whatever bytes
    // happen to replace the served package later in this session.
    fs::write(
        skill_path.join("SKILL.md"),
        "---\nname: mcp-echo\ndescription: mcp-echo fixture.\n---\nChanged after admission.\n",
    )?;
    write_frame(
        server.stdin_mut()?,
        &request(
            30,
            "tools/call",
            json!({
                "name": "mcp-echo", "arguments": {"message": "must not execute"},
            }),
        ),
    )?;
    let drift = server.read_response("package drift", MCP_REQUEST_TIMEOUT)?;
    assert_eq!(drift["result"]["isError"], true, "{drift}");
    assert!(
        drift["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("digest"))
    );

    server.close_stdin();
    let status = server.wait_timeout(Duration::from_secs(10))?;
    assert!(
        status.success(),
        "runx mcp exited with {status}; stderr: {}",
        server.stderr_string()?
    );

    assert_eq!(receipt_ids.len(), 6);
    let receipt_store = LocalReceiptStore::new(receipt_dir.path());
    for receipt_id in receipt_ids {
        let receipt_path = receipt_store.receipt_path(&receipt_id)?;
        let receipt = read_json_file(&receipt_path)?;
        assert_eq!(
            path_text(&receipt, &["schema"])?,
            runx_contracts::RECEIPT_SCHEMA
        );
        assert_eq!(path_text(&receipt, &["id"])?, receipt_id);
        assert_eq!(path_text(&receipt, &["seal", "disposition"])?, "closed");
        assert!(
            receipt.get("seal").is_some(),
            "missing receipt seal in {}",
            receipt_path.display()
        );
    }
    Ok(())
}

/// A served ref resolves as `runx skill` resolves it: a bundled skill by name.
#[test]
fn mcp_serves_a_skill_ref_not_only_a_path() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = spawn_mcp_server(&["skill-build".to_owned()])?;
    write_frame(server.stdin_mut()?, &initialize_request(1))?;
    server.read_response("initialize", MCP_INITIALIZE_TIMEOUT)?;
    write_frame(server.stdin_mut()?, &initialized_notification())?;
    write_frame(server.stdin_mut()?, &request(2, "tools/list", json!({})))?;
    let tools = server.read_response("tools/list", MCP_REQUEST_TIMEOUT)?;
    assert_eq!(
        path_text(&tools, &["result", "tools", "0", "name"])?,
        "skill-build"
    );
    Ok(())
}

#[test]
fn mcp_native_binary_reports_mid_session_framing_fault() -> Result<(), Box<dyn std::error::Error>> {
    let skill_path = repo_root()?.join("fixtures/skills/mcp-echo");
    let mut server = spawn_mcp_server(&[skill_path.display().to_string()])?;

    write_frame(server.stdin_mut()?, &initialize_request(1))?;
    let initialize = server.read_response("initialize", MCP_INITIALIZE_TIMEOUT)?;
    assert_eq!(
        path_text(&initialize, &["result", "protocolVersion"])?,
        "2025-06-18"
    );

    write_frame(server.stdin_mut()?, &initialized_notification())?;
    server.stdin_mut()?.write_all(b"{\n")?;
    server.close_stdin();

    let status = server.wait_timeout(Duration::from_secs(10))?;
    assert!(
        !status.success(),
        "malformed mid-session MCP frame must fail closed"
    );
    let stderr = server.stderr_string()?;
    assert!(
        stderr.contains("MCP rmcp server task failed: EOF while parsing an object"),
        "unexpected stderr: {stderr}"
    );
    Ok(())
}

#[test]
fn mcp_native_binary_uses_one_workspace_env_snapshot_per_session()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = TestTempDir::new("runx-mcp-workspace-env")?;
    // The repository marks the project root.
    fs::create_dir_all(workspace.path().join(".git"))?;
    fs::write(
        workspace.path().join(".env"),
        "MCP_DOGFOOD_MARKER=initial-snapshot\n",
    )?;
    let skill_dir = write_workspace_env_mcp_skill(workspace.path())?;
    let mut server =
        spawn_mcp_server_at(&[skill_dir.display().to_string()], workspace.path(), None)?;

    write_frame(server.stdin_mut()?, &initialize_request(1))?;
    let initialize = server.read_response("initialize", MCP_INITIALIZE_TIMEOUT)?;
    assert_eq!(
        path_text(&initialize, &["result", "protocolVersion"])?,
        "2025-06-18"
    );
    write_frame(server.stdin_mut()?, &initialized_notification())?;

    // A long-lived MCP server must keep the command-entry snapshot. Reloading
    // the file between calls would make one operator session non-deterministic.
    fs::write(
        workspace.path().join(".env"),
        "MCP_DOGFOOD_MARKER=changed-after-start\n",
    )?;
    write_frame(
        server.stdin_mut()?,
        &request(
            2,
            "tools/call",
            json!({
                "name": "mcp-workspace-env",
                "arguments": {},
            }),
        ),
    )?;
    let response = server.read_response("tools/call workspace env", MCP_REQUEST_TIMEOUT)?;
    assert_eq!(
        path_text(&response, &["result", "content", "0", "text"])?,
        serde_json::json!({ "marker": "initial-snapshot" }).to_string()
    );

    server.close_stdin();
    let status = server.wait_timeout(Duration::from_secs(10))?;
    assert!(
        status.success(),
        "runx mcp exited with {status}; stderr: {}",
        server.stderr_string()?
    );
    Ok(())
}

#[test]
fn mcp_continuations_survive_server_exit_and_resume_through_the_cli()
-> Result<(), Box<dyn std::error::Error>> {
    for agent in [false, true] {
        let workspace = TestTempDir::new("runx-mcp-continuation")?;
        let root = workspace.path();
        let receipts = root.join("receipts with spaces");
        let skill = if agent {
            let path = crate::support::write_agent_task_skill(root)?;
            let manifest = fs::read_to_string(path.join("run.yaml"))?
                .replace("required: false", "required: true");
            fs::write(path.join("run.yaml"), manifest)?;
            path
        } else {
            let path = root.join("approval");
            fs::create_dir_all(&path)?;
            fs::write(
                path.join("SKILL.md"),
                "---\nname: approval\ndescription: approval fixture.\n---\n# Approval\n",
            )?;
            fs::write(
                path.join("run.yaml"),
                r#"skill: approval
runners:
  main:
    default: true
    inputs:
      thread_title: { type: string, required: true }
    result_from: [approve]
    steps:
      - id: approve
        ask: { gate: approval, reason: Approve this thread. }
        inputs:
          thread_title: $input.thread_title
"#,
            )?;
            path
        };
        let mut run_ids = std::collections::BTreeSet::new();
        for _ in 0..2 {
            let mut server = spawn_mcp_server_at(
                &[
                    skill.display().to_string(),
                    "--receipt-dir".to_owned(),
                    receipts.display().to_string(),
                ],
                root,
                None,
            )?;
            write_frame(server.stdin_mut()?, &initialize_request(1))?;
            server.read_response("initialize", MCP_INITIALIZE_TIMEOUT)?;
            write_frame(server.stdin_mut()?, &initialized_notification())?;
            write_frame(
                server.stdin_mut()?,
                &request(
                    2,
                    "tools/call",
                    json!({
                        "name": if agent { "code-intake" } else { "approval" },
                        "arguments": { "thread_title": "Preserve this original input" },
                    }),
                ),
            )?;
            let response = server.read_response("continuation", MCP_REQUEST_TIMEOUT)?;
            let pending = &response["result"]["structuredContent"]["runx"];
            let waits_on = if agent {
                "needs_agent"
            } else {
                "needs_approval"
            };
            assert_eq!(pending["status"], waits_on, "{response}");
            assert!(
                pending["requests"]
                    .to_string()
                    .contains("Preserve this original input"),
                "{response}"
            );
            let run_id = pending["run_id"].as_str().ok_or("missing durable run id")?;
            assert!(
                run_ids.insert(run_id.to_owned()),
                "server restart reused a run identity"
            );
            // Agent work is done through the turn tools any skill server
            // serves for the run; a person approves through `runx resume`,
            // pointed at the receipts.
            let text = response["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default();
            if agent {
                assert!(
                    text.contains("turn_complete") && text.contains(run_id),
                    "{text}"
                );
            } else {
                assert!(text.contains("--receipt-dir '"), "{text}");
            }
            let request_id = pending["requests"][0]["id"]
                .as_str()
                .ok_or("missing request id")?;
            let answers = if agent {
                json!({"answers": {request_id: {"intake_report": {"summary": "Original input reviewed"}}}})
            } else {
                json!({"approvals": {request_id: true}})
            };
            let answers_path = root.join("answers.json");
            server.close_stdin();
            assert!(server.wait_timeout(Duration::from_secs(10))?.success());

            let command = || {
                let mut command = isolated_runx_command_with_inherited_cwd("mcp-dogfood-test-key");
                command.current_dir(root).env("RUNX_CWD", root);
                command
            };
            let read = command().args(["history", run_id, "--json"]).output()?;
            assert!(read.status.success());
            let read: Value = serde_json::from_slice(&read.stdout)?;
            assert_eq!(read["run"]["state"], "paused");
            let answers = crate::support::bind_resume(&read, answers)?;

            // An approval of a request the run was not paused on is refused.
            let mut wrong = answers.clone();
            wrong["approvals"]["not-asked"] =
                json!({"request_digest": "sha256:not-asked", "approved": true});
            fs::write(&answers_path, serde_json::to_vec(&wrong)?)?;
            let refused = command()
                .args(["resume", run_id])
                .arg(&answers_path)
                .arg("--receipt-dir")
                .arg(&receipts)
                .arg("--json")
                .output()?;
            assert_eq!(refused.status.code(), Some(1));
            fs::write(&answers_path, serde_json::to_vec(&answers)?)?;
            let resumed = command()
                .args(["resume", run_id])
                .arg(&answers_path)
                .arg("--receipt-dir")
                .arg(&receipts)
                .arg("--json")
                .output()?;
            assert!(
                resumed.status.success(),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&resumed.stdout),
                String::from_utf8_lossy(&resumed.stderr)
            );
            let resumed: Value = serde_json::from_slice(&resumed.stdout)?;
            assert_eq!(resumed["closure"]["disposition"], "closed");
            assert_eq!(resumed["run_id"], run_id);
            let receipt_id = resumed["receipt_id"].as_str().ok_or("missing receipt id")?;
            let verified = command()
                .args(["verify", receipt_id, "--receipt-dir"])
                .arg(&receipts)
                .arg("--json")
                .output()?;
            assert!(
                verified.status.success(),
                "{}",
                String::from_utf8_lossy(&verified.stdout)
            );
            let verified: Value = serde_json::from_slice(&verified.stdout)?;
            assert_eq!(verified["valid"], true);
            let read = command().args(["history", run_id, "--json"]).output()?;
            assert!(read.status.success());
            let read: Value = serde_json::from_slice(&read.stdout)?;
            assert_eq!(read["run"]["state"], "sealed");
        }
    }
    Ok(())
}

#[test]
fn runx_turn_serves_a_runs_open_turns_and_completes_one() -> Result<(), Box<dyn std::error::Error>>
{
    let workspace = TestTempDir::new("runx-turn-workspace")?;
    let skill_dir = workspace.path().join("turn-demo");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: turn-demo\ndescription: Answers with what it digested.\n---\n# Turn demo\n\nDigest what you need, then answer.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "skill: turn-demo\ntools: [data.digest]\nrunners:\n  answer:\n    default: true\n    inputs:\n      question: { type: string, required: true }\n    outputs:\n      answer: string\n",
    )?;
    let receipts = workspace.path().join("receipts");
    let mut start = isolated_runx_command_with_inherited_cwd("mcp-dogfood-test-key");
    let started = start
        .current_dir(workspace.path())
        .env("RUNX_CWD", workspace.path())
        .arg("skill")
        .arg(&skill_dir)
        .args(["--input", "question=what digests?", "--receipt-dir"])
        .arg(&receipts)
        .arg("--json")
        .output()?;
    let started: Value = serde_json::from_slice(&started.stdout)?;
    assert_eq!(started["status"], "needs_agent", "{started}");
    let run_id = started["run_id"]
        .as_str()
        .ok_or("missing run_id")?
        .to_owned();

    let mut server = spawn_runx_stdio(
        &[
            "mcp".to_owned(),
            "--run".to_owned(),
            run_id,
            "--receipt-dir".to_owned(),
            receipts.display().to_string(),
        ],
        workspace.path(),
        Some(workspace.path()),
    )?;
    write_frame(server.stdin_mut()?, &initialize_request(1))?;
    let initialize = server.read_response("initialize", MCP_INITIALIZE_TIMEOUT)?;
    assert!(
        path_text(&initialize, &["result", "instructions"])?.contains("open now: answer"),
        "{initialize}"
    );
    write_frame(server.stdin_mut()?, &initialized_notification())?;
    write_frame(server.stdin_mut()?, &request(2, "tools/list", json!({})))?;
    let tools = server.read_response("tools/list", MCP_REQUEST_TIMEOUT)?;
    let names = tools["result"]["tools"]
        .as_array()
        .ok_or("missing tools")?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["turn_list", "turn_status", "turn_call", "turn_complete"],
        "the tool set is fixed; a turn's own tools come with its status"
    );
    write_frame(
        server.stdin_mut()?,
        &request(
            6,
            "tools/call",
            json!({"name": "turn_status", "arguments": {"turn": "answer"}}),
        ),
    )?;
    let opened = server.read_response("turn_status", MCP_REQUEST_TIMEOUT)?;
    assert!(
        path_text(
            &opened,
            &[
                "result",
                "structuredContent",
                "invocation",
                "envelope",
                "instructions"
            ]
        )?
        .contains("Digest what you need"),
        "{opened}"
    );
    assert_eq!(
        opened["result"]["structuredContent"]["tools"][0]["name"], "data_digest",
        "{opened}"
    );

    write_frame(
        server.stdin_mut()?,
        &request(
            3,
            "tools/call",
            json!({"name": "turn_call", "arguments": {
                "turn": "answer",
                "tool": "data_digest",
                "input": {"value": "hello", "encoding": "utf8_text"},
            }}),
        ),
    )?;
    let digest = server.read_response("turn_call", MCP_REQUEST_TIMEOUT)?;
    assert_eq!(
        path_text(
            &digest,
            &["result", "structuredContent", "outcome", "status"]
        )?,
        "completed",
        "{digest}"
    );
    write_frame(
        server.stdin_mut()?,
        &request(
            5,
            "tools/call",
            json!({"name": "turn_status", "arguments": {"turn": "answer"}}),
        ),
    )?;
    let status = server.read_response("turn_status", MCP_REQUEST_TIMEOUT)?;
    assert_eq!(
        status["result"]["structuredContent"]["calls"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "the turn reads back its recorded call: {status}"
    );

    write_frame(
        server.stdin_mut()?,
        &request(
            4,
            "tools/call",
            json!({"name": "turn_complete", "arguments": {
                "turn": "answer",
                "output": {"answer": "digested"},
            }}),
        ),
    )?;
    let completed = server.read_response("turn_complete", MCP_REQUEST_TIMEOUT)?;
    assert_eq!(
        path_text(
            &completed,
            &["result", "structuredContent", "runx", "status"]
        )?,
        "sealed",
        "{completed}"
    );

    server.close_stdin();
    let status = server.wait_timeout(Duration::from_secs(10))?;
    assert!(
        status.success(),
        "runx mcp --run exited with {status}; stderr: {}",
        server.stderr_string()?
    );
    Ok(())
}

/// A client on the current protocol opens without `initialize`, as Claude
/// Code does, stating its version on each request; a list result then states
/// how long it may be cached, and by whom.
#[test]
fn a_client_on_the_current_protocol_lists_tools_with_their_cache_terms()
-> Result<(), Box<dyn std::error::Error>> {
    let receipt_dir = TestTempDir::new("runx-mcp-current-protocol-receipts")?;
    let skill_dir = copy_mcp_echo_skill()?;
    let mut server = spawn_mcp_server(&[
        skill_dir.path().display().to_string(),
        "--receipt-dir".to_owned(),
        receipt_dir.path().display().to_string(),
    ])?;
    write_frame(
        server.stdin_mut()?,
        &request(
            1,
            "tools/list",
            json!({"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "current", "version": "1"},
            }}),
        ),
    )?;
    let tools = server.read_response("tools/list", MCP_INITIALIZE_TIMEOUT)?;
    assert_eq!(
        path_text(&tools, &["result", "tools", "0", "name"])?,
        "mcp-echo",
        "{tools}"
    );
    assert_eq!(tools["result"]["ttlMs"], json!(0), "{tools}");
    assert_eq!(tools["result"]["cacheScope"], json!("private"), "{tools}");
    Ok(())
}

fn spawn_mcp_server(args: &[String]) -> Result<McpProcess, Box<dyn std::error::Error>> {
    let repo_root = repo_root()?;
    spawn_mcp_server_at(args, &repo_root, Some(&repo_root))
}

fn spawn_mcp_server_at(
    args: &[String],
    cwd: &Path,
    runx_cwd: Option<&Path>,
) -> Result<McpProcess, Box<dyn std::error::Error>> {
    let mut command_args = vec!["mcp".to_owned()];
    command_args.extend_from_slice(args);
    spawn_runx_stdio(&command_args, cwd, runx_cwd)
}

/// A runx command that speaks MCP over its stdio.
fn spawn_runx_stdio(
    args: &[String],
    cwd: &Path,
    runx_cwd: Option<&Path>,
) -> Result<McpProcess, Box<dyn std::error::Error>> {
    let mut command = isolated_runx_command_with_inherited_cwd("mcp-dogfood-test-key");
    command.current_dir(cwd);
    if let Some(runx_cwd) = runx_cwd {
        command.env("RUNX_CWD", runx_cwd);
    }
    let mut child = command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().ok_or("runx child stdin was not piped")?;
    let stdout = spawn_stdout_reader(
        child
            .stdout
            .take()
            .ok_or("runx child stdout was not piped")?,
    );
    let stderr = child
        .stderr
        .take()
        .ok_or("runx child stderr was not piped")?;
    Ok(McpProcess {
        child,
        stdin: Some(stdin),
        stdout,
        stderr: Some(stderr),
    })
}

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<Result<Value, String>>,
    stderr: Option<ChildStderr>,
}

impl McpProcess {
    fn stdin_mut(&mut self) -> Result<&mut ChildStdin, Box<dyn std::error::Error>> {
        self.stdin
            .as_mut()
            .ok_or_else(|| "runx child stdin is closed".into())
    }

    fn read_response(
        &mut self,
        label: &str,
        timeout: Duration,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        match self.stdout.recv_timeout(timeout) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                let _ignored = self.child.kill();
                let _ignored = self.child.wait();
                Err(format!("{label}: {error}; stderr: {}", self.stderr_string()?).into())
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ignored = self.child.kill();
                Err(format!(
                    "timed out waiting for runx mcp response to {label}; stderr: {}",
                    self.stderr_string()?
                )
                .into())
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(format!("runx mcp stdout reader disconnected before {label}").into())
            }
        }
    }

    fn close_stdin(&mut self) {
        let _closed = self.stdin.take();
    }

    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<ExitStatus, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ignored = self.child.kill();
                let _ignored = self.child.wait();
                return Err("timed out waiting for runx mcp to exit".into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn stderr_string(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        let mut text = String::new();
        if let Some(mut stderr) = self.stderr.take() {
            stderr.read_to_string(&mut text)?;
        }
        Ok(text)
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _closed = self.stdin.take();
        let _ignored = self.child.kill();
        let _ignored = self.child.wait();
    }
}

fn write_frame(stdin: &mut ChildStdin, message: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    stdin.write_all(&line)?;
    stdin.flush()?;
    Ok(())
}

fn spawn_stdout_reader(mut stdout: ChildStdout) -> Receiver<Result<Value, String>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        loop {
            match read_frame(&mut stdout) {
                Ok(value) => {
                    if sender.send(Ok(value)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ignored = sender.send(Err(error));
                    return;
                }
            }
        }
    });
    receiver
}

fn read_frame(stdout: &mut ChildStdout) -> Result<Value, String> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        stdout
            .read_exact(&mut byte)
            .map_err(|error| error.to_string())?;
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > 4 * 1024 * 1024 {
            return Err("MCP response line exceeded 4 MiB".to_owned());
        }
    }
    serde_json::from_slice(&line).map_err(|error| error.to_string())
}

fn initialize_request(id: i64) -> Value {
    request(
        id,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {
                "name": "runx-cli-dogfood-test",
                "version": "0.0.0",
            },
        }),
    )
}

fn initialized_notification() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {},
    })
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

fn path_text<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str, Box<dyn std::error::Error>> {
    let mut current = value;
    for segment in path {
        current = match current {
            Value::Array(values) => values
                .get(segment.parse::<usize>()?)
                .ok_or_else(|| format!("missing JSON array index {segment} in {value}"))?,
            Value::Object(record) => record
                .get(*segment)
                .ok_or_else(|| format!("missing JSON object key {segment} in {value}"))?,
            _ => {
                return Err(
                    format!("cannot descend into JSON path segment {segment} in {value}").into(),
                );
            }
        };
    }
    current
        .as_str()
        .ok_or_else(|| format!("JSON path {path:?} is not a string in {value}").into())
}

fn read_json_file(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn copy_mcp_echo_skill() -> Result<TestTempDir, Box<dyn std::error::Error>> {
    let skill_dir = TestTempDir::new("runx-mcp-dogfood-skill")?;
    let fixture = repo_root()?.join("fixtures/skills/mcp-echo");
    for file in ["SKILL.md", "run.yaml", "echo.mjs"] {
        fs::copy(fixture.join(file), skill_dir.path().join(file))?;
    }
    Ok(skill_dir)
}

fn write_workspace_env_mcp_skill(workspace: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = workspace.join("mcp-workspace-env");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: mcp-workspace-env
description: Return an allowlisted workspace value.
---

Return the allowlisted workspace marker.
"#,
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"skill: mcp-workspace-env
runners:
  default:
    default: true
    steps:
      - id: default
        tool: { module: marker.mjs, environment: { required: [MCP_DOGFOOD_MARKER] } }
        outputs:
          marker: string
"#,
    )?;
    fs::write(
        skill_dir.join("marker.mjs"),
        "export default (_inputs, context) => ({ marker: context.environment.MCP_DOGFOOD_MARKER });\n",
    )?;
    Ok(skill_dir)
}

fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

struct TestTempDir {
    path: PathBuf,
}

impl TestTempDir {
    fn new(prefix: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.path);
    }
}
