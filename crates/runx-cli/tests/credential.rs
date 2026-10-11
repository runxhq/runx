use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

const SECRET: &str = "credential-command-secret-sentinel";

#[test]
fn credential_command_sets_lists_and_removes_without_exposing_material()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-credential-command");
    let home = root.join("home");
    fs::create_dir_all(root.join(".runx"))?;

    let mut set = command(&root, &home)?;
    set.args([
        "credential",
        "set",
        "nitrosend",
        "--credential",
        "api_key",
        "--profile",
        "account-one",
        "--from-stdin",
        "--json",
    ]);
    let set = run_with_stdin(set, SECRET)?;
    assert_success_without_secret(&set)?;
    let set_json = serde_json::from_slice::<serde_json::Value>(&set.stdout)?;
    assert_eq!(set_json["credential"]["profile"], "account-one");
    let instance = &set_json["credential"]["instance"];
    assert_eq!(instance["instance_id"], "credential.nitrosend.account-one");
    assert_eq!(instance["authority"]["kind"], "credential");
    assert_eq!(instance["authority"]["credential"], "api_key");
    assert!(instance["credential_declaration_digest"].is_string());

    let list = command(&root, &home)?
        .args(["credential", "list", "--json"])
        .output()?;
    assert_success_without_secret(&list)?;
    let list_json = serde_json::from_slice::<serde_json::Value>(&list.stdout)?;
    assert_eq!(
        list_json["credential"]["profiles"][0]["name"],
        "account-one"
    );

    let remove = command(&root, &home)?
        .args(["credential", "remove", "account-one", "--json"])
        .output()?;
    assert_success_without_secret(&remove)?;
    let remove_json = serde_json::from_slice::<serde_json::Value>(&remove.stdout)?;
    assert_eq!(remove_json["credential"]["removed"], true);
    Ok(())
}

#[test]
fn credential_command_binds_a_connector_of_the_workspace_plugin()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-credential-plugin-connector");
    let home = root.join("home");
    fs::create_dir_all(root.join("connectors/notes"))?;
    fs::write(
        root.join("connectors/notes/connector.yaml"),
        r#"schema: runx.connector.v1
id: notes
name: Notes
description: A plugin's own notes service.
auth:
  credentials:
    token:
      kind: api_key
      fields: [{name: token}]
      delivery: {env: NOTES_TOKEN}
      audience: [api.notes.test]
placements:
  local: {auth: [credentials]}
operations:
  note.read:
    description: Read one note.
    verb: read
    scopes: [notes.read]
    credentials: [token]
    target: {template: {"$argument": "/id"}}
    input_schema: {type: object, required: [id], properties: {id: {type: string}}}
    implementation:
      http:
        allowed_hosts: [api.notes.test]
        auth: {token: {type: bearer}}
        requests:
          - {id: note, method: GET, url: {"$concat": ["https://api.notes.test/notes/", {"$argument": "/id"}]}}
        decode: [{op: auto_json}]
        result_paths: {note: acknowledgement}
"#,
    )?;

    let mut set = command(&root, &home)?;
    set.args([
        "credential",
        "set",
        "notes",
        "--credential",
        "token",
        "--from-stdin",
        "--json",
    ]);
    let set = run_with_stdin(set, SECRET)?;
    assert_success_without_secret(&set)?;
    let set_json = serde_json::from_slice::<serde_json::Value>(&set.stdout)?;
    assert_eq!(set_json["credential"]["instance"]["connector"], "notes");
    Ok(())
}

/// A plugin whose `note.read` runs a process tool with the instance's token,
/// and whose `note.peek` names a core native tool as its process.
fn write_notes_plugin(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("connectors/notes"))?;
    fs::write(
        root.join("connectors/notes/connector.yaml"),
        r#"schema: runx.connector.v1
id: notes
name: Notes
description: A plugin's own notes service, read by a local tool.
auth:
  credentials:
    token:
      kind: api_key
      fields: [{name: token}]
      delivery: {env: NOTES_TOKEN}
      audience: [notes.test]
placements:
  local: {auth: [credentials]}
operations:
  note.read:
    description: Read one note.
    verb: read
    scopes: [notes.read]
    credentials: [token]
    target: {template: {"$argument": "/id"}}
    input_schema: {type: object, required: [id], properties: {id: {type: string}}}
    implementation:
      process: {tool: notes.read}
  note.peek:
    description: Peek at one note.
    verb: read
    scopes: [notes.read]
    credentials: [token]
    target: {template: {"$argument": "/id"}}
    input_schema: {type: object, required: [id], properties: {id: {type: string}}}
    implementation:
      process: {tool: data.digest}
  note.lost:
    description: Read a note the service has lost.
    verb: read
    scopes: [notes.read]
    credentials: [token]
    target: {template: {"$argument": "/id"}}
    input_schema: {type: object, required: [id], properties: {id: {type: string}}}
    implementation:
      process: {tool: notes.lost}
"#,
    )?;
    let skill = root.join("skills/notes");
    fs::create_dir_all(skill.join("tools/notes/read"))?;
    fs::create_dir_all(skill.join("tools/notes/lost"))?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: notes\ndescription: Read one note.\n---\n\n# Notes\n\nRead one note.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        r#"skill: notes
