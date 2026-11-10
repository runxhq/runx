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
    run_on(LocalOrchestrator::default(), root, skill_dir)
}

/// Run `skill_dir` on `orchestrator`.
fn run_on(
    orchestrator: LocalOrchestrator,
    root: &Path,
    skill_dir: PathBuf,
) -> TestResult<RunResult> {
    run_under(orchestrator, root, skill_dir, &env(root))
}

/// Run `skill_dir` on `orchestrator` under `env`.
fn run_under(
    orchestrator: LocalOrchestrator,
    root: &Path,
    skill_dir: PathBuf,
    env: &BTreeMap<String, String>,
) -> TestResult<RunResult> {
    Ok(orchestrator.run_skill_with_binding(
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
            env: env.clone(),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        None,
        None,
        None,
    )?)
}

fn resume(root: &Path, run_id: &str, answers: serde_json::Value) -> TestResult<RunResult> {
    resume_under(root, run_id, answers, env(root))
}

/// Resume `run_id` with `answers` under `env`.
fn resume_under(
    root: &Path,
    run_id: &str,
    answers: serde_json::Value,
    env: BTreeMap<String, String>,
) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().resume(&ResumeRequest {
        run_id: run_id.to_owned(),
        answers: crate::support::bind_resume(&env, root, run_id, answers)?,
        receipt_dir: Some(root.join("receipts")),
        env,
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
                    RunCallOutcome::Waited { .. } => "waited",
                    RunCallOutcome::Read { .. } => "read",
                    RunCallOutcome::TooLarge { .. } => "too_large",
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

/// A run's receipt tree is a budget its calls draw from as they're admitted,
/// as `max_calls` is. A skill call adds `RECEIPTS_PER_SKILL_CALL` receipts:
/// its own, its child run's, and that run's one step. The call that would
/// take the tree past its bound is refused before it runs, naming the
/// bound, and the run seals with every call before it.
#[test]
fn a_call_past_the_runs_receipt_budget_is_refused_and_the_calls_before_it_seal() -> TestResult {
    let temp = tempdir()?;
    crate::support::write_test_skill_package(
        &temp.path().join("digest-child"),
        "---\nname: digest-child\ndescription: Digests a value.\n---\n# Digest child\n\nDigest it.\n",
        r#"minimum_runx: 0.12.0
methods:
  digest:
    inputs:
      value: {type: string}
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: $input.value
          encoding: utf8_text
"#,
    )?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          skills: [../digest-child]\n        max_calls: 3\n        outputs:\n          sealed: {type: array}\n          refused: {type: string}\n",
        r#"export default async function (ctx) {
  const sealed = [];
  for (const value of ["first", "second", "third"]) {
    try {
      await ctx.skill("../digest-child", { value });
      sealed.push(value);
    } catch (rejected) {
      return { sealed, refused: rejected.message };
    }
  }
  return { sealed, refused: "" };
}
"#,
    )?;
    let workflow = crate::support::method_workflow(&skill_dir, "prepare")?;
    let mut options =
        crate::support::signed_runtime_options(crate::support::run_in(temp.path(), "run_budget")?)?;
    options.env.extend(env(temp.path()));
    // The root, its script step, and two skill calls.
    let budget = 2 + 2 * runx_contracts::RECEIPTS_PER_SKILL_CALL;
    options.max_receipts = budget;
    let runtime = runx_runtime::Runtime::new(
        runx_runtime::adapters::javascript::JavaScriptAdapter::default(),
        options,
    );

    let run = runtime.run_workflow_with_host(&skill_dir, workflow, &mut runx_runtime::NoopHost)?;

    assert_eq!(
        run.receipt.seal.outcome,
        runx_contracts::RunOutcome::Completed
    );
    let step = run.steps.first().ok_or("the script step ran")?;
    assert_eq!(
        step.contract.get("sealed"),
        Some(&JsonValue::Array(vec![
            JsonValue::String("first".to_owned()),
            JsonValue::String("second".to_owned()),
        ]))
    );
    assert_eq!(
        step.contract.get("refused").and_then(JsonValue::as_str),
        Some(
            format!("it would take the run's receipt tree past its bound of {budget} receipts")
                .as_str()
        )
    );
    // The step's receipt names the two calls that sealed, and the tree is
    // the budget exactly.
    assert_eq!(step.receipt.lineage.children.len(), 2);
    let receipts = 1 + run
        .steps
        .iter()
        .map(|step| 1 + step.nested_receipts.len())
        .sum::<usize>();
    assert_eq!(receipts, budget);
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

/// The time each wait the stream of run `run_id` settled settled at, by its
/// position.
fn waited(root: &Path, run_id: &str) -> TestResult<Vec<(u32, i64)>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .filter(|record| record.stream.as_str() == run_id)
        .filter_map(|record| match record.event {
            RunEventFact::Call { index, outcome, .. } => match *outcome {
                RunCallOutcome::Waited { at } => Some((index, at)),
                _ => None,
            },
            _ => None,
        })
        .collect())
}

