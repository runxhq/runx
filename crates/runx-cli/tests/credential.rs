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