version: "0.1.0"
connectors:
  notes:
    operations: [note.read, note.peek, note.lost]
runners:
  read:
    inputs:
      id: {type: string, required: true}
    steps:
      - id: read
        operation: notes/note.read
        input:
          id: $input.id
  peek:
    inputs:
      id: {type: string, required: true}
    steps:
      - id: peek
        operation: notes/note.peek
        input:
          id: $input.id
  lost:
    inputs:
      id: {type: string, required: true}
    steps:
      - id: lost
        operation: notes/note.lost
        input:
          id: $input.id
"#,
    )?;
    fs::write(
        skill.join("tools/notes/read/manifest.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "runx.tool.manifest.v1",
            "name": "notes.read",
            "version": "0.1.0",
            "description": "Read one note with the delivered token.",
            "source": {"command": "node", "args": ["./read.mjs"], "input_mode": "stdin"},
            "inputs": {"id": {"type": "string", "required": true}}
        }))?,
    )?;
    fs::write(
        skill.join("tools/notes/read/read.mjs"),
        "let input = '';\nprocess.stdin.on('data', (chunk) => { input += chunk; });\nprocess.stdin.on('end', () => {\n  const { id } = JSON.parse(input);\n  const token = process.env.NOTES_TOKEN ?? '';\n  process.stdout.write(JSON.stringify({ id, token_bytes: token.length }));\n});\n",
    )?;
    fs::write(
        skill.join("tools/notes/lost/manifest.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "runx.tool.manifest.v1",
            "name": "notes.lost",
            "version": "0.1.0",
            "description": "Fail to read one note after receiving the token.",
            "source": {"command": "node", "args": ["./lost.mjs"], "input_mode": "stdin"},
            "inputs": {"id": {"type": "string", "required": true}}
        }))?,
    )?;
    fs::write(
        skill.join("tools/notes/lost/lost.mjs"),
        "const token = process.env.NOTES_TOKEN ?? '';\nprocess.stderr.write(`note lost after ${token.length} token bytes`);\nprocess.exit(3);\n",
    )?;
    fs::write(root.join("inputs.json"), r#"{"id": "n1"}"#)?;
    Ok(())
}

fn step_receipt(
    receipts: &std::path::Path,
    step: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    for entry in walk_files(receipts)? {
        let Ok(receipt) =
            serde_json::from_slice::<serde_json::Value>(&fs::read(&entry).unwrap_or_default())
        else {
            continue;
        };
        if receipt["subject"]["ref"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with(&format!("#{step}")))
        {
            return Ok(receipt);
        }
    }
    Err(format!("no receipt for step {step}").into())
}

#[test]
fn a_process_operation_receives_its_credential_and_its_receipt_records_the_delivery()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-process-operation-credential");
    let home = root.join("home");
    write_notes_plugin(&root)?;
    let mut set = command(&root, &home)?;
    set.args([
        "credential",
        "set",
        "notes",
        "--credential",
        "token",
        "--from-stdin",
        "--json",
    ]);
    assert_success_without_secret(&run_with_stdin(set, SECRET)?)?;

    let receipts = root.join("receipts");
    let read = command(&root, &home)?
        .arg("skill")
        .arg(root.join("skills/notes"))
        .args(["read", "--inputs", "inputs.json", "--receipt-dir"])
        .arg(&receipts)
        .arg("--json")
        .output()?;
    assert_success_without_secret(&read)?;
    let output = serde_json::from_slice::<serde_json::Value>(&read.stdout)?;
    assert_eq!(
        output["result"]["provider_operation"]["data"]["result"]["token_bytes"],
        SECRET.len(),
        "{output}"
    );

    let receipt = step_receipt(&receipts, "read")?;
    assert_eq!(
        receipt["authority"]["enforcement"]["execution_boundary"]["kind"],
        "trusted_host_process"
    );
    // The kernel handed the process its instance's token: the receipt records
    // that delivery by reference, never the token.
    let outcome = &receipt["seal"]["criteria"][0];
    assert_eq!(outcome["criterion_id"], "step_outcome");
    assert!(
        outcome["verification_refs"]
            .as_array()
            .is_some_and(|refs| refs
                .iter()
                .any(|reference| reference["type"] == "credential")),
        "{receipt}"
    );
    for entry in walk_files(&receipts)? {
        assert!(
            !fs::read_to_string(&entry)
                .unwrap_or_default()
                .contains(SECRET)
        );
    }
    Ok(())
}

