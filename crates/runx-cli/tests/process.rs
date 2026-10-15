//! Detached processes through the real binary: a process started by one run
//! outlives it under its own supervisor, later runs type into its terminal
//! and resize it, and the person reads, lists and stops it.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn runx(root: &Path) -> Command {
    let mut command = crate::support::signed_runx_command("process-test-key");
    command
        .current_dir(root)
        .env("RUNX_CWD", root)
        .env("RUNX_HOME", root.join("home"));
    command
}

fn skill(root: &Path, name: &str, profile: &str) -> TestResult {
    let directory = root.join("skills").join(name);
    fs::create_dir_all(&directory)?;
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: A process test step.\n---\n# {name}\n\nRun it.\n"),
    )?;
    fs::write(directory.join("run.yaml"), profile)?;
    Ok(())
}

fn skills(root: &Path) -> TestResult {
    skill(
        root,
        "proc-start",
        r#"skill: proc-start
runners:
  start:
    default: true
    inputs:
      args: { type: array, required: true }
    steps:
      - id: start
        tool: process.start
        scopes: [process.exec]
        inputs:
          command: sh
          args: $input.args
          max_seconds: 60
"#,
    )?;
    skill(
        root,
        "proc-tty",
        r#"skill: proc-tty
runners:
  start:
    default: true
    inputs:
      args: { type: array, required: true }
    steps:
      - id: start
        tool: process.start
        scopes: [process.exec]
        inputs:
          command: sh
          args: $input.args
          max_seconds: 60
          tty: { rows: 24, cols: 80 }
"#,
    )?;
    skill(
        root,
        "cmd",
        r#"skill: cmd
runners:
  run:
    default: true
    inputs:
      args: { type: array, required: true }
    steps:
      - id: run
        tool: command.execute
        scopes: [process.exec]
        inputs:
          command: sh
          args: $input.args
          output_mode: text
"#,
    )?;
    skill(
        root,
        "proc-write",
        r#"skill: proc-write
runners:
  write:
    default: true
    inputs:
      process_id: { type: string, required: true }
      input: { type: string, required: true }
    steps:
      - id: write
        tool: process.write
        scopes: [process.exec]
        inputs:
          process_id: $input.process_id
          input: $input.input
"#,
    )?;
    skill(
        root,
        "proc-resize",
        r#"skill: proc-resize
runners:
  resize:
    default: true
    inputs:
      process_id: { type: string, required: true }
    steps:
      - id: resize
        tool: process.resize
        scopes: [process.exec]
        inputs:
          process_id: $input.process_id
          rows: 40
          cols: 100
"#,
    )
}

