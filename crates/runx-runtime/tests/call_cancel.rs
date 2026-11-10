//! A host cancels one call by its address while the others go on (script
//! step contract, "Completion and cleanup"; D36): through the run's one
//! cancel, which serves a turn's calls and a script's alike. Each call is
//! kept where the test needs it by a named pipe, which blocks its process
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

/// Wait until the call waiting on `started` says so.
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
    open_under(root, skill, None)
}

/// Start `skill` under the host's `authority` to its turn, and open it.
fn open_under(
    root: &Path,
    skill: PathBuf,
    authority: Option<serde_json::Value>,
) -> TestResult<(String, Arc<TurnSession>)> {
    let started = orchestrator(root)?.run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: authority.map(serde_json::from_value).transpose()?,
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

/// End the turn, so its run seals and writes its receipts.
fn turn_complete(session: &TurnSession) -> TestResult {
    session.complete(TURN, input(serde_json::json!({"answer": "done"}))?)?;
    Ok(())
}

fn input(value: serde_json::Value) -> TestResult<JsonObject> {
    Ok(serde_json::from_value(value)?)
}

/// Each call event of run `stream`'s steps, in run `run_id`'s tree, as
/// `(index, outcome, cancelled)`.
fn calls(root: &Path, run_id: &str, stream: &str) -> TestResult<Vec<(u32, &'static str, bool)>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .filter(|record| of_run(record.stream.as_str(), stream))
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
                    RunCallOutcome::Declined { .. } => "declined",
                    RunCallOutcome::Failed { .. } => "failed",
                    RunCallOutcome::Pending { .. } => "pending",
                    RunCallOutcome::Claimed { .. } => "claimed",
                    RunCallOutcome::Executing => "executing",
                    RunCallOutcome::Child { .. } => "child",
                    RunCallOutcome::Cancelled => "cancelled",
                    RunCallOutcome::Waited { .. } => "waited",
                    RunCallOutcome::Read { .. } => "read",
                    RunCallOutcome::TooLarge { .. } => "too_large",
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
/// catches it. A skill call in flight is reached in its child run: its effect
/// runs to its end, and the child, which committed it, seals killed, so the
/// call settles as the child actually ended, with the cancel beside it. Both
/// are cancelled through the turn's session, at the script's depth.
#[test]
fn a_scripts_call_not_started_settles_cancelled_and_an_effect_in_flight_as_it_ends() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let (on, off) = (pipe(&root.join("started"))?, pipe(&root.join("release"))?);
    crate::support::write_test_skill_package(
        &root.join("pause"),
        "---\nname: pause\ndescription: Says it started, then waits to be released.\n---\n# Pause\n\nPause.\n",
        &format!(
            r#"minimum_runx: 0.12.0
methods:
  pause:
    steps:
      - id: pause
        tool:
          command: sh
          args:
            - -c
            - 'printf x > "{on}" && read line < "{off}" && printf "{{\"paused\":true}}\n"'
        outputs:
          paused: {{type: boolean}}
"#,
            on = on.display(),
            off = off.display(),
        ),
    )?;
    let batch = root.join("batch");
    crate::support::write_test_skill_package(
        &batch,
        "---\nname: batch\ndescription: Pauses, then digests.\n---\n# Batch\n\nPause, then digest.\n",
        r#"minimum_runx: 0.12.0
methods:
  prepare:
    steps:
      - id: prepare
        script: {module: prepare.mjs}
        calls:
          skills: [../pause]
          tools: [data.digest]
        max_calls: 2
        outputs:
          paused: {type: string}
          digest: {type: string}
"#,
    )?;
    std::fs::write(
        batch.join("prepare.mjs"),
        r#"export default async function (ctx) {
  const [paused, digest] = await Promise.allSettled([
    ctx.skill("../pause", {}),
    ctx.tool("data.digest", { value: "hello", encoding: "utf8_text" }),
  ]);
  return { paused: paused.status, digest: digest.status === "rejected" ? digest.reason.reason : "resolved" };
}
"#,
    )?;
    let skill = asker(root, "          skills: [../batch]\n")?;
    let (run_id, session) = open(root, skill)?;
    let calling = std::thread::spawn({
        let session = Arc::clone(&session);
        move || session.call(TURN, "skill__batch", None, JsonObject::new())
    });
    // Its concurrency is one: call 0 pauses, and call 1 waits its turn.
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
        serde_json::json!({"paused": "rejected", "digest": "cancelled"})
    );
    let script = calls(root, &run_id, &format!("{run_id}/answer/0"))?;
    assert_eq!(
        script,
        [
            (0, "child", false),
            (1, "executing", false),
            (0, "completed", true),
            (1, "cancelled", true),
        ]
    );
    assert!(session.cancel("answer/0#prepare/0").is_err());
    Ok(())
}

/// A connector whose `slot.take` reads: its process takes the first free
/// slot under the directory it's given, says it started on that slot's pipe,
/// and waits to be released on the slot's other.
const SLOT_CONNECTOR: &str = r#"schema: runx.connector.v1
name: slot
title: Slot
description: Reads once it is released.
auth:
  none: {}
