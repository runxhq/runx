use std::io::Write;
use std::process::Stdio;

#[test]
fn parser_eval_accepts_stdin_json() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = crate::support::repo_runx_command()
        .args(["parser", "eval", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(
            br#"{"kind":"parser.validateSkillMarkdown","markdown":"---\nname: portable-agent\ndescription: Portable agent skill\n---\n# Portable agent\n"}"#,
        )?;
    }

    let output = child.wait_with_output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "success");
    assert_eq!(value["result"]["kind"], "output");
    assert_eq!(value["result"]["value"]["name"], "portable-agent");
    Ok(())
}

#[test]
fn parser_eval_validates_graph_yaml() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = crate::support::repo_runx_command()
        .args(["parser", "eval", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(
            br#"{"kind":"parser.validateGraphYaml","yaml":"name: gx\nsteps:\n  - id: one\n    tool:\n      command: node\n      args: [\"-e\", \"process.stdout.write('{}')\"]\n"}"#,
        )?;
    }

    let output = child.wait_with_output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "success");
    assert_eq!(value["result"]["value"]["name"], "gx");
    assert_eq!(value["result"]["value"]["steps"][0]["id"], "one");
    Ok(())
}

#[test]
fn parser_eval_usage_errors_exit_64() -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::support::repo_runx_command()
        .args(["parser", "eval", "--input", "-"])
        .output()?;

    assert_eq!(output.status.code(), Some(64));
    assert!(String::from_utf8(output.stderr)?.contains("runx parser eval requires --json"));
    Ok(())
}

#[test]
fn parser_eval_unknown_kind_returns_structured_error() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = crate::support::repo_runx_command()
        .args(["parser", "eval", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(br#"{"kind":"parser.unknown"}"#)?;
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
            .is_some_and(|message| message.contains("unsupported parser input kind"))
    );
    Ok(())
}

#[test]
fn parser_eval_batches_documents_and_returns_independent_errors()
-> Result<(), Box<dyn std::error::Error>> {
    let mut child = crate::support::repo_runx_command()
        .args(["parser", "eval", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(
            br#"{
              "inputs": [
                {
                  "kind": "parser.validateHarnessFixtureYaml",
                  "yaml": "name: ready\nkind: skill\ntarget: ..\nexpect:\n  status: sealed\n"
                },
                {
                  "kind": "parser.validateHarnessFixtureYaml",
                  "yaml": "not: a-fixture\n"
                }
              ],
              "returnErrors": true
            }"#,
        )?;
    }

    let output = child.wait_with_output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["result"]["value"][0]["status"], "success");
    assert_eq!(value["result"]["value"][0]["value"]["name"], "ready");
    assert_eq!(value["result"]["value"][1]["status"], "failure");
    assert_eq!(
        value["result"]["value"][1]["error"]["code"],
        "validation_error"
    );
    Ok(())
}