/// What `result` waits on, each request as its host reads it.
fn waits(result: &RunResult) -> TestResult<Vec<runx_contracts::ResolutionRequest>> {
    Ok(result
        .pending_requests
        .iter()
        .map(|request| serde_json::from_value(serde_json::to_value(request)?))
        .collect::<Result<_, _>>()?)
}

const WAIT_STEP: &str = "        calls:\n          tools: [data.digest]\n        max_calls: 1\n        outputs:\n          now: {type: number}\n          at: {type: number}\n";

/// A wait is a pending request the run waits on, runx's own: it takes no
/// answer, and a resume at or after its `until` settles it, at the time the
/// kernel records, which is what the wait returns.
#[test]
fn a_wait_keeps_the_run_waiting_until_a_resume_at_or_after_it_settles_it() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_one_step(
        temp.path(),
        WAIT_STEP,
        r#"export default async function (ctx) {
  const now = ctx.now();
  const at = await ctx.wait({ until: now });
  return { now, at };
}
"#,
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let [(0, now)] = clock_reads(temp.path(), &run_id)?[..] else {
        return Err("the script read the clock once".into());
    };
    assert_eq!(
        waits(&started)?,
        [runx_contracts::ResolutionRequest::Wait {
            id: "prepare/1".into(),
            until: now,
        }]
    );
    let refused = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {"prepare/1": true}}),
    )
    .err()
    .ok_or("an answer to a wait is refused")?;
    assert!(refused.to_string().contains("takes no answer"), "{refused}");

    // A resume with nothing to answer, at or after the wait's until.
    let sealed = resume(temp.path(), &run_id, serde_json::json!({}))?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let [(1, at)] = waited(temp.path(), &run_id)?[..] else {
        return Err("the wait settled once, at its position".into());
    };
    assert!(at >= now, "it settled at or after its until");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(
        serde_json::to_value(result)?,
        serde_json::json!({"now": now, "at": at}),
        "the wait returned the time the kernel recorded"
    );
    Ok(())
}

/// A resume before a wait's `until` settles nothing and reports the same
/// wait again.
#[test]
fn a_resume_before_a_waits_until_reports_it_again() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_one_step(
        temp.path(),
        WAIT_STEP,
        r#"export default async function (ctx) {
  const now = ctx.now();
  const at = await ctx.wait({ until: now + 3600000 });
  return { now, at };
}
"#,
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let waiting = waits(&started)?;
    assert!(
        matches!(&waiting[..], [runx_contracts::ResolutionRequest::Wait { id, .. }] if id.as_str() == "prepare/1"),
        "{waiting:?}"
    );
    let again = resume(temp.path(), &run_id, serde_json::json!({}))?;
    assert_eq!(waits(&again)?, waiting, "the same wait, reported again");
    assert!(
        waited(temp.path(), &run_id)?.is_empty(),
        "nothing settled it"
    );
    Ok(())
}

/// A child skill, `big`, whose process returns `letters` letters as its
/// `text`: a result past the inline bound, as a provider's large read is.
fn write_big(root: &Path, letters: usize) -> TestResult {
    Ok(crate::support::write_test_skill_package(
        &root.join("big"),
        "---\nname: big\ndescription: Returns a large text.\n---\n# Big\n\nReturn it.\n",
        &format!(
            r#"minimum_runx: 0.12.0
methods:
  make:
    steps:
      - id: make
        tool:
          command: sh
          args:
            - -c
            - "printf '{{\"text\":\"' && head -c {letters} /dev/zero | tr '\\0' a && printf '\"}}'"
        outputs:
          text: {{type: string}}
"#
        ),
    )?)
}

/// The artifacts a call at `index` of run `run_id`'s stream recorded its
/// output by, by output.
fn call_outputs(root: &Path, run_id: &str, index: u32) -> TestResult<BTreeMap<String, String>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .filter(|record| record.stream.as_str() == run_id)
        .filter_map(|record| match record.event {
            RunEventFact::Call {
                index: at, outcome, ..
            } if at == index => match *outcome {
                RunCallOutcome::Completed { outputs, .. } => Some(outputs),
                _ => None,
            },
            _ => None,
        })
        .flatten()
        .filter_map(|part| {
            part.output
                .map(|output| (output.to_string(), part.artifact.digest.to_string()))
        })
        .collect())
}