/// Run `name` with `inputs`; its result's one packet.
fn run(root: &Path, name: &str, inputs: serde_json::Value) -> TestResult<serde_json::Value> {
    let mut child = runx(root)
        .args([
            "skill",
            &format!("skills/{name}"),
            "--inputs",
            "-",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("stdin")?
        .write_all(inputs.to_string().as_bytes())?;
    let output = child.wait_with_output()?;
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(envelope["status"], "sealed", "{envelope}");
    let result = envelope["result"].as_object().ok_or("a result")?;
    let (_, packet) = result.iter().next().ok_or("one packet")?;
    Ok(packet["data"].clone())
}

fn process(root: &Path, args: &[&str]) -> TestResult<serde_json::Value> {
    let output = runx(root)
        .arg("process")
        .args(args)
        .arg("--json")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Wait until what process `id` wrote holds `text`.
fn read_until(root: &Path, id: &str, text: &str) -> TestResult<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let read = process(root, &["read", id])?;
        let written = read["chunks"]
            .as_array()
            .ok_or("chunks")?
            .iter()
            .filter_map(|chunk| chunk["text"].as_str())
            .collect::<String>();
        if written.contains(text) {
            return Ok(written);
        }
        if Instant::now() > deadline {
            return Err(format!("{id} never wrote {text:?}: {written:?}").into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn exited(root: &Path, id: &str) -> TestResult<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let listed = process(root, &["list"])?;
        let found = listed["processes"]
            .as_array()
            .ok_or("processes")?
            .iter()
            .find(|process| process["process_id"] == id)
            .cloned()
            .ok_or("listed")?;
        if found["state"] == "exited" {
            return Ok(found);
        }
        if Instant::now() > deadline {
            return Err(format!("{id} never exited: {found}").into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_terminal_program_outlives_its_run_and_is_driven_by_later_ones() -> TestResult {
    let root = crate::support::temp_root("runx-process-tty");
    fs::create_dir_all(&root)?;
    skills(&root)?;
    let started = run(
        &root,
        "proc-tty",
        serde_json::json!({"args": ["-c", "stty size; while read line; do case $line in size) stty size;; *) echo got:$line;; esac; done"]}),
    )?;
    assert_eq!(started["tty"], serde_json::json!({"rows": 24, "cols": 80}));
    let id = started["process_id"]
        .as_str()
        .ok_or("a process id")?
        .to_owned();
    read_until(&root, &id, "24 80")?;

    let write = |input: &str| {
        run(
            &root,
            "proc-write",
            serde_json::json!({"process_id": id, "input": input}),
        )
    };
    write("hello\n")?;
    read_until(&root, &id, "got:hello")?;
    run(&root, "proc-resize", serde_json::json!({"process_id": id}))?;
    write("size\n")?;
    read_until(&root, &id, "40 100")?;

    // Ctrl-C reaches the program as the terminal's interrupt.
    write("\u{3}")?;
    let gone = exited(&root, &id)?;
    assert_eq!(gone["exit"]["signal"], 2, "{gone}");
    Ok(())
}

#[test]
fn the_person_reads_and_stops_a_process_a_run_started() -> TestResult {
    let root = crate::support::temp_root("runx-process-stop");
    fs::create_dir_all(&root)?;
    skills(&root)?;
    let started = run(
        &root,
        "proc-start",
        serde_json::json!({"args": ["-c", "echo ready; sleep 30"]}),
    )?;
    let id = started["process_id"]
        .as_str()
        .ok_or("a process id")?
        .to_owned();
    assert_eq!(started["state"], "running");
    read_until(&root, &id, "ready")?;

    let human = runx(&root).args(["process", "list"]).output()?;
    assert!(String::from_utf8(human.stdout)?.contains(&format!("{id}  running  sh -c")));
    let stopped = process(&root, &["stop", &id])?;
    assert_eq!(stopped["state"], "exited");
    assert_eq!(stopped["exit"]["reason"], "stopped");

    let unknown = runx(&root)
        .args(["process", "stop", "proc_00000000000000000000", "--json"])
        .output()?;
    assert!(!unknown.status.success());
    assert!(String::from_utf8(unknown.stdout)?.contains("unknown_process"));
    Ok(())
}

#[test]
fn a_runs_command_is_a_process_whose_output_is_read_by_cursor() -> TestResult {
    let root = crate::support::temp_root("runx-process-command");
    fs::create_dir_all(&root)?;
    skills(&root)?;
    let executed = run(
        &root,
        "cmd",
        serde_json::json!({"args": ["-c", "echo one; echo two >&2"]}),
    )?;
    assert_eq!(
        executed["stdout"], "one\n",
        "the call still carries its output"
    );

    let listed = process(&root, &["list"])?;
    let [command] = listed["processes"]
        .as_array()
        .ok_or("processes")?
        .as_slice()
    else {
        return Err(format!("one process: {listed}").into());
    };
    assert_eq!(command["kind"], "command");
    assert_eq!(command["exit"]["code"], 0);
    let id = command["process_id"].as_str().ok_or("an id")?;
    let read = process(&root, &["read", id])?;
    assert_eq!(read["state"], "exited");
    let streams = read["chunks"]
        .as_array()
        .ok_or("chunks")?
        .iter()
        .map(|chunk| (chunk["stream"].clone(), chunk["text"].clone()))
        .collect::<Vec<_>>();
    assert!(
        streams.contains(&("stdout".into(), "one\n".into())),
        "{streams:?}"
    );
    assert!(
        streams.contains(&("stderr".into(), "two\n".into())),
        "{streams:?}"
    );
    Ok(())
}
