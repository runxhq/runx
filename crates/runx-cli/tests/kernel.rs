use std::io::Write;
use std::process::Stdio;

#[test]
fn kernel_eval_state_machine_fixture_json() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/kernel/state-machine/sequential-plan-first-step.json"
    ))?;
    let mut child = crate::support::repo_runx_command()
        .args(["kernel", "eval", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(fixture["input"].to_string().as_bytes())?;
    }
    let output = child.wait_with_output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "success");
    assert_eq!(value["result"]["kind"], "output");
    assert_eq!(value["result"]["value"]["type"], "run_step");
    assert_eq!(value["result"]["value"]["stepId"], "first");
    assert_eq!(value["result"]["value"]["attempt"], 1);
    Ok(())
}

#[test]
fn kernel_eval_accepts_stdin_json() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = crate::support::repo_runx_command()
        .args(["kernel", "eval", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(
            br#"{"kind":"state-machine.createSequentialGraphState","graphId":"only","steps":[{"id":"first"}]}"#,
        )?;
    }

    let output = child.wait_with_output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "success");
    assert_eq!(value["result"]["value"]["graphId"], "only");
    assert_eq!(value["result"]["value"]["status"], "pending");
    Ok(())
}

#[test]
fn kernel_eval_usage_errors_exit_64() -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::support::repo_runx_command()
        .args(["kernel", "eval", "--input", "-"])
        .output()?;

    assert_eq!(output.status.code(), Some(64));
    assert!(String::from_utf8(output.stderr)?.contains("runx kernel eval requires --json"));
    Ok(())
}

#[test]
fn kernel_eval_invalid_json_returns_structured_error() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = crate::support::repo_runx_command()
        .args(["kernel", "eval", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"not json")?;
    }

    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "failure");
    assert_eq!(value["error"]["code"], "invalid_document");
    Ok(())
}

#[test]
fn kernel_eval_unknown_kind_returns_structured_error() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = crate::support::repo_runx_command()
        .args(["kernel", "eval", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(br#"{"kind":"policy.unknown"}"#)?;
    }

    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "failure");
    assert_eq!(value["error"]["code"], "invalid_input");
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("unsupported kernel input kind"))
    );
    Ok(())
}