/// A field of a call's result past the inline bound reaches the script as a
/// reference, the artifact that holds it in its own words, which `ctx.read`
/// reads as the value it would have held inline, and a drive after a wait
/// replays the same reference and the same read.
#[test]
fn a_result_past_the_inline_bound_arrives_by_reference_and_reads_the_same_after_replay()
-> TestResult {
    let temp = tempdir()?;
    write_big(temp.path(), 100_000)?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          skills: [../big]\n        max_calls: 3\n        outputs:\n          kind: {type: string}\n          digest: {type: string}\n          held: {type: object}\n          length: {type: number}\n          letters: {type: boolean}\n",
        r#"export default async function (ctx) {
  const made = await ctx.skill("../big", {});
  const kind = typeof made.text;
  const { ref, size_bytes, labels } = made.text;
  await ctx.wait({ until: ctx.now() });
  const text = await ctx.read(made.text);
  return {
    kind,
    digest: made.text.digest,
    held: { schema: made.text.schema, ref, size_bytes, labels },
    length: text.length,
    letters: text === "a".repeat(100000),
  };
}
"#,
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    assert_eq!(requests(&started)?, ["prepare/2"], "the script waits");
    let sealed = resume(temp.path(), &run_id, serde_json::json!({}))?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    let recorded = call_outputs(temp.path(), &run_id, 0)?;
    let digest = recorded.get("text").ok_or("the text is its own artifact")?;
    assert_eq!(
        serde_json::to_value(result)?,
        serde_json::json!({
            "kind": "object",
            "digest": digest,
            "held": {
                "schema": "runx.artifact.v1",
                "ref": format!("runx:project-artifact:{digest}"),
                "size_bytes": 100_002,
                "labels": {"sharing": "configured_model"},
            },
            "length": 100_000,
            "letters": true,
        }),
        "the replayed reference names the artifact its call recorded, and reads as the text"
    );
    assert_eq!(
        call_events(temp.path(), &run_id)?
            .iter()
            .filter(|(index, _)| *index == 0)
            .count(),
        2,
        "the replay ran the call that made the text no more"
    );
    Ok(())
}

/// `ctx.read` reads only what fits the worker's input bound: past it, the
/// read is refused as too large, naming the size and the bound.
#[test]
fn a_read_past_the_workers_input_bound_is_refused_as_too_large() -> TestResult {
    let temp = tempdir()?;
    write_big(temp.path(), 5_000_000)?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          skills: [../big]\n        max_calls: 2\n        outputs:\n          reason: {type: string}\n          message: {type: string}\n",
        r#"export default async function (ctx) {
  const made = await ctx.skill("../big", {});
  try {
    await ctx.read(made.text);
    return { reason: "read", message: "" };
  } catch (refused) {
    return { reason: refused.reason, message: refused.message };
  }
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(text(result, "reason"), Some("too_large"), "{result:?}");
    let message = text(result, "message").ok_or("a message")?;
    assert!(
        message.contains("5000002") && message.contains("4194304"),
        "it names the size and the bound: {message}"
    );
    Ok(())
}

/// A reference given to a call reaches the callee as the artifact, its
/// content checked against the input it's given to, and one returned is the
/// output field's value, checked against its type. A reference no call of
/// the step returned is refused, naming it.
#[test]
fn a_reference_given_to_a_call_or_returned_is_its_artifact_and_a_made_up_one_is_refused()
-> TestResult {
    let temp = tempdir()?;
    write_big(temp.path(), 100_000)?;
    write_sink(temp.path())?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          skills: [../big, ../sink]\n        max_calls: 3\n        outputs:\n          length: {type: number}\n          made_up: {type: string}\n          text: {type: string}\n",
        r#"export default async function (ctx) {
  const made = await ctx.skill("../big", {});
  const measured = await ctx.skill("../sink", { text: made.text });
  const digest = "sha256:" + "a".repeat(64);
  let made_up;
  try {
    await ctx.skill("../sink", {
      text: {
        schema: "runx.artifact.v1",
        ref: "runx:project-artifact:" + digest,
        digest,
        size_bytes: 3,
        media_type: "application/json",
        path: "/nowhere.json",
        labels: { sharing: "configured_model" },
      },
    });
    made_up = "accepted";
  } catch (refused) {
    made_up = refused.reason + ": " + refused.message;
  }
  return { length: measured.length, made_up, text: made.text };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(
        result.get("length").and_then(JsonValue::as_u64),
        Some(100_000),
        "the child read the artifact: {result:?}"
    );
    let made_up = text(result, "made_up").ok_or("made_up")?;
    assert!(
        made_up.starts_with("refused") && made_up.contains(&"a".repeat(64)),
        "a made-up reference is refused, naming it: {made_up}"
    );
    assert_eq!(
        text(result, "text").map(str::len),
        Some(100_000),
        "the returned reference is the field's value"
    );
    Ok(())
}

