//! A host cancels one call by its address while the others go on (script
//! step contract, "Completion and cleanup"; D36): through the run's one
//! cancel, which serves a turn's calls and a script's alike. Each call is
//! kept where the test needs it by a named pipe, which blocks its process
//! until the test opens the other end, so nothing here waits on a clock.

#![cfg(unix)]

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

/// A skill at `root/lead` whose agent step, `draft`, answers with a draft.
fn lead(root: &Path) -> TestResult {
    crate::support::write_test_skill_package(
        &root.join("lead"),
        "---\nname: lead\ndescription: Drafts for one lead.\n---\n# Lead\n\n## Agent task: draft\n\nDraft it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  draft:\n    steps:\n      - id: draft\n        agent: draft\n        outputs:\n          draft: {type: string}\n",
    )?;
    Ok(())
}

/// The address of the turn the run waits on whose id ends `name`.
fn turn_named(session: &TurnSession, name: &str) -> TestResult<String> {
    session
        .waiting()?
        .into_iter()
        .find_map(|request| match request {
            runx_contracts::ResolutionRequest::Turn { id, .. } if id.as_str().ends_with(name) => {
                Some(id.as_str().to_owned())
            }
            _ => None,
        })
        .ok_or_else(|| format!("no open turn {name}").into())
}

/// One lead's completion goes ahead with a sibling's call still running
/// (D36): the lead's turn is answered and its run seals while the sibling's
/// read is in flight, held by a named pipe the test opens only after the
/// completion has returned. Were the completion to wait on every call in
/// flight, it could never return.
#[test]
fn a_leads_completion_goes_ahead_with_a_siblings_call_still_running() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let tool = root.join("connectors/gate/tools/gate/wait");
    std::fs::create_dir_all(&tool)?;
    std::fs::write(root.join("connectors/gate/connector.yaml"), GATE_CONNECTOR)?;
    std::fs::write(tool.join("manifest.json"), GATE_TOOL)?;
    std::fs::write(tool.join("wait.sh"), WAIT_SCRIPT)?;
    let (on, off) = (pipe(&root.join("started"))?, pipe(&root.join("release"))?);
    lead(root)?;
    let skill = asker(
        root,
        "          skills: [../lead]\n          operations: [gate/gate.wait]\n",
    )?;
    let (run_id, session) = open(root, skill)?;
    let drafting = session.call(TURN, "skill__lead", None, JsonObject::new())?;
    assert!(
        matches!(drafting.outcome, TurnCallOutcome::Pending { .. }),
        "{drafting:?}"
    );
    let draft = turn_named(&session, "draft")?;
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

    session.complete(&draft, input(serde_json::json!({"draft": "hello"}))?)?;
    // Read from the record, which waits on nothing: the lead's call settled
    // while its sibling still ran.
    assert_eq!(
        calls(root, &run_id, &run_id)?,
        [
            (0, "child", false),
            (1, "claimed", false),
            (0, "completed", false)
        ]
    );

    {
        use std::os::unix::fs::OpenOptionsExt;
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
            .open(&off);
        match opened {
            Ok(mut pipe) => {
                // Release through the handle that proved the read is still
                // waiting: closing it first could give that read EOF.
                writeln!(pipe)?;
            }
            Err(_) => {
                let read = reading.join().map_err(|_| "the read panicked")?;
                return Err(
                    format!("the sibling's read ended before its release: {read:?}").into(),
                );
            }
        }
    }
    let read = reading.join().map_err(|_| "the read panicked")??;
    assert!(
        matches!(read.outcome, TurnCallOutcome::Completed { .. }),
        "{read:?}"
    );
    turn_complete(&session)
}

/// A completion doesn't hold the driver while its run goes on (D36): the
/// lead's continuation, a read held by a named pipe after its turn, runs
/// apart from the driver, so the root turn makes and records another call
/// meanwhile. Were the continuation to hold the driver, that call could
/// never be admitted, and the test would never release the pipe.
#[test]
fn a_completions_continuation_runs_apart_from_the_driver() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let tool = root.join("connectors/gate/tools/gate/wait");
    std::fs::create_dir_all(&tool)?;
    std::fs::write(root.join("connectors/gate/connector.yaml"), GATE_CONNECTOR)?;
    std::fs::write(tool.join("manifest.json"), GATE_TOOL)?;
    std::fs::write(tool.join("wait.sh"), WAIT_SCRIPT)?;
    let (on, off) = (pipe(&root.join("started"))?, pipe(&root.join("release"))?);
    crate::support::write_test_skill_package(
        &root.join("lead"),
        "---\nname: lead\ndescription: Drafts for one lead, then reads.\n---\n# Lead\n\n## Agent task: draft\n\nDraft it.\n",
        &format!(
            r#"minimum_runx: 0.12.0
methods:
  draft:
    steps:
      - id: draft
        agent: draft
        outputs:
          draft: {{type: string}}
      - id: read
        operation: gate/gate.wait
        inputs: {{started: "{on}", release: "{off}"}}
"#,
            on = on.display(),
            off = off.display(),
        ),
    )?;
    let skill = asker(
        root,
        "          skills: [../lead]\n          tools: [data.digest]\n",
    )?;
    let (run_id, session) = open(root, skill)?;
    session.call(TURN, "skill__lead", None, JsonObject::new())?;
    let draft = turn_named(&session, "draft")?;
    let completing = std::thread::spawn({
        let session = Arc::clone(&session);
        let output = input(serde_json::json!({"draft": "hello"}))?;
        move || session.complete(&draft, output)
    });
    started(&on)?;

    let digest = session.call(
        TURN,
        "data_digest",
        None,
        input(serde_json::json!({"value": "meanwhile", "encoding": "utf8_text"}))?,
    )?;
    assert!(
        matches!(digest.outcome, TurnCallOutcome::Completed { .. }),
        "{digest:?}"
    );

    release(&off)?;
    completing.join().map_err(|_| "the completion panicked")??;
    let recorded = calls(root, &run_id, &run_id)?;
    assert_eq!(
        recorded.last(),
        Some(&(0, "completed", false)),
        "the lead's call is concluded once its run went on: {recorded:?}"
    );
    turn_complete(&session)
}

