//! End-to-end proof for declared installed-skill credentials, workspace
//! environment fallback, encrypted local profiles, CLI-tool delivery, and
//! output redaction.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const SECRET: &str = "ghs_cli_local_provision_secret_value";

#[test]
fn workspace_env_does_not_block_native_help() -> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-workspace-env-help");
    fs::create_dir_all(&temp)?;
    fs::write(temp.join(".env"), format!("PROBE_TOKEN='{SECRET}\n"))?;

    let output = native_command()?
        .current_dir(&temp)
        .arg("skill")
        .arg("--help")
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(output.status.success(), "native help failed: {stderr}");
    assert!(stdout.contains("runx skill"));
    assert!(!stdout.contains(SECRET) && !stderr.contains(SECRET));
    Ok(())
}

#[test]
fn malformed_workspace_env_fails_json_safe_without_secret_exposure()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-workspace-env-malformed");
    fs::create_dir_all(&temp)?;
    let skill_dir = write_env_probe_skill(&temp)?;
    fs::write(temp.join(".env"), format!("PROBE_TOKEN='{SECRET}\n"))?;

    let output = native_command()?
        .current_dir(&temp)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--json")
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    let value = serde_json::from_slice::<Value>(&output.stdout)?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(value["status"], "failure");
    assert_eq!(value["error"]["code"], "workspace_env_error");
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("invalid syntax"))
    );
    assert!(stderr.is_empty());
    assert!(!stdout.contains(SECRET) && !stderr.contains(SECRET));
    Ok(())
}

#[test]
fn process_tool_receives_allowlisted_workspace_env_without_wrapper()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-workspace-env");
    fs::create_dir_all(&temp)?;
    let skill_dir = write_env_probe_skill(&temp)?;
    fs::write(temp.join(".env"), format!("PROBE_TOKEN={SECRET}\n"))?;

    let output = native_command()?
        .current_dir(&temp)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--json")
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "workspace env run failed: {stderr}\n{stdout}"
    );
    let value = serde_json::from_str::<Value>(&stdout)?;
    assert_eq!(value["result"]["configured"], true);
    assert!(!stdout.contains(SECRET) && !stderr.contains(SECRET));
    Ok(())
}

#[test]
fn workspace_env_is_loaded_from_discovered_project_root() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = crate::support::temp_root("runx-cli-workspace-env-nested");
    let nested = temp.join("nested/operator");
    let runx_dir = temp.join(".runx");
    fs::create_dir_all(&nested)?;
    fs::create_dir_all(&runx_dir)?;
    fs::write(runx_dir.join("project.json"), valid_project_state())?;
    let skill_dir = write_env_probe_skill(&temp)?;
    fs::write(temp.join(".env"), format!("PROBE_TOKEN={SECRET}\n"))?;

    let output = native_command()?
        .current_dir(&nested)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--json")
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "nested workspace env run failed: {stderr}\n{stdout}"
    );
    let value = serde_json::from_str::<Value>(&stdout)?;
    assert_eq!(value["result"]["configured"], true);
    assert!(!stdout.contains(SECRET) && !stderr.contains(SECRET));
    Ok(())
}

#[test]
fn workspace_env_supports_quoted_values() -> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-workspace-env-quoted");
    fs::create_dir_all(&temp)?;
    let skill_dir = write_env_probe_skill(&temp)?;
    fs::write(
        temp.join(".env"),
        "PROBE_TOKEN=\"quoted # value\"\nEXPECTED_TOKEN=\"quoted # value\"\n",
    )?;

    let output = native_command()?
        .current_dir(&temp)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--json")
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "quoted workspace env run failed: {stderr}\n{stdout}"
    );
    let value = serde_json::from_str::<Value>(&stdout)?;
    assert_eq!(value["result"]["configured"], true);
    assert!(!stdout.contains("quoted # value") && !stderr.contains("quoted # value"));
    Ok(())
}

#[test]
fn workspace_env_remains_blocked_when_not_declared() -> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-workspace-env-denied");
    fs::create_dir_all(&temp)?;
    let skill_dir = write_env_denial_skill(&temp)?;
    fs::write(temp.join(".env"), format!("PROBE_TOKEN={SECRET}\n"))?;

    let output = native_command()?
        .current_dir(&temp)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--json")
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "deny-by-default workspace env run failed: {stderr}\n{stdout}"
    );
    let value = serde_json::from_str::<Value>(&stdout)?;
    assert_eq!(value["result"]["blocked"], true);
    assert!(!stdout.contains(SECRET) && !stderr.contains(SECRET));
    Ok(())
}