/// A child skill, `sink`, that measures the text it's given in JavaScript.
fn write_sink(root: &Path) -> TestResult {
    let sink = root.join("sink");
    crate::support::write_test_skill_package(
        &sink,
        "---\nname: sink\ndescription: Measures a text.\n---\n# Sink\n\nMeasure it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  measure:\n    inputs:\n      text: {type: string}\n    steps:\n      - id: measure\n        inputs: {text: $input.text}\n        tool: {module: measure.mjs, export: measure}\n        outputs: {length: {type: number}}\n",
    )?;
    std::fs::write(
        sink.join("measure.mjs"),
        "export function measure({ text }) { return { length: text.length }; }\n",
    )?;
    Ok(())
}

/// A script's live worker is its own, apart from the pool, so a call whose
/// child runs JavaScript starts a worker of its own while the script waits
/// on it, rather than waiting on the script's.
#[test]
fn a_scripts_call_that_runs_javascript_never_waits_on_the_scripts_worker() -> TestResult {
    let temp = tempdir()?;
    write_sink(temp.path())?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          skills: [../sink]\n        max_calls: 1\n        outputs:\n          length: {type: number}\n",
        r#"export default async function (ctx) {
  const measured = await ctx.skill("../sink", { text: "four" });
  return { length: measured.length };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(result.get("length").and_then(JsonValue::as_u64), Some(4));
    Ok(())
}

/// Each run records the implementation that started it, decided from its
/// own method: a script's run records the digest of the sources the runtime
/// was built from and the one its worker reports, which a resume must match;
/// the child it calls, whose method runs no JavaScript, records no worker.
#[test]
fn a_run_records_the_implementation_that_started_it() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        DIGEST_THEN_CONFIRM,
        "          digested: {type: array}\n          confirmed: {type: array}\n",
    )?;
    let paused = run(temp.path(), skill_dir)?;
    assert_eq!(
        requests(&paused)?,
        ["prepare/1#confirm"],
        "{:?}",
        paused.output
    );
    let run_id = text(object(&paused.output)?, "run_id").ok_or("missing run_id")?;
    let runtime = runx_contracts::schema::NonEmptyString::from(env!("RUNX_RUNTIME_SOURCE_DIGEST"));
    assert_eq!(
        started_implementation(temp.path(), run_id)?,
        Some(runx_contracts::RunImplementation {
            runtime: runtime.clone(),
            worker: Some(env!("RUNX_JS_WORKER_SOURCE_DIGEST").into()),
        })
    );
    let child = runx_runtime::run_record::read_run(&env(temp.path()), temp.path(), run_id, None)?
        .events
        .into_iter()
        .find_map(|record| match record.event {
            RunEventFact::ChildStarted { implementation, .. } => Some(implementation),
            _ => None,
        })
        .ok_or("the child started")?;
    assert_eq!(
        child,
        Some(runx_contracts::RunImplementation {
            runtime,
            worker: None,
        })
    );
    Ok(())
}

/// The implementation a run's `Started` event records.
fn started_implementation(
    root: &Path,
    run_id: &str,
) -> TestResult<Option<runx_contracts::RunImplementation>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .find_map(|record| match record.event {
            RunEventFact::Started { implementation, .. } => Some(implementation),
            _ => None,
        })
        .ok_or("the run started")?)
}

/// A worker at `root/<name>` that marks each start of it in
/// `root/<name>.starts`, then does `then`.
#[cfg(unix)]
fn write_worker(root: &Path, name: &str, then: &str) -> TestResult<String> {
    use std::os::unix::fs::PermissionsExt;
    let worker = root.join(name);
    std::fs::write(
        &worker,
        format!(
            "#!/bin/sh\necho start >> '{}.starts'\n{then}\n",
            worker.display()
        ),
    )?;
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755))?;
    Ok(worker.to_string_lossy().into_owned())
}

/// A worker that greets as one built from the sources `digest` names, then
/// reads until it's stopped.
#[cfg(unix)]
fn write_worker_reporting(root: &Path, digest: &str) -> TestResult<String> {
    let ready = serde_json::to_string(&serde_json::json!({
        "type": "ready",
        "protocol_version": runx_contracts::javascript_worker::PROTOCOL_VERSION,
        "source_digest": digest,
    }))?;
    let length = u32::try_from(ready.len())?
        .to_be_bytes()
        .iter()
        .map(|byte| format!("\\{byte:03o}"))
        .collect::<String>();
    write_worker(
        root,
        "reporting",
        &format!("printf '{length}{ready}'\nexec /bin/cat >/dev/null"),
    )
}

/// The worker beside this runx, started through a wrapper that marks each
/// start.
#[cfg(unix)]
fn write_counted_worker(root: &Path) -> TestResult<String> {
    let beside = std::env::current_exe()?
        .parent()
        .and_then(Path::parent)
        .ok_or("the test binary has no build directory")?
        .join("runx-js-worker");
    if !beside.is_file() {
        return Err(format!("no worker at {}", beside.display()).into());
    }
    // The worker refuses any environment, and the shell gives it one.
    write_worker(
        root,
        "counted",
        &format!("exec /usr/bin/env -i '{}'", beside.display()),
    )
}