/// Stopping a run stops its script's calls in flight and never reaches the
/// script's catch: the script waits on a read whose process the stop ends,
/// the drive returns stopped, and the run stays open where it stood.
#[test]
fn stopping_a_run_stops_its_scripts_calls_in_flight_and_never_reaches_its_catch() -> TestResult {
    use std::os::unix::fs::OpenOptionsExt;
    let temp = tempdir()?;
    let root = temp.path();
    let tool = root.join("connectors/gate/tools/gate/wait");
    std::fs::create_dir_all(&tool)?;
    std::fs::write(root.join("connectors/gate/connector.yaml"), GATE_CONNECTOR)?;
    std::fs::write(tool.join("manifest.json"), GATE_TOOL)?;
    std::fs::write(tool.join("wait.sh"), WAIT_SCRIPT)?;
    let (on, off) = (pipe(&root.join("started"))?, pipe(&root.join("release"))?);
    let skill = root.join("stopper");
    crate::support::write_test_skill_package(
        &skill,
        "---\nname: stopper\ndescription: Waits on a read.\n---\n# Stopper\n\nRead.\n",
        "minimum_runx: 0.12.0\nmethods:\n  prepare:\n    steps:\n      - id: prepare\n        script: {module: prepare.mjs}\n        calls:\n          operations: [gate/gate.wait]\n        max_calls: 1\n        outputs:\n          done: {type: string}\n",
    )?;
    std::fs::write(
        skill.join("prepare.mjs"),
        format!(
            "export default async function (ctx) {{\n  try {{\n    await ctx.operation(\"gate/gate.wait\", {{ started: {on:?}, release: {off:?} }});\n    return {{ done: \"read\" }};\n  }} catch (rejected) {{\n    return {{ done: `caught ${{rejected.reason}}` }};\n  }}\n}}\n",
            on = on.display().to_string(),
            off = off.display().to_string(),
        ),
    )?;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = std::thread::scope(|scope| {
        scope.spawn(|| {
            if started(&on).is_ok() {
                stop.store(true, std::sync::atomic::Ordering::Release);
            }
        });
        runx_runtime::interrupt::while_working(&stop, || {
            orchestrator(root)?
                .run_skill_with_binding(
                    &SkillRunRequest {
                        current_context: Vec::new(),
                        historical_context: Vec::new(),
                        run_authority: None,
                        act: None,
                        skill_path: skill.clone(),
                        receipt_dir: Some(root.join("receipts")),
                        run_id: Some("stopped_run".to_owned()),
                        answers: None,
                        inputs: BTreeMap::new(),
                        env: env(root),
                        cwd: root.to_path_buf(),
                        managed_agent: ManagedAgentPolicy::HostDriven,
                    },
                    None,
                    None,
                    None,
                )
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
        })
    });
    let error = match stopped {
        Err(error) => error,
        Ok(result) => {
            return Err(format!("a stopped run returns stopped, not {:?}", result.output).into());
        }
    };
    assert!(error.to_string().contains("stopped"), "{error}");
    // The read's process ended: nothing reads its release pipe any more.
    let released = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(&off);
    assert!(released.is_err(), "the read is still in flight");
    // The call stands as it was claimed, and the run stays open.
    let read = runx_runtime::run_record::read_run(&env(root), root, "stopped_run", None)?;
    assert!(
        !read
            .events
            .iter()
            .any(|record| matches!(record.event, RunEventFact::Sealed { .. })),
        "the stopped run sealed"
    );
    Ok(())
}

/// Whether `stream` is run `run_id`'s own or one of its steps' calls
/// streams.
fn of_run(stream: &str, run_id: &str) -> bool {
    stream == run_id || runx_runtime::run_record::is_calls_stream_of(stream, run_id)
}