#[test]
fn process_env_takes_precedence_over_workspace_env() -> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-workspace-env-precedence");
    fs::create_dir_all(&temp)?;
    let skill_dir = write_env_probe_skill(&temp)?;
    fs::write(temp.join(".env"), "PROBE_TOKEN=from-file\n")?;

    let output = native_command()?
        .current_dir(&temp)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--json")
        .env("PROBE_TOKEN", SECRET)
        .env("EXPECTED_TOKEN", SECRET)
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "workspace env precedence run failed: {stderr}\n{stdout}"
    );
    let value = serde_json::from_str::<Value>(&stdout)?;
    assert_eq!(value["result"]["configured"], true);
    assert!(!stdout.contains(SECRET) && !stderr.contains(SECRET));
    Ok(())
}

#[test]
fn resume_loads_workspace_env_from_discovered_project_root()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-workspace-env-resume");
    let nested = temp.join("nested/operator");
    let runx_dir = temp.join(".runx");
    fs::create_dir_all(&nested)?;
    fs::create_dir_all(&runx_dir)?;
    fs::write(runx_dir.join("project.json"), valid_project_state())?;
    let skill_dir = write_resume_env_skill(&temp)?;
    let receipt_dir = temp.join("receipts");
    fs::write(temp.join(".env"), "RESUME_PROBE_TOKEN=before-resume\n")?;

    let pause = native_command()?
        .current_dir(&nested)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    assert_eq!(pause.status.code(), Some(2));
    let pause_json = serde_json::from_slice::<Value>(&pause.stdout)?;
    assert_eq!(pause_json["status"], "needs_agent");
    let run_id = pause_json["run_id"].as_str().ok_or("missing run id")?;

    // Resume is a new command and must capture the current workspace snapshot,
    // rather than retaining the pause command's values.
    fs::write(temp.join(".env"), "RESUME_PROBE_TOKEN=after-resume\n")?;
    let answers_path = temp.join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "approve": {
                    "approved": true
                }
            }
        })
        .to_string(),
    )?;

    let resume = native_command()?
        .current_dir(&nested)
        .arg("resume")
        .arg(run_id)
        .arg(&answers_path)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    let resume_json = serde_json::from_slice::<Value>(&resume.stdout)?;
    assert!(
        resume.status.success(),
        "resume failed: {}\n{}",
        String::from_utf8_lossy(&resume.stderr),
        String::from_utf8_lossy(&resume.stdout)
    );
    assert_eq!(resume_json["status"], "sealed");
    assert!(!String::from_utf8(resume.stdout)?.contains("after-resume"));
    assert!(!String::from_utf8(resume.stderr)?.contains("after-resume"));
    Ok(())
}

#[test]
fn inline_graph_process_tool_preserves_timeout_policy() -> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-inline-graph-timeout");
    fs::create_dir_all(&temp)?;
    let skill_dir = write_inline_graph_timeout_skill(&temp)?;

    let started = Instant::now();
    let output = native_command()?
        .current_dir(&temp)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--json")
        .output()?;
    let elapsed = started.elapsed();
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;

    assert!(
        elapsed < Duration::from_secs(4),
        "inline graph ignored its one-second timeout and ran for {elapsed:?}"
    );
    assert!(
        !output.status.success(),
        "timed-out inline graph unexpectedly succeeded: {stderr}\n{stdout}"
    );
    Ok(())
}

#[test]
fn cli_rejects_empty_secret_value() -> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-local-credential-empty");
    crate::support::plugin_with_source_connectors(&temp, &["github"])?;
    let mut command = native_command()?;
    command.current_dir(&temp).args([
        "credential",
        "set",
        "github",
        "--credential",
        "token",
        "--from-stdin",
        "--json",
    ]);
    let output = run_with_stdin(command, "")?;

    assert!(
        !output.status.success(),
        "expected an empty stdin credential to be rejected"
    );
    let message = json_failure_message(&output.stdout)?;
    assert!(
        message.contains("must not be empty"),
        "expected an error about the empty secret value, got: {message}"
    );

    Ok(())
}