/// How many times `worker` was started.
#[cfg(unix)]
fn starts(worker: &str) -> TestResult<usize> {
    match std::fs::read_to_string(format!("{worker}.starts")) {
        Ok(marks) => Ok(marks.lines().count()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

/// Under `worker`.
#[cfg(unix)]
fn with_worker(root: &Path, worker: String) -> BTreeMap<String, String> {
    let mut env = env(root);
    env.insert("RUNX_JS_WORKER_PATH".to_owned(), worker);
    env
}

/// A run whose method runs no JavaScript starts no worker and records none,
/// so a worker of other sources never refuses its resume.
#[cfg(unix)]
#[test]
fn a_run_that_runs_no_javascript_records_no_worker_and_resumes_under_another() -> TestResult {
    let temp = tempdir()?;
    write_script_skill(
        temp.path(),
        DIGEST_THEN_CONFIRM,
        "          digested: {type: array}\n          confirmed: {type: array}\n",
    )?;
    let paused = run(temp.path(), temp.path().join("confirm-child"))?;
    assert_eq!(requests(&paused)?, ["confirm"], "{:?}", paused.output);
    let run_id = text(object(&paused.output)?, "run_id").ok_or("missing run_id")?;
    let recorded = started_implementation(temp.path(), run_id)?.ok_or("no implementation")?;
    assert_eq!(recorded.worker, None);

    let worker = write_worker_reporting(temp.path(), "sha256:other")?;
    let sealed = resume_under(
        temp.path(),
        run_id,
        serde_json::json!({"approvals": {"confirm": true}}),
        with_worker(temp.path(), worker.clone()),
    )?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(starts(&worker)?, 0, "the resume started no worker");
    Ok(())
}

/// A run of a worker tool starts one worker process, which learns the
/// worker's digest as the run starts and then runs the tool, and records
/// that digest.
#[cfg(unix)]
#[test]
fn a_javascript_run_starts_one_worker_process_and_records_it() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("echo");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: echo\ndescription: Echoes a value.\n---\n# Echo\n\nEcho the value.\n",
        "minimum_runx: 0.12.0\nmethods:\n  echo:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: echo\n        tool: {module: echo.mjs}\n        inputs:\n          value: $input.value\n        outputs:\n          value: {type: string}\n",
    )?;
    std::fs::write(
        skill_dir.join("echo.mjs"),
        "export default ({ value }) => ({ value });\n",
    )?;
    let worker = write_counted_worker(temp.path())?;
    let sealed = run_under(
        LocalOrchestrator::default(),
        temp.path(),
        skill_dir,
        &with_worker(temp.path(), worker.clone()),
    )?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(
        starts(&worker)?,
        1,
        "one worker learned the digest and ran the tool"
    );
    let run_id = text(output, "run_id").ok_or("missing run_id")?;
    let recorded = started_implementation(temp.path(), run_id)?.ok_or("no implementation")?;
    assert_eq!(
        recorded.worker.as_deref(),
        Some(env!("RUNX_JS_WORKER_SOURCE_DIGEST"))
    );
    Ok(())
}

/// A run whose method runs JavaScript doesn't start under a worker that
/// reports other sources than this runx, built beside the worker's, expects:
/// the refusal names both, and no run is recorded.
#[cfg(unix)]
#[test]
fn a_javascript_run_does_not_start_on_a_worker_of_other_sources() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        DIGEST_THEN_CONFIRM,
        "          digested: {type: array}\n          confirmed: {type: array}\n",
    )?;
    let worker = write_worker_reporting(temp.path(), "sha256:other")?;
    let refused = run_under(
        LocalOrchestrator::default(),
        temp.path(),
        skill_dir,
        &with_worker(temp.path(), worker),
    )
    .err()
    .ok_or("the run was refused")?
    .to_string();
    assert!(
        refused.contains(&format!(
            "built from other sources (sha256:other) than this runx expects ({})",
            env!("RUNX_JS_WORKER_SOURCE_DIGEST")
        )),
        "{refused}"
    );
    let runs = runx_runtime::run_record::list_runs(
        &env(temp.path()),
        temp.path(),
        &runx_runtime::run_record::RunFilter::default(),
    )?;
    assert!(runs.runs.is_empty(), "{:?}", runs.runs);
    Ok(())
}

