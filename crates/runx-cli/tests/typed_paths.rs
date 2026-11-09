//! A path an operator types names the directory they typed it in, for every
//! command that takes one. Each test types one inside a package, `inner`, in
//! a workspace whose root is itself a package, `workspace`, so a path read
//! from the workspace root names the wrong package or the wrong file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// `inner` digests the note it's given, and has a test; the workspace's own
/// package has a method and no test, which doctor reports.
struct Typed {
    base: PathBuf,
}

impl Typed {
    fn new(name: &str) -> TestResult<Self> {
        let typed = Self {
            base: crate::support::temp_root(name),
        };
        let root = typed.root();
        fs::create_dir_all(root.join("home"))?;
        fs::write(
            root.join("SKILL.md"),
            "---\nname: workspace\ndescription: The workspace's own package.\n---\n# Workspace\n",
        )?;
        fs::write(root.join("run.yaml"), digest_profile("workspace"))?;
        let inner = typed.inner();
        fs::create_dir_all(inner.join("tests"))?;
        fs::write(
            inner.join("SKILL.md"),
            "---\nname: inner\ndescription: Digests the note it's given.\n---\n# Inner\n",
        )?;
        fs::write(inner.join("run.yaml"), digest_profile("inner"))?;
        fs::write(
            inner.join("tests/note.yaml"),
            "name: note\nmethod: run\ninputs:\n  note: typed\nexpect:\n  status: sealed\n",
        )?;
        Ok(typed)
    }

    fn root(&self) -> PathBuf {
        self.base.join("ws")
    }

    fn inner(&self) -> PathBuf {
        self.root().join("skills/inner")
    }

    /// `runx <args>` typed in `inner`, with no workspace named, so runx finds
    /// the workspace from there as an operator's shell would.
    fn runx_in(&self, dir: &Path, args: &[&str]) -> TestResult<Output> {
        Ok(crate::support::unsigned_runx_command_with_inherited_cwd()
            .current_dir(dir)
            .env("RUNX_HOME", self.root().join("home"))
            .args(args)
            .output()?)
    }

    fn runx(&self, args: &[&str]) -> TestResult<Output> {
        self.runx_in(&self.inner(), args)
    }
}

fn digest_profile(name: &str) -> String {
    format!(
        "minimum_runx: 0.12.0\nmethods:\n  run:\n    description: Digest the note {name} is given.\n    inputs:\n      note: {{type: string}}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.note\n          encoding: utf8_text\n"
    )
}

fn succeeded(output: &Output) -> TestResult<Value> {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn has_files(dir: &Path) -> TestResult<bool> {
    Ok(dir.is_dir() && fs::read_dir(dir)?.next().is_some())
}

#[test]
fn a_skill_reads_its_inputs_and_writes_its_receipts_where_they_are_typed() -> TestResult {
    let typed = Typed::new("runx-typed-skill")?;
    fs::write(typed.inner().join("input.json"), r#"{"note": "typed"}"#)?;
    succeeded(&typed.runx(&[
        "skill",
        ".",
        "--inputs",
        "input.json",
        "--receipt-dir",
        "receipts",
        "--json",
    ])?)?;
    assert!(has_files(&typed.inner().join("receipts"))?);
    assert!(!typed.root().join("receipts").exists());
    Ok(())
}

#[test]
fn a_test_writes_its_receipts_where_they_are_typed() -> TestResult {
    let typed = Typed::new("runx-typed-test")?;
    succeeded(&typed.runx(&["test", ".", "--receipt-dir", "receipts", "--json"])?)?;
    assert!(has_files(&typed.inner().join("receipts"))?);
    assert!(!typed.root().join("receipts").exists());
    Ok(())
}

#[test]
fn verify_reads_receipts_where_they_are_typed() -> TestResult {
    let typed = Typed::new("runx-typed-verify")?;
    let receipts = typed.inner().join("receipts");
    succeeded(&typed.runx(&[
        "skill",
        ".",
        "-i",
        "note=typed",
        "--receipt-dir",
        receipts.to_str().ok_or("non-utf8 receipts")?,
        "--json",
    ])?)?;
    let tree = succeeded(&typed.runx(&["verify", "--receipt-dir", "receipts", "--json"])?)?;
    let read = tree["receipt_dir"].as_str().ok_or("no receipt dir")?;
    assert!(
        Path::new(read).ends_with("ws/skills/inner/receipts"),
        "{tree}"
    );
    assert!(
        tree["trees"][0]["receipt_count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "{tree}"
    );
    let receipt = fs::read_dir(&receipts)?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| name.ends_with(".json") && !name.starts_with('.'))
        .ok_or("no receipt file")?;
    succeeded(&typed.runx(&[
        "verify",
        "--receipt",
        &format!("receipts/{receipt}"),
        "--json",
    ])?)?;
    Ok(())
}

#[test]
fn resume_writes_its_receipts_where_they_are_typed() -> TestResult {
    let typed = Typed::new("runx-typed-resume")?;
    let gate = typed.root().join("skills/gate");
    fs::create_dir_all(&gate)?;
    fs::write(
        gate.join("SKILL.md"),
        "---\nname: gate\ndescription: Waits on one approval.\n---\n# Gate\n",
    )?;
    fs::write(
        gate.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: approve\n        ask: {gate: typed.gate, reason: approve the typed path fixture}\n",
    )?;
    let paused = typed.runx(&["skill", "../gate", "--json"])?;
    assert_eq!(
        paused.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&paused.stderr)
    );
    let paused: Value = serde_json::from_slice(&paused.stdout)?;
    let run_id = paused["run_id"].as_str().ok_or("no run id")?;
    let request_id = paused["requests"][0]["id"].as_str().ok_or("no request")?;
    let read = succeeded(&typed.runx(&["history", run_id, "--json"])?)?;
    fs::write(
        typed.inner().join("answers.json"),
        crate::support::bind_resume(&read, json!({"approvals": { request_id: false }}))?
            .to_string(),
    )?;
    let resumed = succeeded(&typed.runx(&[
        "resume",
        run_id,
        "answers.json",
        "--receipt-dir",
        "receipts",
        "--json",
    ])?)?;
    assert_eq!(resumed["status"], "sealed", "{resumed}");
    assert!(has_files(&typed.inner().join("receipts"))?);
    assert!(!typed.root().join("receipts").exists());
    Ok(())
}

#[test]
fn mcp_serves_the_package_where_it_is_typed() -> TestResult {
    let typed = Typed::new("runx-typed-mcp")?;
    let mut server = crate::support::unsigned_runx_command_with_inherited_cwd()
        .current_dir(typed.inner())
        .env("RUNX_HOME", typed.root().join("home"))
        .args(["mcp", "."])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let mut stdin = server.stdin.take().ok_or("no stdin")?;
        for message in [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "runx-typed-paths", "version": "0.0.0"},
            }}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        ] {
            let mut line = serde_json::to_vec(&message)?;
            line.push(b'\n');
            stdin.write_all(&line)?;
        }
    }
    let served = server.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&served.stdout);
    let tools = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|message| message["id"] == 2)
        .ok_or_else(|| {
            format!(
                "no tools/list answer\nstdout={stdout}\nstderr={}",
                String::from_utf8_lossy(&served.stderr)
            )
        })?;
    let names = tools["result"]["tools"]
        .as_array()
        .ok_or("tools is a list")?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(
        names.contains(&"inner") && !names.contains(&"workspace"),
        "{tools}"
    );
    Ok(())
}