placements:
  local:
    auth: [none]
operations:
  slot.take:
    description: Take a slot, wait to be released, then read.
    verb: read
    scopes: [slot.read]
    target:
      template: {"$argument": "/dir"}
    input_schema:
      type: object
      required: [dir]
      additionalProperties: false
      properties:
        dir: {type: string, minLength: 1}
    result_fields: [taken]
    implementation:
      process:
        tool: slot.take
"#;

const SLOT_TOOL: &str = r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "slot.take",
  "version": "0.1.0",
  "description": "Take a slot, say it started, wait to be released, then read.",
  "process": {
    "command": "sh",
    "args": ["./wait.sh", "{{dir}}"]
  },
  "inputs": {
    "dir": {"type": "string", "description": "Where its slots and pipes are."}
  }
}
"#;

const SLOT_SCRIPT: &str = "#!/bin/sh\nset -eu\nfor n in 0 1 2 3; do mkdir \"$1/slot-$n\" 2>/dev/null && break; done\nprintf x > \"$1/s$n\"\nread line < \"$1/r$n\"\nprintf '{\"taken\":true}\\n'\n";

/// A run's `max_concurrency` caps a script's `concurrency`: under a run
/// authority of 2, a script that declares 4 has two calls executing while
/// the other two haven't started, which the run's cancel counts. Each call is
/// a child run that reads, so nothing but the cap keeps them apart, and the
/// calls the scheduler dispatches at once are in flight before any runs, so a
/// cap that failed would find calls 2 and 3 started.
#[test]
fn a_runs_max_concurrency_caps_how_many_of_a_scripts_calls_execute_at_once() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let tool = root.join("connectors/slot/tools/slot/take");
    std::fs::create_dir_all(&tool)?;
    std::fs::write(root.join("connectors/slot/connector.yaml"), SLOT_CONNECTOR)?;
    std::fs::write(tool.join("manifest.json"), SLOT_TOOL)?;
    std::fs::write(tool.join("wait.sh"), SLOT_SCRIPT)?;
    let slots = root.join("slots");
    std::fs::create_dir_all(&slots)?;
    for slot in 0..4 {
        pipe(&slots.join(format!("s{slot}")))?;
        pipe(&slots.join(format!("r{slot}")))?;
    }
    crate::support::write_test_skill_package(
        &root.join("take"),
        "---\nname: take\ndescription: Takes a slot until it is released.\n---\n# Take\n\nTake a slot.\n",
        &format!(
            "minimum_runx: 0.12.0\nmethods:\n  take:\n    steps:\n      - id: take\n        operation: slot/slot.take\n        inputs: {{dir: \"{}\"}}\n",
            slots.display()
        ),
    )?;
    let batch = root.join("batch");
    crate::support::write_test_skill_package(
        &batch,
        "---\nname: batch\ndescription: Takes four slots at once.\n---\n# Batch\n\nTake four.\n",
        r#"minimum_runx: 0.12.0
methods:
  prepare:
    steps:
      - id: prepare
        script: {module: prepare.mjs}
        calls:
          skills: [../take]
        max_calls: 4
        concurrency: 4
        outputs:
          outcomes: {type: array}
"#,
    )?;
    std::fs::write(
        batch.join("prepare.mjs"),
        r#"export default async function (ctx) {
  const settled = await Promise.allSettled([0, 1, 2, 3].map(() => ctx.skill("../take", {})));
  return { outcomes: settled.map((call) => (call.status === "fulfilled" ? "taken" : call.reason.reason)) };
}
"#,
    )?;
    let skill = asker(root, "          skills: [../batch]\n")?;
    let (_, session) = open_under(root, skill, Some(serde_json::json!({"max_concurrency": 2})))?;
    let calling = std::thread::spawn({
        let session = Arc::clone(&session);
        move || session.call(TURN, "skill__batch", None, JsonObject::new())
    });
    let slot = |name: String| slots.join(name);
    started(&slot("s0".to_owned()))?;
    started(&slot("s1".to_owned()))?;
    // Two are executing; count the rest.
    let beyond = [
        session.cancel("answer/0#prepare/2")?,
        session.cancel("answer/0#prepare/3")?,
    ];
    // Release whatever started, so the run ends either way.
    release(&slot("r0".to_owned()))?;
    release(&slot("r1".to_owned()))?;
    for (n, cancel) in [2, 3].into_iter().zip(beyond) {
        if cancel == CallCancel::SettlesAsItEnds {
            started(&slot(format!("s{n}")))?;
            release(&slot(format!("r{n}")))?;
        }
    }
    let called = calling.join().map_err(|_| "the call panicked")??;
    assert_eq!(
        beyond,
        [CallCancel::Cancelled, CallCancel::Cancelled],
        "no more than two of the script's calls had started"
    );
    let TurnCallOutcome::Completed { output, .. } = &called.outcome else {
        return Err(format!("the batch completed: {called:?}").into());
    };
    assert_eq!(
        serde_json::to_value(output)?,
        serde_json::json!({"outcomes": ["taken", "taken", "cancelled", "cancelled"]})
    );
    Ok(())
}