/// A run whose method runs JavaScript doesn't resume under a worker that
/// reports other sources, and the refusal names both.
#[cfg(unix)]
#[test]
fn a_resume_under_a_worker_that_reports_other_sources_is_refused() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        DIGEST_THEN_CONFIRM,
        "          digested: {type: array}\n          confirmed: {type: array}\n",
    )?;
    let paused = run(temp.path(), skill_dir)?;
    assert_eq!(
        requests(&paused)?,
        ["prepare/1#confirm"],
        "{:?}",
        paused.output
    );
    let run_id = text(object(&paused.output)?, "run_id").ok_or("missing run_id")?;
    let started = started_implementation(temp.path(), run_id)?.ok_or("no implementation")?;
    let recorded = started.worker.ok_or("the run records its worker")?;

    let worker = write_worker_reporting(temp.path(), "sha256:other")?;
    let refused = resume_under(
        temp.path(),
        run_id,
        serde_json::json!({"approvals": {"prepare/1#confirm": true}}),
        with_worker(temp.path(), worker),
    )
    .err()
    .ok_or("the resume was refused")?
    .to_string();
    assert!(
        refused.contains(recorded.as_str()) && refused.contains("sha256:other"),
        "{refused}"
    );
    Ok(())
}

/// A connector, `measure`, whose `text.measure` reads the length of the text
/// it's given, through a process.
fn write_measure_connector(root: &Path) -> TestResult {
    let tool = root.join("connectors/measure/tools/text/measure");
    std::fs::create_dir_all(&tool)?;
    std::fs::write(
        root.join("connectors/measure/connector.yaml"),
        r#"schema: runx.connector.v1
name: measure
title: Measure
description: Measures a text.
auth:
  none: {}
placements:
  local:
    auth: [none]
operations:
  text.measure:
    description: Measure the text given.
    verb: read
    scopes: [text.read]
    target:
      template: "text"
    input_schema:
      type: object
      required: [text]
      additionalProperties: false
      properties:
        text: {type: string}
    result_fields: [length]
    implementation:
      process:
        tool: text.measure
"#,
    )?;
    std::fs::write(
        tool.join("manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "text.measure",
  "version": "0.1.0",
  "description": "Measure a text.",
  "process": {"command": "sh", "args": ["./measure.sh", "{{text}}"]},
  "inputs": {"text": {"type": "string", "description": "The text."}}
}
"#,
    )?;
    std::fs::write(
        tool.join("measure.sh"),
        "#!/bin/sh\nprintf '{\"length\":%s}\\n' \"${#1}\"\n",
    )?;
    Ok(())
}

/// A script's operation call reaches the connector its step's `calls`
/// names, whose requirement the step's operations declare.
#[test]
fn a_scripts_operation_call_reaches_its_connector() -> TestResult {
    let temp = tempdir()?;
    write_measure_connector(temp.path())?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          operations: [measure/text.measure]\n        max_calls: 1\n        outputs:\n          measured: {type: object}\n",
        r#"export default async function (ctx) {
  return { measured: await ctx.operation("measure/text.measure", { text: "four" }) };
}
"#,
    )?;
    // Connector operations run on the kernel's local effects.
    let orchestrator = LocalOrchestrator::with_effects_and_environment(
        runx_runtime::RuntimeEffectRegistry::local()?,
        env(temp.path()),
    );
    let sealed = run_on(orchestrator, temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert!(
        serde_json::to_string(result)?.contains("\"length\":4"),
        "the operation measured its text: {result:?}"
    );
    Ok(())
}

/// A reference given in an operation call's input reaches the operation as
/// the artifact's content, the same way a skill's or a tool's input does.
#[test]
fn a_reference_in_an_operations_input_reaches_it_as_the_artifacts_content() -> TestResult {
    let temp = tempdir()?;
    write_big(temp.path(), 100_000)?;
    write_measure_connector(temp.path())?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          skills: [../big]\n          operations: [measure/text.measure]\n        max_calls: 2\n        outputs:\n          measured: {type: object}\n",
        r#"export default async function (ctx) {
  const made = await ctx.skill("../big", {});
  const measured = await ctx.operation("measure/text.measure", { text: made.text });
  return { measured };
}
"#,
    )?;
    // Connector operations run on the kernel's local effects.
    let orchestrator = LocalOrchestrator::with_effects_and_environment(
        runx_runtime::RuntimeEffectRegistry::local()?,
        env(temp.path()),
    );
    let sealed = run_on(orchestrator, temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert!(
        serde_json::to_string(result)?.contains("\"length\":100000"),
        "the operation measured the artifact's text: {result:?}"
    );
    Ok(())
}

/// A child skill, `pack`, whose JavaScript returns `letters` letters as the
/// `text` of its `letters` packet.
fn write_pack(root: &Path, letters: usize) -> TestResult {
    let pack = root.join("pack");
    crate::support::write_test_skill_package(
        &pack,
        "---\nname: pack\ndescription: Packs letters.\n---\n# Pack\n\nPack them.\n",
        "minimum_runx: 0.12.0\nmethods:\n  pack:\n    steps:\n      - id: pack\n        tool: {module: pack.mjs, export: pack}\n        outputs:\n          letters: {type: object, packet: acme/pack.letters.v1}\n",
    )?;
    std::fs::create_dir_all(pack.join("packets"))?;
    std::fs::write(
        pack.join("packets/letters.v1.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/pack/letters/v1.json",
  "x-runx-packet-id": "acme/pack.letters.v1",
  "type": "object",
  "required": ["text"],
  "properties": { "text": { "type": "string" } },
  "additionalProperties": false
}"#,
    )?;
    std::fs::write(
        pack.join("pack.mjs"),
        format!(
            "export const pack = () => ({{ letters: {{ text: \"a\".repeat({letters}) }} }});\n"
        ),
    )?;
    Ok(())
}

