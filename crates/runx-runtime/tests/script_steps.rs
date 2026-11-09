//! Script steps end to end: a script calls one tool and one skill, its run
//! waits on the skill's approval, and the next drive replays the record and
//! seals, without running the recorded call again (plan §9's control-flow
//! row, its first vertical slice).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{JsonObject, JsonValue, RunCallOutcome, RunEventFact};
use runx_runtime::{
    LocalOrchestrator, ManagedAgentPolicy, ResumeRequest, RunResult, SkillRunRequest,
};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A script that digests its value with a catalog tool, then has a child
/// skill confirm it; the child asks a person first.
fn write_script_skill(root: &Path, script: &str, outputs: &str) -> TestResult<PathBuf> {
    crate::support::write_test_skill_package(
        &root.join("confirm-child"),
        "---\nname: confirm-child\ndescription: Confirms a value once a person approves.\n---\n# Confirm child\n\nConfirm the value.\n",
        r#"minimum_runx: 0.12.0
methods:
  confirm:
    inputs:
      value: {type: string}
    steps:
      - id: approve
        ask: {gate: confirm, reason: Confirm the digest.}
      - id: digest
        tool: data.digest
        inputs:
          value: $input.value
          encoding: utf8_text
"#,
    )?;
    let skill_dir = root.join("script-demo");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: script-demo\ndescription: Digests a value and has it confirmed.\n---\n# Script demo\n\nDigest, then confirm.\n",
        &format!(
            r#"minimum_runx: 0.12.0
methods:
  prepare:
    inputs:
      value: {{type: string}}
    steps:
      - id: prepare
        script: {{module: prepare.mjs}}
        inputs:
          value: $input.value
        calls:
          tools: [data.digest]
          skills: [../confirm-child]
        max_calls: 4
        outputs:
{outputs}"#
        ),
    )?;
    std::fs::write(skill_dir.join("prepare.mjs"), script)?;
    Ok(skill_dir)
}

const DIGEST_THEN_CONFIRM: &str = r#"export default async function (ctx, { value }) {
  const digested = await ctx.tool("data.digest", { value, encoding: "utf8_text" });
  const confirmed = await ctx.skill("../confirm-child", { value });
  return { digested: Object.keys(digested), confirmed: Object.keys(confirmed) };
}
"#;

fn run(root: &Path, skill_dir: PathBuf) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::from([("value".to_owned(), JsonValue::String("hello".to_owned()))]),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        None,
        None,
        None,
    )?)
}

fn resume(root: &Path, run_id: &str, answers: serde_json::Value) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().resume(&ResumeRequest {
        run_id: run_id.to_owned(),
        answers: crate::support::bind_resume(&env(root), root, run_id, answers)?,
        receipt_dir: Some(root.join("receipts")),
        env: env(root),
        cwd: root.to_path_buf(),
        managed_agent: ManagedAgentPolicy::HostDriven,
        package_digest: None,
        execution_closure_digest: None,
    })?)
}

/// Each call event the run records at its root, as `(index, outcome)`.
fn call_events(root: &Path, run_id: &str) -> TestResult<Vec<(u32, &'static str)>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .filter(|record| record.stream.as_str() == run_id)
        .filter_map(|record| match record.event {
            RunEventFact::Call { index, outcome, .. } => Some((
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
            )),
            _ => None,
        })
        .collect())
}

#[test]
fn a_script_calls_a_tool_and_a_skill_and_replays_its_record_to_seal() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        DIGEST_THEN_CONFIRM,
        "          digested: {type: array}\n          confirmed: {type: array}\n",
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    // The child waits on its approval, so the run does too, with the tool's
    // call claimed and settled once, and the skill's linked to its child run.
    let waiting = requests(&started)?;
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    // The child run of call 1 asks at its gate.
    assert_eq!(waiting, ["prepare/1#confirm"]);
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [(0, "claimed"), (0, "completed"), (1, "child")]
    );

    let sealed = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {waiting[0].as_str(): true}}),
    )?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    // The next drive replayed the tool's recorded result and ran nothing
    // again: the only new event settles the skill's call.
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [
            (0, "claimed"),
            (0, "completed"),
            (1, "child"),
            (1, "completed")
        ]
    );
    // The step's receipt names both calls.
    let receipt_id = text(output, "receipt_id").ok_or("missing receipt_id")?;
    let receipts = temp.path().join("receipts");
    let workflow = crate::support::read_test_signed_receipt(&receipts, receipt_id)?;
    let step = crate::support::read_test_first_step_receipt(&receipts, &workflow)?;
    assert_eq!(step.lineage.children.len(), 2);
    Ok(())
}