/// A cancel reaches a skill call's child run by the same rule: paused inside
/// the first of its two effects, the first completes, the second never
/// starts, and the child seals killed because its caller cancelled it. The
/// child committed an effect, so the parent's call settles as the child
/// actually ended, with the cancel beside it.
#[test]
fn a_cancel_reaches_a_skill_calls_child_run_which_seals_killed() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let (on, off) = (pipe(&root.join("started"))?, pipe(&root.join("release"))?);
    let second = root.join("second-ran");
    crate::support::write_test_skill_package(
        &root.join("two"),
        "---\nname: two\ndescription: Two effects, one after the other.\n---\n# Two\n\nDo both.\n",
        &format!(
            r#"minimum_runx: 0.12.0
methods:
  two:
    steps:
      - id: first
        tool:
          command: sh
          args:
            - -c
            - 'printf x > "{on}" && read line < "{off}" && printf "{{\"first\":true}}\n"'
        outputs:
          first: {{type: boolean}}
      - id: second
        tool:
          command: sh
          args:
            - -c
            - ': > "{second}" && printf "{{\"second\":true}}\n"'
        outputs:
          second: {{type: boolean}}
"#,
            on = on.display(),
            off = off.display(),
            second = second.display(),
        ),
    )?;
    let skill = asker(root, "          skills: [../two]\n")?;
    let (run_id, session) = open(root, skill)?;
    let calling = std::thread::spawn({
        let session = Arc::clone(&session);
        move || session.call(TURN, "skill__two", None, JsonObject::new())
    });
    // The child is inside its first effect.
    started(&on)?;
    assert_eq!(session.cancel("answer/0")?, CallCancel::SettlesAsItEnds);
    release(&off)?;
    let called = calling.join().map_err(|_| "the call panicked")??;
    let TurnCallOutcome::Failed {
        receipt_id: Some(receipt_id),
        ..
    } = &called.outcome
    else {
        return Err(format!("the call ended as its child did: {called:?}").into());
    };
    assert!(!second.exists(), "the second effect never started");
    turn_complete(&session)?;
    let child =
        crate::support::read_test_signed_receipt(&root.join("receipts"), receipt_id.as_str())?;
    assert_eq!(child.seal.outcome, runx_contracts::RunOutcome::Killed);
    assert_eq!(
        child.seal.reason,
        Some(runx_contracts::SealReason::Cancelled)
    );
    assert_eq!(
        calls(root, &run_id, &run_id)?,
        [(0, "child", false), (0, "completed", true)],
        "the call settled as its child ended, with the cancel beside it"
    );
    Ok(())
}

/// A skill call whose child run committed no effect settles cancelled: paused
/// inside a read, the cancel stops the read, and the child seals killed.
#[test]
fn a_cancelled_skill_call_whose_child_only_read_settles_cancelled() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let tool = root.join("connectors/gate/tools/gate/wait");
    std::fs::create_dir_all(&tool)?;
    std::fs::write(root.join("connectors/gate/connector.yaml"), GATE_CONNECTOR)?;
    std::fs::write(tool.join("manifest.json"), GATE_TOOL)?;
    std::fs::write(tool.join("wait.sh"), WAIT_SCRIPT)?;
    let (on, off) = (pipe(&root.join("started"))?, pipe(&root.join("release"))?);
    crate::support::write_test_skill_package(
        &root.join("peek"),
        "---\nname: peek\ndescription: Reads once it is released.\n---\n# Peek\n\nRead.\n",
        &format!(
            "minimum_runx: 0.12.0\nmethods:\n  peek:\n    steps:\n      - id: peek\n        operation: gate/gate.wait\n        inputs: {{started: \"{}\", release: \"{}\"}}\n",
            on.display(),
            off.display()
        ),
    )?;
    let skill = asker(root, "          skills: [../peek]\n")?;
    let (run_id, session) = open(root, skill)?;
    let calling = std::thread::spawn({
        let session = Arc::clone(&session);
        move || session.call(TURN, "skill__peek", None, JsonObject::new())
    });
    started(&on)?;
    assert_eq!(session.cancel("answer/0")?, CallCancel::SettlesAsItEnds);
    // Nothing releases the read: the cancel stops it.
    let called = calling.join().map_err(|_| "the call panicked")??;
    assert_eq!(called.outcome, TurnCallOutcome::Cancelled, "{called:?}");
    assert_eq!(
        calls(root, &run_id, &run_id)?,
        [(0, "child", false), (0, "cancelled", true)]
    );
    Ok(())
}

/// Whether `stream` is run `run_id`'s own or one of its steps' calls
/// streams.
fn of_run(stream: &str, run_id: &str) -> bool {
    stream == run_id || runx_runtime::run_record::is_calls_stream_of(stream, run_id)
}