/// The packet a script step's own `letters` output declares, requiring
/// `field`.
fn write_letters_packet(skill_dir: &Path, field: &str) -> TestResult {
    std::fs::create_dir_all(skill_dir.join("packets"))?;
    std::fs::write(
        skill_dir.join("packets/letters.v1.schema.json"),
        format!(
            r#"{{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/script-one/letters/v1.json",
  "x-runx-packet-id": "acme/script-one.letters.v1",
  "type": "object",
  "required": ["{field}"],
  "properties": {{ "{field}": {{ "type": "string" }} }},
  "additionalProperties": false
}}"#
        ),
    )?;
    Ok(())
}

/// A packet output reaches a script as its data, exactly as a `$steps`
/// reference reads it in a workflow, so it passes straight into another
/// call's packet input, as the contract's own example passes an enrichment
/// into qualification.
#[test]
fn a_packet_output_passes_straight_into_the_next_calls_packet_input() -> TestResult {
    let temp = tempdir()?;
    write_pack(temp.path(), 4)?;
    let unpack = temp.path().join("unpack");
    crate::support::write_test_skill_package(
        &unpack,
        "---\nname: unpack\ndescription: Counts packed letters.\n---\n# Unpack\n\nCount them.\n",
        "minimum_runx: 0.12.0\nmethods:\n  unpack:\n    inputs:\n      letters:\n        packet: acme/unpack.letters.v1\n    steps:\n      - id: count\n        tool: {module: unpack.mjs, export: count}\n        inputs: {letters: $input.letters}\n        outputs:\n          length: {type: number}\n",
    )?;
    std::fs::create_dir_all(unpack.join("packets"))?;
    std::fs::write(
        unpack.join("packets/letters.v1.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/unpack/letters/v1.json",
  "x-runx-packet-id": "acme/unpack.letters.v1",
  "type": "object",
  "required": ["text"],
  "properties": { "text": { "type": "string" } },
  "additionalProperties": false
}"#,
    )?;
    std::fs::write(
        unpack.join("unpack.mjs"),
        "export const count = ({ letters }) => ({ length: letters.text.length });\n",
    )?;
    let skill_dir = write_one_step(
        temp.path(),
        "        calls:\n          skills: [../pack, ../unpack]\n        max_calls: 2\n        outputs:\n          letters: {type: object}\n          length: {type: number}\n",
        r#"export default async function (ctx) {
  const packed = await ctx.skill("../pack", {});
  const counted = await ctx.skill("../unpack", { letters: packed.letters });
  return { letters: packed.letters, length: counted.length };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "outcome"), Some("completed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(
        serde_json::to_value(result.get("letters").ok_or("letters")?)?,
        serde_json::json!({ "text": "aaaa" }),
        "the script held the packet's data"
    );
    assert_eq!(
        serde_json::to_value(result.get("length").ok_or("length")?)?,
        serde_json::json!(4)
    );
    Ok(())
}

/// A packet output's field past the inline bound reaches the script as a
/// reference to the packet's artifact. `ctx.read` of it returns exactly what
/// the field would have held inline, the packet's data, and the reference
/// returned as the script's own packet output is the packet, checked against
/// that output's packet schema.
#[test]
fn a_large_packet_reads_as_its_inline_value_and_returns_as_a_checked_packet() -> TestResult {
    let script = r#"export default async function (ctx) {
  const made = await ctx.skill("../pack", {});
  const read = await ctx.read(made.letters);
  return { kind: typeof made.letters, read, letters: made.letters };
}
"#;
    let step = "        calls:\n          skills: [../pack]\n        max_calls: 2\n        outputs:\n          kind: {type: string}\n          read: {type: object}\n          letters: {type: object, packet: acme/script-one.letters.v1}\n";
    let temp = tempdir()?;
    write_pack(temp.path(), 100_000)?;
    let skill_dir = write_one_step(temp.path(), step, script)?;
    write_letters_packet(&skill_dir, "text")?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    let letters = serde_json::json!({"text": "a".repeat(100_000)});
    assert_eq!(
        text(result, "kind"),
        Some("object"),
        "it arrived by reference"
    );
    assert_eq!(
        serde_json::to_value(result.get("read").ok_or("read")?)?,
        letters,
        "ctx.read returns the field as it would have held it inline"
    );
    assert_eq!(
        serde_json::to_value(result.get("letters").ok_or("letters")?)?,
        serde_json::json!({ "data": letters }),
        "the returned reference is the step's packet"
    );

    // The returned packet is checked against the output's packet schema.
    let temp = tempdir()?;
    write_pack(temp.path(), 100_000)?;
    let skill_dir = write_one_step(temp.path(), step, script)?;
    write_letters_packet(&skill_dir, "count")?;
    let failed = run(temp.path(), skill_dir)?;
    let output = object(&failed.output)?;
    assert_eq!(text(output, "outcome"), Some("failed"), "{output:?}");
    assert!(
        serde_json::to_string(output)?.contains("output violates schema"),
        "the packet schema refused it: {output:?}"
    );
    Ok(())
}