#[test]
fn cli_rejects_secret_env_value_on_argv() -> Result<(), Box<dyn std::error::Error>> {
    let temp = crate::support::temp_root("runx-cli-local-credential-argv-secret");
    fs::create_dir_all(&temp)?;
    let output = native_command()?
        .current_dir(&temp)
        .arg("credential")
        .arg("set")
        .arg("github")
        .arg(SECRET)
        .arg("--from-stdin")
        .arg("--json")
        .output()?;

    assert!(
        !output.status.success(),
        "expected argv secret material to be rejected"
    );
    let message = json_failure_message(&output.stdout)?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        message.contains("must be provided through --from-stdin"),
        "expected an error about argv secret material, got: {message}"
    );
    assert!(
        stderr.is_empty(),
        "json failures should keep stderr clean, got: {stderr}"
    );
    assert!(
        !stdout.contains(SECRET) && !stderr.contains(SECRET),
        "raw secret leaked into the error output"
    );

    Ok(())
}

fn native_command() -> Result<Command, Box<dyn std::error::Error>> {
    Ok(crate::support::isolated_runx_command_with_inherited_cwd(
        "local-credential-test-key",
    ))
}

fn run_with_stdin(
    mut command: Command,
    value: &str,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("credential command stdin was not piped")?
        .write_all(value.as_bytes())?;
    Ok(child.wait_with_output()?)
}

fn valid_project_state() -> &'static str {
    r#"{"version":1,"project_id":"proj_workspace_env_test","created_at":"2026-07-16T00:00:00Z"}"#
}

fn json_failure_message(stdout: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    let value = serde_json::from_slice::<Value>(stdout)?;
    assert_eq!(value["status"], "failure");
    Ok(value["error"]["message"]
        .as_str()
        .ok_or("missing failure message")?
        .to_owned())
}

fn write_env_probe_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("env-probe");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: env-probe\ndescription: env-probe fixture.\n---\n# Environment Probe\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: env-probe
runners:
  probe:
    default: true
    steps:
      - id: probe
        tool: { command: sh, args: ["-c", 'test -n "$PROBE_TOKEN" && { test -z "$EXPECTED_TOKEN" || test "$PROBE_TOKEN" = "$EXPECTED_TOKEN"; } && printf ''{"configured":true}'''], environment: { required: [PROBE_TOKEN], optional: [EXPECTED_TOKEN] } }
        outputs:
          configured: boolean
"#,
    )?;
    Ok(skill_dir)
}

fn write_env_denial_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("env-denial");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: env-denial\ndescription: env-denial fixture.\n---\n# Environment Denial\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: env-denial
runners:
  probe:
    default: true
    steps:
      - id: probe
        tool: { command: sh, args: ["-c", 'test -z "$PROBE_TOKEN" && printf ''{"blocked":true}'''] }
        outputs:
          blocked: boolean
"#,
    )?;
    Ok(skill_dir)
}

fn write_resume_env_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("resume-env");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: resume-env\ndescription: resume-env fixture.\n---\n# Resume Environment\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: resume-env
runners:
  resume-env:
    default: true
    result_from: [approve]
    steps:
      - id: approve
        agent: { role: reviewer, task: approve-workspace-env }
        outputs:
          approved: boolean
      - id: probe
        tool: { command: sh, args: ["-c", 'payload="$(cat)"; if test -z "$payload"; then echo missing-stdin >&2; exit 9; elif test -z "$RESUME_PROBE_TOKEN"; then echo missing-probe >&2; exit 10; elif test "$RESUME_PROBE_TOKEN" != "after-resume"; then echo stale-probe >&2; exit 11; fi'], timeout_seconds: 5, input_mode: stdin, environment: { required: [RESUME_PROBE_TOKEN] } }
"#,
    )?;
    Ok(skill_dir)
}

fn write_inline_graph_timeout_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("inline-graph-timeout");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: inline-graph-timeout\ndescription: inline-graph-timeout fixture.\n---\n# Inline Graph Timeout\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: inline-graph-timeout
runners:
  timeout:
    default: true
    steps:
      - id: timeout
        tool: { command: sh, args: ["-c", "sleep 5"], timeout_seconds: 1 }
        outputs:
          slept: boolean
"#,
    )?;
    Ok(skill_dir)
}