#[test]
fn export_exports_the_package_where_it_is_typed() -> TestResult {
    let typed = Typed::new("runx-typed-export")?;
    let report = succeeded(&typed.runx(&["export", "claude", ".", "--project", "--json"])?)?;
    let exported = report["exported"]
        .as_array()
        .ok_or("exported is a list")?
        .iter()
        .filter_map(|entry| entry["skill"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(exported, ["inner"], "{report}");
    Ok(())
}

#[test]
fn doctor_checks_the_package_where_it_is_typed() -> TestResult {
    let typed = Typed::new("runx-typed-doctor")?;
    let report = succeeded(&typed.runx(&["doctor", ".", "--json"])?)?;
    assert_eq!(report["diagnostics"], json!([]), "{report}");
    Ok(())
}

#[test]
fn tool_inspect_finds_a_tool_beside_where_it_is_typed() -> TestResult {
    let typed = Typed::new("runx-typed-tool")?;
    let source = crate::support::repo_root()?
        .join("fixtures/tool-catalogs/inspect/tool-roots/local/fixture/local_echo");
    let tool = typed.inner().join(".runx/tools/fixture/local_echo");
    fs::create_dir_all(&tool)?;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        fs::copy(entry.path(), tool.join(entry.file_name()))?;
    }
    let inspected =
        succeeded(&typed.runx(&["tool", "inspect", "fixture.local_echo", "--json"])?)?;
    let found = inspected["tool"]["reference_path"]
        .as_str()
        .ok_or("no reference path")?;
    assert!(
        found.contains("ws/skills/inner/.runx/tools/fixture/local_echo"),
        "{inspected}"
    );
    Ok(())
}

#[test]
fn authority_save_names_the_workspace_where_it_is_typed() -> TestResult {
    let typed = Typed::new("runx-typed-authority")?;
    let other = typed.base.join("other");
    fs::create_dir_all(other.join("skills"))?;
    let authority = json!({
        "delegations": [{
            "connector": "private",
            "terms": [{
                "verbs": ["read"],
                "bounds": {
                    "operations": ["private/*"],
                    "scopes": ["store.read"],
                    "targets": ["project:inner/said:*"],
                    "packages": ["project:inner"],
                },
            }],
        }],
    })
    .to_string();
    fs::write(typed.inner().join("authority.json"), &authority)?;
    fs::write(other.join("authority.json"), &authority)?;
    let made_there = succeeded(&typed.runx_in(
        &other,
        &["authority", "save", "there", "authority.json", "--json"],
    )?)?;
    let named = succeeded(&typed.runx(&[
        "authority",
        "save",
        "named",
        "authority.json",
        "--workspace",
        "../../../other",
        "--json",
    ])?)?;
    assert_eq!(
        named["workspaces"], made_there["workspaces"],
        "named={named}\nmade there={made_there}"
    );
    Ok(())
}

#[test]
fn registry_install_reads_its_records_where_they_are_typed() -> TestResult {
    let typed = Typed::new("runx-typed-records")?;
    fs::write(typed.inner().join("records.json"), "not records")?;
    let output = typed.runx(&["registry", "install", "--records", "records.json", "--json"])?;
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{said}");
    assert!(
        said.contains("is one JSON array of registry version records"),
        "the records typed in inner weren't the ones read: {said}"
    );
    Ok(())
}