#[test]
fn a_call_outside_the_steps_calls_is_refused_and_the_script_can_catch_it() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        r#"export default async function (ctx) {
  try {
    await ctx.tool("fs.read", { path: "secret.txt" });
    return { refused: null };
  } catch (rejected) {
    return { refused: rejected.reason, why: rejected.message };
  }
}
"#,
        "          refused: {type: string}\n          why: {type: string}\n",
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let run_id = text(output, "run_id").ok_or("missing run_id")?.to_owned();
    assert_eq!(call_events(temp.path(), &run_id)?, [(0, "refused")]);
    Ok(())
}

fn requests(result: &RunResult) -> TestResult<Vec<String>> {
    Ok(result
        .pending_requests
        .iter()
        .map(|request| {
            request
                .as_object()
                .and_then(|request| text(request, "id"))
                .map(str::to_owned)
                .ok_or("a request names its id")
        })
        .collect::<Result<_, _>>()?)
}

fn env(root: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned())]);
    crate::support::insert_test_signing_env(&mut env);
    env
}

fn object(value: &JsonValue) -> TestResult<&JsonObject> {
    Ok(value.as_object().ok_or("expected an object")?)
}

fn text<'a>(object: &'a JsonObject, field: &str) -> Option<&'a str> {
    object.get(field).and_then(JsonValue::as_str)
}

#[test]
fn two_calls_waiting_on_approval_at_once_are_both_listed_and_answered_apart() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        r#"export default async function (ctx) {
  const [a, b] = await Promise.all([
    ctx.skill("../confirm-child", { value: "a" }),
    ctx.skill("../confirm-child", { value: "b" }),
  ]);
  return { confirmed: [Object.keys(a).length, Object.keys(b).length] };
}
"#,
        "          confirmed: {type: array}\n",
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    // The first call's wait didn't stop the second: the run waits on both.
    assert_eq!(
        requests(&started)?,
        ["prepare/0#confirm", "prepare/1#confirm"]
    );

    // An answer to one leaves the run waiting on the other alone.
    let one = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {"prepare/1#confirm": true}}),
    )?;
    assert_eq!(requests(&one)?, ["prepare/0#confirm"], "{:?}", one.output);
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [(0, "child"), (1, "child"), (1, "completed")]
    );

    let sealed = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {"prepare/0#confirm": true}}),
    )?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [
            (0, "child"),
            (1, "child"),
            (1, "completed"),
            (0, "completed")
        ]
    );
    Ok(())
}

/// Three calls run at once under `concurrency: 3`: each call's child marks
/// that it started, then waits until all three have, which it can only get
/// past if all three run together.
#[test]
fn a_script_runs_its_calls_at_once_up_to_its_concurrency() -> TestResult {
    let temp = tempdir()?;
    let barrier = temp.path().join("started");
    let child = temp.path().join("barrier-child");
    crate::support::write_test_skill_package(
        &child,
        "---\nname: barrier-child\ndescription: Waits until three of it have started.\n---\n# Barrier child\n\nMeet the others.\n",
        &format!(
            r#"minimum_runx: 0.12.0
methods:
  meet:
    steps:
      - id: meet
        tool:
          command: sh
          args:
            - -c
            - 'mkdir -p "{dir}" && : > "{dir}/$$" && until [ "$(ls "{dir}" | wc -l)" -ge 3 ]; do :; done; printf "{{\"met\":true}}\n"'
        outputs:
          met: {{type: boolean}}
"#,
            dir = barrier.display()
        ),
    )?;
    let skill_dir = temp.path().join("script-demo");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: script-demo\ndescription: Meets three children at once.\n---\n# Script demo\n\nMeet.\n",
        r#"minimum_runx: 0.12.0
methods:
  prepare:
    inputs:
      value: {type: string}
    steps:
      - id: prepare
        script: {module: prepare.mjs}
        calls:
          skills: [../barrier-child]
        max_calls: 3
        concurrency: 3
        outputs:
          met: {type: array}
"#,
    )?;
    std::fs::write(
        skill_dir.join("prepare.mjs"),
        r#"export default async function (ctx) {
  const met = await Promise.all([0, 1, 2].map(() => ctx.skill("../barrier-child", {})));
  return { met: met.map((child) => child.met) };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let run_id = text(output, "run_id").ok_or("missing run_id")?.to_owned();
    // Admitted in position order under the driver; recorded as they finished.
    let events = call_events(temp.path(), &run_id)?;
    assert_eq!(events[..3], [(0, "child"), (1, "child"), (2, "child")]);
    let mut completed = events[3..]
        .iter()
        .map(|(index, _)| *index)
        .collect::<Vec<_>>();
    completed.sort_unstable();
    assert_eq!(completed, [0, 1, 2]);
    assert_eq!(std::fs::read_dir(&barrier)?.count(), 3);
    Ok(())
}