#[test]
fn a_failed_process_operation_still_records_the_credential_it_received()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-process-operation-failed");
    let home = root.join("home");
    write_notes_plugin(&root)?;
    let mut set = command(&root, &home)?;
    set.args([
        "credential",
        "set",
        "notes",
        "--credential",
        "token",
        "--from-stdin",
        "--json",
    ]);
    assert_success_without_secret(&run_with_stdin(set, SECRET)?)?;

    let receipts = root.join("receipts");
    let lost = command(&root, &home)?
        .arg("skill")
        .arg(root.join("skills/notes"))
        .args(["lost", "--inputs", "inputs.json", "--receipt-dir"])
        .arg(&receipts)
        .arg("--json")
        .output()?;
    let stdout = String::from_utf8(lost.stdout.clone())?;
    assert!(!stdout.contains(SECRET));
    let output = serde_json::from_str::<serde_json::Value>(&stdout)?;
    assert_eq!(output["closure"]["disposition"], "failed", "{output}");

    // The process ran with the token and failed: its receipt records where it
    // ran and the delivery, as a successful run's does.
    let receipt = step_receipt(&receipts, "lost")?;
    assert_eq!(
        receipt["authority"]["enforcement"]["execution_boundary"]["kind"], "trusted_host_process",
        "{receipt}"
    );
    let outcome = &receipt["seal"]["criteria"][0];
    assert!(
        outcome["verification_refs"]
            .as_array()
            .is_some_and(|refs| refs
                .iter()
                .any(|reference| reference["type"] == "credential")),
        "{receipt}"
    );
    for entry in walk_files(&receipts)? {
        assert!(
            !fs::read_to_string(&entry)
                .unwrap_or_default()
                .contains(SECRET)
        );
    }
    Ok(())
}

#[test]
fn a_process_implementation_runs_only_a_process_tool_never_a_native_capability()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-process-operation-native");
    let home = root.join("home");
    write_notes_plugin(&root)?;
    let mut set = command(&root, &home)?;
    set.args([
        "credential",
        "set",
        "notes",
        "--credential",
        "token",
        "--from-stdin",
        "--json",
    ]);
    assert_success_without_secret(&run_with_stdin(set, SECRET)?)?;

    let receipts = root.join("receipts");
    let peek = command(&root, &home)?
        .arg("skill")
        .arg(root.join("skills/notes"))
        .args(["peek", "--inputs", "inputs.json", "--receipt-dir"])
        .arg(&receipts)
        .arg("--json")
        .output()?;
    let stdout = String::from_utf8(peek.stdout.clone())?;
    assert!(!stdout.contains(SECRET));
    let output = serde_json::from_str::<serde_json::Value>(&stdout)?;
    assert_eq!(output["closure"]["disposition"], "failed", "{output}");
    assert!(
        output["result"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Imported tool 'data.digest' was not found")),
        "{output}"
    );
    Ok(())
}

#[test]
fn secret_config_rejects_argv_and_accepts_stdin_without_plaintext_at_rest()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-secret-config-stdin");
    let home = root.join("home");
    fs::create_dir_all(&root)?;

    let rejected = command(&root, &home)?
        .args(["config", "set", "api-key", SECRET, "--json"])
        .output()?;
    assert!(!rejected.status.success());
    assert!(!String::from_utf8(rejected.stdout)?.contains(SECRET));
    assert!(!String::from_utf8(rejected.stderr)?.contains(SECRET));

    let mut set = command(&root, &home)?;
    set.args(["config", "set", "api-key", "--from-stdin", "--json"]);
    let set = run_with_stdin(set, SECRET)?;
    assert_success_without_secret(&set)?;
    assert!(String::from_utf8(set.stdout)?.contains("[encrypted]"));

    for entry in walk_files(&home)? {
        assert!(
            !fs::read_to_string(&entry)
                .unwrap_or_default()
                .contains(SECRET),
            "secret config material must not be stored as plaintext in {}",
            entry.display()
        );
    }
    Ok(())
}

fn command(
    root: &std::path::Path,
    home: &std::path::Path,
) -> Result<Command, Box<dyn std::error::Error>> {
    let mut command =
        crate::support::isolated_runx_command_with_inherited_cwd("credential-command-test-key");
    command.current_dir(root).env("RUNX_HOME", home);
    Ok(command)
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
        .ok_or("credential stdin was not piped")?
        .write_all(value.as_bytes())?;
    Ok(child.wait_with_output()?)
}

fn assert_success_without_secret(
    output: &std::process::Output,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = String::from_utf8(output.stdout.clone())?;
    let stderr = String::from_utf8(output.stderr.clone())?;
    assert!(
        output.status.success(),
        "credential command failed: {stderr}\n{stdout}"
    );
    assert!(!stdout.contains(SECRET));
    assert!(!stderr.contains(SECRET));
    Ok(())
}

fn walk_files(root: &std::path::Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}