/// What `kept.txt` holds, which only a model on this machine may read.
const KEPT: &str = "the gate code is 4417";

/// A skill that admits `kept.txt` as `local_only`, runs the script step
/// `prepare` declared by `prepare`, and gives the agent step `answer` what
/// `answer_inputs` names. No managed agent works its runs, so their host
/// can't say its model runs on this machine.
fn write_local_only_skill(
    root: &Path,
    prepare: &str,
    script: &str,
    answer_inputs: &str,
) -> TestResult<PathBuf> {
    std::fs::write(root.join("kept.txt"), KEPT)?;
    let skill_dir = root.join("local-notes");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: local-notes\ndescription: Answers from notes kept on this machine.\n---\n# Local notes\n\n## Agent task: answer\n\nAnswer from the notes.\n",
        &format!(
            "minimum_runx: 0.12.0\nmethods:\n  answer:\n    inputs:\n      value: {{type: string}}\n    steps:\n      - id: kept\n        tool: artifact.admit\n        inputs: {{path: kept.txt, media_type: text/plain, labels: {{sharing: local_only}}}}\n      - id: prepare\n        script: {{module: prepare.mjs}}\n{prepare}      - id: answer\n        agent: answer\n        inputs:\n{answer_inputs}        outputs:\n          answer: {{type: string}}\n"
        ),
    )?;
    std::fs::write(skill_dir.join("prepare.mjs"), script)?;
    Ok(skill_dir)
}

/// A script's call that reads what's labelled `local_only` is withheld from
/// it as it is from a turn, when the run's host can't say its model runs on
/// this machine: the call settles as refused, and the script never sees what
/// it read. What the script makes is labelled by the `local_only` reference
/// it was given, so the turn after it is refused before it's asked, and never
/// sees it either.
#[test]
fn a_scripts_read_of_a_local_only_artifact_on_a_remote_host_settles_as_refused() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_local_only_skill(
        temp.path(),
        "        inputs:\n          kept: $steps.kept.artifact.ref\n        calls:\n          tools: [artifact.read]\n        max_calls: 1\n        outputs:\n          text: {type: string}\n",
        r#"export default async function (ctx, { kept }) {
  try {
    const read = await ctx.tool("artifact.read", { ref: kept, encoding: "utf8" });
    return { text: read.artifact_page.data };
  } catch (refused) {
    return { text: refused.reason + ": " + refused.message };
  }
}
"#,
        "          text: $steps.prepare.text\n",
    )?;
    let refused = run(temp.path(), skill_dir)?;
    assert!(requests(&refused)?.is_empty(), "{:?}", refused.output);
    let rendered = serde_json::to_string(&refused.output)?;
    assert!(!rendered.contains(KEPT), "{rendered}");
    assert!(
        rendered.contains(
            "refused: call 0 reads what's labelled local_only, and this step's host can't say its model runs on this machine"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "what it is given is labelled local_only, and its host can't say its model runs on this machine"
        ),
        "{rendered}"
    );
    Ok(())
}

/// A script given a `local_only` artifact can't return it: a returned
/// descriptor that no call of the step returned fails the step, so no turn
/// after it is given it.
#[test]
fn a_returned_local_only_descriptor_no_call_of_the_step_returned_fails_the_step() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_local_only_skill(
        temp.path(),
        "        inputs:\n          kept: $steps.kept.artifact\n        max_calls: 1\n        outputs:\n          kept: {type: object}\n",
        "export default async function (ctx, { kept }) {\n  return { kept };\n}\n",
        "          kept: $steps.prepare.kept\n",
    )?;
    let failed = run(temp.path(), skill_dir)?;
    let output = object(&failed.output)?;
    assert_eq!(text(output, "outcome"), Some("failed"), "{output:?}");
    let rendered = serde_json::to_string(output)?;
    assert!(
        rendered.contains("step prepare: output kept: ")
            && rendered.contains("names no artifact a call of the step returned"),
        "{rendered}"
    );
    assert!(
        requests(&failed)?.is_empty(),
        "no turn is asked: {rendered}"
    );
    Ok(())
}