/// A script package of one step, `prepare`, whose run.yaml step is `step`
/// and whose module is `script`.
fn write_one_step(root: &Path, step: &str, script: &str) -> TestResult<PathBuf> {
    let skill_dir = root.join("script-one");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: script-one\ndescription: One script step.\n---\n# Script one\n\nRun it.\n",
        &format!(
            "minimum_runx: 0.12.0\nmethods:\n  prepare:\n    inputs:\n      value: {{type: string}}\n    steps:\n      - id: prepare\n        script: {{module: prepare.mjs}}\n{step}"
        ),
    )?;
    std::fs::write(skill_dir.join("prepare.mjs"), script)?;
    Ok(skill_dir)
}

/// After Promise.all rejects, its siblings keep running: the one that
/// succeeds is still made and recorded, and the script catches the
/// rejection with its reason.
#[test]
fn a_rejected_sibling_leaves_the_others_running_and_settled() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          tools: [data.digest]\n        max_calls: 2\n        concurrency: 2\n        outputs:\n          caught: {type: string}\n",
        r#"export default async function (ctx, { value }) {
  try {
    await Promise.all([
      ctx.tool("fs.read", { path: "secret.txt" }),
      ctx.tool("data.digest", { value: "sibling", encoding: "utf8_text" }),
    ]);
    return { caught: "nothing" };
  } catch (rejected) {
    return { caught: rejected.reason };
  }
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(
        object(output.get("result").ok_or("no result")?)?.get("caught"),
        Some(&JsonValue::String("refused".to_owned()))
    );
    let run_id = text(output, "run_id").ok_or("missing run_id")?.to_owned();
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [(0, "refused"), (1, "claimed"), (1, "completed")]
    );
    Ok(())
}

/// Host-enforced termination never reaches the script: past max_calls the
/// step fails, and the script's catch never runs.
#[test]
fn termination_never_reaches_a_scripts_catch() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          tools: [data.digest]\n        max_calls: 2\n        outputs:\n          caught: {type: string}\n",
        r#"export default async function (ctx, { value }) {
  try {
    for (;;) await ctx.tool("data.digest", { value: "again", encoding: "utf8_text" });
  } catch (stopped) {
    return { caught: JSON.stringify(stopped) };
  }
}
"#,
    )?;
    let ended = run(temp.path(), skill_dir)?;
    let output = object(&ended.output)?;
    assert_eq!(text(output, "outcome"), Some("failed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert!(
        result.get("caught").is_none(),
        "the catch returned: {output:?}"
    );
    assert!(
        text(result, "message").is_some_and(|message| message.contains("max_calls of 2")),
        "{output:?}"
    );
    let run_id = text(output, "run_id").ok_or("missing run_id")?.to_owned();
    // Two calls ran; the third was never made.
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [
            (0, "claimed"),
            (0, "completed"),
            (1, "claimed"),
            (1, "completed")
        ]
    );
    Ok(())
}

