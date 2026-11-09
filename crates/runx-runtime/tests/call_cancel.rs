//! A host cancels one call by its address while the others go on (script
//! step contract, "Completion and cleanup"; D36): through the run's one
//! cancel, which serves a turn's calls and a script's alike. Each call is
//! held where the test needs it by a named pipe, which blocks its process
//! until the test opens the other end, so nothing here waits on a clock.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{JsonObject, JsonValue, RunCallOutcome, RunEventFact, TurnCallOutcome};
use runx_runtime::{
    CallCancel, LocalOrchestrator, ManagedAgentPolicy, SkillRunRequest, TurnRequest, TurnSession,
};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The turn every test works.
const TURN: &str = "answer";

/// A connector whose `gate.wait` reads: its process says it started, then
/// waits to be released.
const GATE_CONNECTOR: &str = r#"schema: runx.connector.v1
name: gate
title: Gate
description: Reads once it is released.
auth:
  none: {}
placements:
  local:
    auth: [none]
operations:
  gate.wait:
    description: Wait to be released, then read.
    verb: read
    scopes: [gate.read]
    target:
      template: {"$argument": "/started"}
    input_schema:
      type: object
      required: [started, release]
      additionalProperties: false
      properties:
        started: {type: string, minLength: 1}
        release: {type: string, minLength: 1}
    result_fields: [read]
    implementation:
      process:
        tool: gate.wait
"#;

const GATE_TOOL: &str = r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "gate.wait",
  "version": "0.1.0",
  "description": "Say it started, wait to be released, then read.",
  "process": {
    "command": "sh",
    "args": ["./wait.sh", "{{started}}", "{{release}}"]
  },
  "inputs": {
    "started": {"type": "string", "description": "The pipe it says it started on."},
    "release": {"type": "string", "description": "The pipe it waits on."}
  }
}
"#;

/// Opening a pipe blocks until its other end opens: the write says it
/// started once the test reads, and the read waits until the test writes.
const WAIT_SCRIPT: &str =
    "#!/bin/sh\nset -eu\nprintf x > \"$1\"\nread line < \"$2\"\nprintf '{\"read\":\"done\"}\\n'\n";

fn env(root: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([
        ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
        // No load reaches a network.
        (
            "RUNX_REGISTRY_URL".to_owned(),
            "http://127.0.0.1:9".to_owned(),
        ),
    ]);
    crate::support::insert_test_signing_env(&mut env);
    env
}

fn orchestrator(root: &Path) -> TestResult<LocalOrchestrator> {
    Ok(LocalOrchestrator::with_effects_and_environment(
        runx_runtime::RuntimeEffectRegistry::local()?,
        env(root),
    ))
}

/// A named pipe at `path`.
fn pipe(path: &Path) -> TestResult<PathBuf> {
    let made = std::process::Command::new("mkfifo").arg(path).status()?;
    if !made.success() {
        return Err(format!("mkfifo {} failed", path.display()).into());
    }
    Ok(path.to_path_buf())
}

/// Wait until the call holding `started` says so.
fn started(started: &Path) -> TestResult {
    let mut said = String::new();
    std::fs::File::open(started)?.read_to_string(&mut said)?;
    assert_eq!(said, "x");
    Ok(())
}

/// Release the call waiting on `release`.
fn release(release: &Path) -> TestResult {
    let mut pipe = std::fs::OpenOptions::new().write(true).open(release)?;
    writeln!(pipe)?;
    Ok(())
}

/// A skill at `root/asker` whose agent step, `answer`, may make `calls`.
fn asker(root: &Path, calls: &str) -> TestResult<PathBuf> {
    let skill = root.join("asker");
    crate::support::write_test_skill_package(
        &skill,
        "---\nname: asker\ndescription: Answers with its calls.\n---\n# Asker\n\n## Agent task: answer\n\nMake the calls, then answer.\n",
        &format!(
            "minimum_runx: 0.12.0\nmethods:\n  answer:\n    steps:\n      - id: answer\n        agent: answer\n        calls:\n{calls}        outputs:\n          answer: {{type: string}}\n"
        ),
    )?;
    Ok(skill)
}

/// Start `skill` to its turn and open it for its host.
fn open(root: &Path, skill: PathBuf) -> TestResult<(String, Arc<TurnSession>)> {
    let started = orchestrator(root)?.run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        None,
        None,
        None,
    )?;
    let run_id = started
        .output
        .as_object()
        .and_then(|output| output.get("run_id"))
        .and_then(JsonValue::as_str)
        .ok_or("the run names its id")?
        .to_owned();
    let session = orchestrator(root)?.open_turns(&TurnRequest {
        run_id: run_id.clone(),
        receipt_dir: Some(root.join("receipts")),
        env: env(root),
        cwd: root.to_path_buf(),
    })?;
    Ok((run_id, Arc::new(session)))
}

fn input(value: serde_json::Value) -> TestResult<JsonObject> {
    Ok(serde_json::from_value(value)?)
}

/// Each call event of stream `stream` of run `run_id`, as `(index, outcome,
/// cancelled)`.
fn calls(root: &Path, run_id: &str, stream: &str) -> TestResult<Vec<(u32, &'static str, bool)>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .filter(|record| record.stream.as_str() == stream)
        .filter_map(|record| match record.event {
            RunEventFact::Call {
                index,
                outcome,
                cancelled,
                ..
            } => Some((
                index,
                match *outcome {
                    RunCallOutcome::Completed { .. } => "completed",
                    RunCallOutcome::Refused { .. } => "refused",
                    RunCallOutcome::Failed { .. } => "failed",
                    RunCallOutcome::Pending { .. } => "pending",
                    RunCallOutcome::Claimed { .. } => "claimed",
                    RunCallOutcome::Child { .. } => "child",
                    RunCallOutcome::Cancelled => "cancelled",
                },
                cancelled,
            )),
            _ => None,
        })
        .collect())
}

/// A turn's read in flight settles cancelled, and its stop ends it: the
/// process it runs is never released, so the call returns only because the
/// cancel stopped it.
#[test]
fn a_turns_read_in_flight_settles_cancelled_and_its_stop_ends_it() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let tool = root.join("connectors/gate/tools/gate/wait");
    std::fs::create_dir_all(&tool)?;
    std::fs::write(root.join("connectors/gate/connector.yaml"), GATE_CONNECTOR)?;
    std::fs::write(tool.join("manifest.json"), GATE_TOOL)?;
    std::fs::write(tool.join("wait.sh"), WAIT_SCRIPT)?;
    let (on, off) = (pipe(&root.join("started"))?, pipe(&root.join("release"))?);
    let skill = asker(root, "          operations: [gate/gate.wait]\n")?;
    let (run_id, session) = open(root, skill)?;
    let wait = session
        .status(TURN)?
        .tools
        .into_iter()
        .find(|tool| tool.name.as_str().contains("gate"))
        .ok_or("the turn offers the gate's read")?
        .name;
    let reading = std::thread::spawn({
        let session = Arc::clone(&session);
        let call = input(serde_json::json!({"input": {
            "started": on.to_string_lossy(),
            "release": off.to_string_lossy(),
        }}))?;
        move || session.call(TURN, wait.as_str(), None, call)
    });
    started(&on)?;
    assert_eq!(session.cancel("answer/0")?, CallCancel::Cancelled);
    let read = reading.join().map_err(|_| "the call panicked")??;
    assert_eq!(read.outcome, TurnCallOutcome::Cancelled, "{read:?}");
    assert_eq!(
        calls(root, &run_id, &run_id)?,
        [(0, "claimed", false), (0, "cancelled", true)]
    );
    let again = session
        .cancel("answer/0")
        .err()
        .ok_or("a settled call isn't executing")?;
    assert!(
        again.to_string().contains("executing no call answer/0"),
        "{again}"
    );
    Ok(())
}

/// A script's call that hasn't started settles cancelled, and the script
/// catches it; an effect in flight runs to its end and settles as it
/// actually ended, with the cancel beside it. Both are cancelled through the
/// turn's session, at the script's depth.
#[test]
fn a_scripts_call_not_started_settles_cancelled_and_an_effect_in_flight_as_it_ends() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let (on, off) = (pipe(&root.join("started"))?, pipe(&root.join("release"))?);
    crate::support::write_test_skill_package(
        &root.join("hold"),
        "---\nname: hold\ndescription: Says it started, then waits to be released.\n---\n# Hold\n\nHold.\n",
        &format!(
            r#"minimum_runx: 0.12.0
methods:
  hold:
    steps:
      - id: hold
        tool:
          command: sh
          args:
            - -c
            - 'printf x > "{on}" && read line < "{off}" && printf "{{\"held\":true}}\n"'
        outputs:
          held: {{type: boolean}}
"#,
            on = on.display(),
            off = off.display(),
        ),
    )?;
    let batch = root.join("batch");
    crate::support::write_test_skill_package(
        &batch,
        "---\nname: batch\ndescription: Holds, then digests.\n---\n# Batch\n\nHold, then digest.\n",
        r#"minimum_runx: 0.12.0
methods:
  prepare:
    steps:
      - id: prepare
        script: {module: prepare.mjs}
        calls:
          skills: [../hold]
          tools: [data.digest]
        max_calls: 2
        outputs:
          held: {type: string}
          digest: {type: string}
"#,
    )?;
    std::fs::write(
        batch.join("prepare.mjs"),
        r#"export default async function (ctx) {
  const [held, digest] = await Promise.allSettled([
    ctx.skill("../hold", {}),
    ctx.tool("data.digest", { value: "hello", encoding: "utf8_text" }),
  ]);
  return { held: held.status, digest: digest.status === "rejected" ? digest.reason.reason : "resolved" };
}
"#,
    )?;
    let skill = asker(root, "          skills: [../batch]\n")?;
    let (run_id, session) = open(root, skill)?;
    let calling = std::thread::spawn({
        let session = Arc::clone(&session);
        move || session.call(TURN, "skill__batch", None, JsonObject::new())
    });
    // Its concurrency is one: call 0 holds, and call 1 waits its turn.
    started(&on)?;
    assert_eq!(session.cancel("answer/0#prepare/1")?, CallCancel::Cancelled);
    assert_eq!(
        session.cancel("answer/0#prepare/0")?,
        CallCancel::SettlesAsItEnds
    );
    release(&off)?;
    let called = calling.join().map_err(|_| "the call panicked")??;
    let TurnCallOutcome::Completed { output, .. } = &called.outcome else {
        return Err(format!("the batch completed: {called:?}").into());
    };
    assert_eq!(
        serde_json::to_value(output)?,
        serde_json::json!({"held": "fulfilled", "digest": "cancelled"})
    );
    let script = calls(root, &run_id, &format!("{run_id}/answer/0"))?;
    assert_eq!(
        script,
        [
            (0, "child", false),
            (1, "claimed", false),
            (0, "completed", true),
            (1, "cancelled", true),
        ]
    );
    assert!(session.cancel("answer/0#prepare/0").is_err());
    Ok(())
}