/// Under `effects: none` a write is refused at admission at any depth: the
/// script's own and one its child skill makes, and nothing is written.
#[test]
fn effects_none_refuses_a_write_at_any_depth() -> TestResult {
    let temp = tempdir()?;
    crate::support::write_test_skill_package(
        &temp.path().join("writer-child"),
        "---\nname: writer-child\ndescription: Writes a file.\n---\n# Writer child\n\nWrite it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  write:\n    steps:\n      - id: write\n        tool: fs.write\n        inputs: {path: child.txt, contents: written}\n",
    )?;
    let skill_dir = write_one_step(
        temp.path(),
        "        effects: none\n        calls:\n          tools: [fs.write]\n          skills: [../writer-child]\n        max_calls: 2\n        outputs:\n          own: {type: string}\n          child: {type: string}\n",
        r#"export default async function (ctx) {
  const reason = (call) => call.then(() => "ran", (rejected) => rejected.reason);
  return {
    own: await reason(ctx.tool("fs.write", { path: "own.txt", contents: "written" })),
    child: await reason(ctx.skill("../writer-child", {})),
  };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(text(result, "own"), Some("refused"), "{result:?}");
    assert_ne!(text(result, "child"), Some("ran"), "{result:?}");
    assert!(!temp.path().join("own.txt").exists());
    assert!(!temp.path().join("child.txt").exists());
    assert!(!temp.path().join("writer-child/child.txt").exists());
    Ok(())
}

/// A child skill whose own step takes a malformed input fails that one call
/// with the child's reason, and leaves the script to decide; it never aborts
/// the parent run.
#[test]
fn a_childs_malformed_step_input_fails_the_call_not_the_run() -> TestResult {
    let temp = tempdir()?;
    crate::support::write_test_skill_package(
        &temp.path().join("bad-child"),
        "---\nname: bad-child\ndescription: Calls a tool without its input.\n---\n# Bad child\n\nTry it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  write:\n    steps:\n      - id: write\n        tool: fs.write\n        inputs: {path: child.txt, content: written}\n",
    )?;
    let skill_dir = write_one_step(
        temp.path(),
        "        effects: none\n        calls:\n          skills: [../bad-child]\n        max_calls: 1\n        outputs:\n          reason: {type: string}\n          message: {type: string}\n",
        r#"export default async function (ctx) {
  try {
    await ctx.skill("../bad-child", {});
    return { reason: "ran", message: "" };
  } catch (rejected) {
    return { reason: rejected.reason, message: rejected.message };
  }
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(text(result, "reason"), Some("failed"), "{result:?}");
    assert!(
        text(result, "message").is_some_and(|message| message.contains("contents")),
        "the child's reason: {result:?}"
    );
    Ok(())
}

/// Each clock read the stream of run `run_id` records, as `(index, millis)`.
fn clock_reads(root: &Path, run_id: &str) -> TestResult<Vec<(u32, i64)>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .filter(|record| record.stream.as_str() == run_id)
        .filter_map(|record| match record.event {
            RunEventFact::Clock { index, millis, .. } => Some((index, millis)),
            _ => None,
        })
        .collect())
}

/// `ctx.now()` is read once and recorded at its position: the drive after
/// the approval replays the first read's recorded time, records only the
/// read it makes past the record, and returns both.
#[test]
fn ctx_now_is_recorded_at_its_position_and_replays_the_same() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        r#"export default async function (ctx, { value }) {
  const first = ctx.now();
  await ctx.skill("../confirm-child", { value });
  return { first, second: ctx.now() };
}
"#,
        "          first: {type: number}\n          second: {type: number}\n",
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let waiting = requests(&started)?;
    assert_eq!(waiting, ["prepare/1#confirm"]);
    let first = clock_reads(temp.path(), &run_id)?;
    let [(0, recorded)] = first[..] else {
        return Err(format!("the first read is recorded at its position: {first:?}").into());
    };

    let sealed = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {waiting[0].as_str(): true}}),
    )?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let reads = clock_reads(temp.path(), &run_id)?;
    let [(0, again), (2, second)] = reads[..] else {
        return Err(format!("replay records nothing again: {reads:?}").into());
    };
    assert_eq!(again, recorded);
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(
        serde_json::to_value(result)?,
        serde_json::json!({"first": recorded, "second": second}),
        "the script read the recorded time again, and the new one"
    );
    Ok(())
}
