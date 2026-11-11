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
        closure_digest: None,
    })?)
}

/// Each call event the run records at its root, as `(index, outcome)`.
fn call_events(root: &Path, run_id: &str) -> TestResult<Vec<(u32, &'static str)>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .filter(|record| of_run(record.stream.as_str(), run_id))
        .filter_map(|record| match record.event {
            RunEventFact::Call { index, outcome, .. } => Some((
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
        [(0, "executing"), (0, "completed"), (1, "child")]
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
            (0, "executing"),
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

/// Three calls run at once under `concurrency: 3`: each call's child reads
/// through a process that marks that it started, then waits until all three
/// have, which it can only get past if all three run together. A process
/// read shares the workspace's lock, its `read` its author's promise. Changed
/// 11 November: each child ran a process step, which now holds the
/// workspace's lock alone, so three can't meet.
#[test]
fn a_script_runs_its_calls_at_once_up_to_its_concurrency() -> TestResult {
    let temp = tempdir()?;
    let barrier = temp.path().join("started");
    let tool = temp.path().join("connectors/meet/tools/meet/wait");
    std::fs::create_dir_all(&tool)?;
    std::fs::write(
        temp.path().join("connectors/meet/connector.yaml"),
        r#"schema: runx.connector.v1
name: meet
title: Meet
description: Reads once three have started.
auth:
  none: {}
placements:
  local:
    auth: [none]
operations:
  meet.wait:
    description: Mark a start, then read once three have started.
    verb: read
    scopes: [meet.read]
    target:
      template: "meet"
    result_fields: [met]
    implementation:
      process:
        tool: meet.wait
"#,
    )?;
    std::fs::write(
        tool.join("manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "meet.wait",
  "version": "0.1.0",
  "description": "Mark a start, then read once three have started.",
  "process": {"command": "sh", "args": ["./wait.sh"]},
  "inputs": {}
}
"#,
    )?;
    std::fs::write(
        tool.join("wait.sh"),
        format!(
            "#!/bin/sh\nmkdir -p \"{dir}\" && : > \"{dir}/$$\" && until [ \"$(ls \"{dir}\" | wc -l)\" -ge 3 ]; do :; done; printf '{{\"met\":true}}\\n'\n",
            dir = barrier.display()
        ),
    )?;
    let child = temp.path().join("barrier-child");
    crate::support::write_test_skill_package(
        &child,
        "---\nname: barrier-child\ndescription: Waits until three of it have started.\n---\n# Barrier child\n\nMeet the others.\n",
        r#"minimum_runx: 0.12.0
methods:
  meet:
    steps:
      - id: meet
        operation: meet/meet.wait
        inputs: {}
"#,
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
  return { met: met.map((child) => Boolean(child)) };
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

/// Recovery runs a script from its beginning against its record, and each
/// call it makes must be the call the record holds at that position. A script
/// changed since, making a call with another input or to another target
/// there, is a failed recovery that acts on nothing: no call executes and
/// nothing is recorded. A host that pins its run's package refuses a changed
/// script before any replay; a runtime that reads packages as they are, as a
/// test replay does, reaches this check.
#[test]
fn a_script_that_changed_since_its_record_recovers_nothing() -> TestResult {
    let script = |first: &str| {
        format!(
            "export default async function (ctx) {{\n  await {first};\n  await ctx.wait({{ until: 4102444800000 }});\n  return {{ done: true }};\n}}\n"
        )
    };
    let recorded = r#"ctx.tool("data.digest", { value: "first", encoding: "utf8_text" })"#;
    for changed in [
        r#"ctx.tool("data.digest", { value: "other", encoding: "utf8_text" })"#,
        r#"ctx.tool("data.compare", { actual: "first", expected: "first" })"#,
    ] {
        let temp = tempdir()?;
        let skill_dir = write_one_step(
            temp.path(),
            "        calls:\n          tools: [data.digest, data.compare]\n        max_calls: 2\n        outputs:\n          done: {type: boolean}\n",
            &script(recorded),
        )?;
        let started = run(temp.path(), skill_dir.clone())?;
        let run_id = text(object(&started.output)?, "run_id")
            .ok_or("missing run_id")?
            .to_owned();
        let before = call_events(temp.path(), &run_id)?;
        assert_eq!(before, [(0, "executing"), (0, "completed"), (1, "pending")]);

        std::fs::write(skill_dir.join("prepare.mjs"), script(changed))?;
        let paths = runx_runtime::paths::RunxPaths::resolve(&env(temp.path()), temp.path());
        let mut options = crate::support::signed_runtime_options(runx_runtime::RunLog::open(
            runx_runtime::kernel::store::KernelStore::for_project(&paths)?,
            &run_id,
        )?)?;
        options.env.extend(env(temp.path()));
        let refused = runx_runtime::Runtime::new(
            runx_runtime::adapters::javascript::JavaScriptAdapter::default(),
            options,
        )
        .run_workflow_with_host(
            &skill_dir,
            crate::support::method_workflow(&skill_dir, "prepare")?,
            &mut runx_runtime::NoopHost,
        )
        .err()
        .ok_or("a changed script is not recovered")?;
        assert!(
            refused.to_string().contains("can't be recovered at call 0"),
            "{changed}: {refused}"
        );
        assert_eq!(
            call_events(temp.path(), &run_id)?,
            before,
            "{changed}: nothing was executed or recorded"
        );
    }
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
        [(0, "refused"), (1, "executing"), (1, "completed")]
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
            (0, "executing"),
            (0, "completed"),
            (1, "executing"),
            (1, "completed")
        ]
    );
    Ok(())
}

/// Under `effects: none` a write is refused at any depth before the run
/// starts: the script's own listed write and its child skill's fixed one are
/// both named, no step runs, no worker starts and nothing is written.
/// Changed 11 November: before the run-start check, the script ran and each
/// write was refused at admission as the script made it.
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
    let refused = run(temp.path(), skill_dir)?;
    let output = object(&refused.output)?;
    assert_eq!(text(output, "outcome"), Some("blocked"), "{output:?}");
    assert_eq!(text(output, "seal_reason"), Some("authority_denied"));
    let reason = output
        .get("refusal")
        .and_then(|refusal| refusal.as_object())
        .and_then(|refusal| text(refusal, "reason"))
        .ok_or("the run names why it was refused")?;
    assert_eq!(
        reason,
        "the run's effects are none, so it changes nothing: the fs.write tool, which changes the workspace, in step prepare; the fs.write tool, which changes the workspace, in writer-child#write's step write, are each refused before the run starts"
    );
    let run_id = text(output, "run_id").ok_or("missing run_id")?.to_owned();
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        Vec::<(u32, &str)>::new(),
        "the script never ran"
    );
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
        "        calls:\n          skills: [../bad-child]\n        max_calls: 1\n        outputs:\n          reason: {type: string}\n          message: {type: string}\n",
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
        .filter(|record| of_run(record.stream.as_str(), run_id))
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
        .filter(|record| of_run(record.stream.as_str(), run_id))
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
        .filter(|record| of_run(record.stream.as_str(), run_id))
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

/// An array input past the inline bound reaches a script item by item, each
/// a reference: `ctx.read` gives an item's content, a call given an item is
/// given its content, and `ctx.read` of an inline input gives it back as it
/// is, so a script reads its inputs the same way at any size.
#[test]
fn an_array_input_past_the_inline_bound_reaches_a_script_item_by_item_by_reference() -> TestResult {
    let temp = tempdir()?;
    write_sink(temp.path())?;
    let skill_dir = temp.path().join("script-items");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: script-items\ndescription: Measures a batch item by item.\n---\n# Script items\n\nMeasure them.\n",
        r#"minimum_runx: 0.12.0
methods:
  prepare:
    inputs:
      value: {type: string}
    steps:
      - id: make
        tool: {module: make.mjs, export: make}
        outputs:
          items: {type: array}
      - id: prepare
        script: {module: prepare.mjs}
        inputs:
          items: $steps.make.items
          note: $input.value
        calls:
          skills: [../sink]
        max_calls: 2
        outputs:
          given: {type: array}
          first: {type: number}
          second: {type: number}
          note: {type: string}
"#,
    )?;
    std::fs::write(
        skill_dir.join("make.mjs"),
        "export function make() { return { items: [\"a\".repeat(40000), \"b\".repeat(40000)] }; }\n",
    )?;
    std::fs::write(
        skill_dir.join("prepare.mjs"),
        r#"export default async function (ctx, { items, note }) {
  const measured = await ctx.skill("../sink", { text: items[0] });
  const second = await ctx.read(items[1]);
  return {
    given: items.map((item) => typeof item === "object" && item.schema),
    first: measured.length,
    second: second.length,
    note: await ctx.read(note),
  };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = serde_json::to_value(output.get("result").ok_or("no result")?)?;
    assert_eq!(
        result["given"],
        serde_json::json!(["runx.artifact.v1", "runx.artifact.v1"]),
        "each item is a reference: {result}"
    );
    assert_eq!(result["first"], 40_000, "{result}");
    assert_eq!(result["second"], 40_000, "{result}");
    assert_eq!(result["note"], "hello", "{result}");
    Ok(())
}

/// A read of an inline value is no call: the worker gives the value back,
/// the run records nothing, and it draws nothing from `max_calls`. Five reads
/// and one call fit a `max_calls` of 1, and the call takes position 0.
#[test]
fn a_read_of_an_inline_value_is_no_call() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_one_step(
        temp.path(),
        "        inputs:\n          value: $input.value\n        calls:\n          tools: [data.digest]\n        max_calls: 1\n        outputs:\n          read: {type: string}\n",
        r#"export default async function (ctx, { value }) {
  let read = value;
  for (let time = 0; time < 5; time += 1) read = await ctx.read(read);
  await ctx.tool("data.digest", { value: read, encoding: "utf8_text" });
  return { read };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = object(output.get("result").ok_or("no result")?)?;
    assert_eq!(text(result, "read"), Some("hello"));
    let run_id = text(output, "run_id").ok_or("missing run_id")?.to_owned();
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [(0, "executing"), (0, "completed")],
        "the reads took no position"
    );
    Ok(())
}

/// A reference a script's output holds at any depth is the content of the
/// artifact it names: a call's result past the inline bound, nested in an
/// array of objects, comes back as the text, never as its descriptor. One
/// that names no artifact a call of the step returned fails the step, naming
/// where the output holds it.
#[test]
fn a_reference_nested_in_a_scripts_output_is_its_content_and_a_made_up_one_fails_the_step()
-> TestResult {
    let temp = tempdir()?;
    write_big(temp.path(), 100_000)?;
    let step = "        calls:\n          skills: [../big]\n        max_calls: 1\n        outputs:\n          outcomes: {type: array}\n";
    let skill_dir = write_one_step(
        temp.path(),
        step,
        r#"export default async function (ctx) {
  const made = await ctx.skill("../big", {});
  return { outcomes: [{ lead: "ada", review: { text: made.text } }] };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let result = serde_json::to_value(output.get("result").ok_or("no result")?)?;
    assert_eq!(
        result["outcomes"][0]["review"]["text"]
            .as_str()
            .map(str::len),
        Some(100_000),
        "the nested reference is the field's value: {}",
        result["outcomes"][0]["review"]
    );

    let made_up = tempdir()?;
    let skill_dir = write_one_step(
        made_up.path(),
        "        max_calls: 1\n        outputs:\n          outcomes: {type: array}\n",
        r#"export default async function () {
  const digest = "sha256:" + "a".repeat(64);
  const review = {
    schema: "runx.artifact.v1",
    ref: "runx:project-artifact:" + digest,
    digest,
    size_bytes: 3,
    media_type: "application/json",
    path: "/nowhere.json",
    labels: { sharing: "configured_model" },
  };
  return { outcomes: [{ lead: "ada", review }] };
}
"#,
    )?;
    let failed = run(made_up.path(), skill_dir)?;
    let output = object(&failed.output)?;
    assert_eq!(text(output, "outcome"), Some("failed"), "{output:?}");
    let rendered = serde_json::to_string(output)?;
    assert!(
        rendered.contains(&format!(
            "output outcomes[0].review: sha256:{} names no artifact a call of the step returned",
            "a".repeat(64)
        )),
        "{rendered}"
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
        runx_contracts::RunImplementation {
            runtime: runtime.clone(),
            worker: Some(env!("RUNX_JS_WORKER_SOURCE_DIGEST").into()),
        }
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
        runx_contracts::RunImplementation {
            runtime,
            worker: None,
        }
    );
    Ok(())
}

/// The implementation a run's `Started` event records.
fn started_implementation(
    root: &Path,
    run_id: &str,
) -> TestResult<runx_contracts::RunImplementation> {
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
    let recorded = started_implementation(temp.path(), run_id)?;
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
    let recorded = started_implementation(temp.path(), run_id)?;
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
    let started = started_implementation(temp.path(), run_id)?;
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
        letters,
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

/// A skill call that asks to reuse a result records what it asked, and
/// that it ran again with no candidate, once, on the call it made. A call
/// that asks for nothing records nothing, and both run their child as
/// before.
#[test]
fn a_skill_call_records_its_reuse_ask_once_and_runs() -> TestResult {
    let temp = tempdir()?;
    crate::support::write_test_skill_package(
        &temp.path().join("digest-child"),
        "---\nname: digest-child\ndescription: Digests a value.\n---\n# Digest child\n\nDigest it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  digest:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {value: $input.value, encoding: utf8_text}\n",
    )?;
    let skill_dir = write_one_step(
        temp.path(),
        "        inputs:\n          value: $input.value\n        calls:\n          skills: [../digest-child]\n        max_calls: 2\n        outputs:\n          asked: {type: integer}\n          plain: {type: integer}\n",
        r#"export default async function (ctx, { value }) {
  const asked = await ctx.skill("../digest-child", { value }, { reuse: { max_age_seconds: 604800 } });
  const plain = await ctx.skill("../digest-child", { value });
  return { asked: Object.keys(asked).length, plain: Object.keys(plain).length };
}
"#,
    )?;
    let sealed = run(temp.path(), skill_dir)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let run_id = text(output, "run_id").ok_or("missing run_id")?;
    let read = runx_runtime::run_record::read_run(&env(temp.path()), temp.path(), run_id, None)?;
    let asked = |wanted: u32| {
        read.events
            .iter()
            .filter(|record| of_run(record.stream.as_str(), run_id))
            .filter_map(|record| match &record.event {
                RunEventFact::Call { index, reuse, .. } if *index == wanted => Some(reuse.clone()),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        asked(0),
        [runx_contracts::RunCallReuse {
            max_age_seconds: 604_800,
            model: None,
            decision: runx_contracts::ReuseDecision::Recomputed {
                reason: runx_contracts::RecomputeReason::NoCandidate,
            },
        }],
        "recorded once, on the call that asked"
    );
    assert!(
        asked(1).is_empty(),
        "a call that asks for nothing records nothing"
    );
    Ok(())
}

/// `script` as a step whose calls are an approval-gated write under
/// `effects: ask`, a child that waits on its own ask, a wait and a digest.
fn write_cleanup_skill(root: &Path, script: &str) -> TestResult<PathBuf> {
    let skill_dir = write_script_skill(root, script, "          done: {type: string}\n")?;
    let manifest = skill_dir.join("run.yaml");
    let declared = std::fs::read_to_string(&manifest)?
        .replace(
            "          tools: [data.digest]\n",
            "          tools: [data.digest, fs.write]\n",
        )
        .replace(
            "        max_calls: 4\n",
            "        max_calls: 8\n        concurrency: 4\n        effects: ask\n",
        );
    std::fs::write(&manifest, declared)?;
    Ok(skill_dir)
}

/// How the run ended, and the message its step failed with.
fn ending(result: &RunResult) -> TestResult<(String, String)> {
    let output = object(&result.output)?;
    let message = output
        .get("result")
        .and_then(JsonValue::as_object)
        .and_then(|result| text(result, "message"))
        .unwrap_or_default();
    Ok((
        text(output, "outcome").unwrap_or_default().to_owned(),
        message.to_owned(),
    ))
}

/// How call `index` last settled in `events`.
fn last_of(events: &[(u32, &'static str)], index: u32) -> Option<&'static str> {
    events
        .iter()
        .rev()
        .find(|(known, _)| *known == index)
        .map(|(_, outcome)| *outcome)
}

/// The run a script's skill call at `index` started.
fn child_of(root: &Path, run_id: &str, index: u32) -> TestResult<String> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    read.events
        .into_iter()
        .find_map(|record| match record.event {
            RunEventFact::Call {
                index: known,
                outcome,
                ..
            } if known == index => match *outcome {
                RunCallOutcome::Child { run_id } => Some(run_id.as_str().to_owned()),
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| format!("call {index} started no child run").into())
}

/// How `child`, a run beneath `run_id`, sealed, as `run_id`'s read holds it.
fn seal_of(root: &Path, run_id: &str, child: &str) -> TestResult<serde_json::Value> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    read.events
        .into_iter()
        .filter(|record| record.stream.as_str() == child)
        .find_map(|record| match record.event {
            RunEventFact::Sealed { outcome, .. } => serde_json::to_value(outcome).ok(),
            _ => None,
        })
        .ok_or_else(|| format!("run {child} hasn't sealed").into())
}

/// After a throw, the kernel drops what the script started and never got
/// to: a write still waiting on its approval and a wait settle `cancelled`,
/// and a child waiting on its own person is cancelled, so it seals killed.
/// Only then does the step fail, with the script's own reason, and the run
/// waits on nothing.
#[test]
fn a_script_that_throws_drops_what_it_started_and_fails_with_its_own_reason() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_cleanup_skill(
        temp.path(),
        r#"export default async function (ctx, { value }) {
  ctx.tool("fs.write", { path: "never.txt", contents: value });
  ctx.skill("../confirm-child", { value });
  ctx.wait({ until: 4102444800000 });
  await ctx.tool("data.digest", { value, encoding: "utf8_text" });
  throw new Error("the script's own reason");
}
"#,
    )?;
    let ended = run(temp.path(), skill_dir)?;
    let (outcome, message) = ending(&ended)?;
    assert_eq!(outcome, "failed", "{:?}", ended.output);
    assert!(
        message.contains("the script's own reason"),
        "{:?}",
        ended.output
    );
    assert!(ended.pending_requests.is_empty(), "{:?}", ended.output);
    assert!(!temp.path().join("never.txt").exists());
    let run_id = text(object(&ended.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let events = call_events(temp.path(), &run_id)?;
    for index in [0, 1, 2] {
        assert_eq!(last_of(&events, index), Some("cancelled"), "{events:?}");
    }
    assert_eq!(last_of(&events, 3), Some("completed"), "{events:?}");
    let child = child_of(temp.path(), &run_id, 1)?;
    assert_eq!(seal_of(temp.path(), &run_id, &child)?, "killed");
    Ok(())
}

/// A script that returns with a call outstanding fails its step, tested at
/// the kernel: its output never counts, and the call it left is dropped.
#[test]
fn a_script_that_returns_with_a_call_outstanding_fails_and_never_seals_its_output() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_cleanup_skill(
        temp.path(),
        r#"export default async function (ctx, { value }) {
  ctx.skill("../confirm-child", { value });
  await ctx.tool("data.digest", { value, encoding: "utf8_text" });
  return { done: "early" };
}
"#,
    )?;
    let ended = run(temp.path(), skill_dir)?;
    let (outcome, message) = ending(&ended)?;
    assert_eq!(outcome, "failed", "{:?}", ended.output);
    assert!(
        message.contains("returned with call 0 outstanding"),
        "{:?}",
        ended.output
    );
    assert!(ended.pending_requests.is_empty(), "{:?}", ended.output);
    let run_id = text(object(&ended.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let events = call_events(temp.path(), &run_id)?;
    assert_eq!(last_of(&events, 0), Some("cancelled"), "{events:?}");
    assert_eq!(last_of(&events, 1), Some("completed"), "{events:?}");
    Ok(())
}

/// A rejection the script never handles ends it as a throw does: the
/// sibling still waiting on its person is dropped, and the step fails with
/// the rejection's own reason.
#[test]
fn an_unhandled_rejection_drops_its_siblings_and_fails_with_its_reason() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_cleanup_skill(
        temp.path(),
        r#"export default async function (ctx, { value }) {
  await Promise.all([
    ctx.skill("../confirm-child", { value }),
    ctx.tool("fs.read", { path: "secret.txt" }),
  ]);
  return { done: "never" };
}
"#,
    )?;
    let ended = run(temp.path(), skill_dir)?;
    let (outcome, message) = ending(&ended)?;
    assert_eq!(outcome, "failed", "{:?}", ended.output);
    assert!(message.contains("fs.read"), "{:?}", ended.output);
    assert!(ended.pending_requests.is_empty(), "{:?}", ended.output);
    let run_id = text(object(&ended.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let events = call_events(temp.path(), &run_id)?;
    assert_eq!(last_of(&events, 0), Some("cancelled"), "{events:?}");
    assert_eq!(last_of(&events, 1), Some("refused"), "{events:?}");
    Ok(())
}

/// A person declining a call's approval rejects its promise with the reason
/// `declined`, which the script catches like any rejection; nothing ran.
#[test]
fn a_declined_approval_rejects_its_call_as_declined_and_the_script_catches_it() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_cleanup_skill(
        temp.path(),
        r#"export default async function (ctx, { value }) {
  try {
    await ctx.tool("fs.write", { path: "never.txt", contents: value });
    return { done: "written" };
  } catch (rejected) {
    return { done: `${rejected.reason}/${rejected.code}` };
  }
}
"#,
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    assert_eq!(requests(&started)?, ["prepare/0/tool"]);

    let sealed = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {"prepare/0/tool": false}}),
    )?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(
        object(output.get("result").ok_or("no result")?)?.get("done"),
        Some(&JsonValue::String("declined/declined".to_owned()))
    );
    assert!(!temp.path().join("never.txt").exists());
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [(0, "pending"), (0, "declined")]
    );
    Ok(())
}

/// A fixed step whose approval a person declines seals the run declined,
/// the person's no, never a refusal of runx's.
#[test]
fn a_fixed_step_a_person_declines_seals_its_run_declined() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("fixed-write");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: fixed-write\ndescription: Writes a file.\n---\n# Fixed write\n\nWrite it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  prepare:\n    inputs:\n      value: {type: string}\n    effects: ask\n    steps:\n      - id: write\n        tool: fs.write\n        inputs: {path: never.txt, contents: $input.value}\n",
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    assert_eq!(requests(&started)?, ["write/tool"]);

    let sealed = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {"write/tool": false}}),
    )?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "outcome"), Some("declined"), "{output:?}");
    assert_eq!(
        text(output, "seal_reason"),
        Some("approval_declined"),
        "{output:?}"
    );
    assert!(!temp.path().join("never.txt").exists());
    Ok(())
}

/// A worker limit ends the script's step where it stands: a loop past the
/// engine's bound, a stack past its depth and memory past the worker's heap
/// never reach the script's catch, and the step fails. (A single buffer the
/// heap can't hold is the language's own RangeError, which a script may
/// catch: the heap's bound holds, and nothing past it was allocated.)
#[test]
fn a_worker_limit_never_reaches_a_scripts_catch() -> TestResult {
    for (limit, body) in [
        ("loop", "while (true) {}"),
        ("stack", "const recurse = () => recurse(); recurse();"),
        (
            "memory",
            "const held = []; while (true) held.push(new Array(1024).fill(held.length));",
        ),
    ] {
        let temp = tempdir()?;
        let skill_dir = write_one_step(
            temp.path(),
            "        calls:\n          tools: [data.digest]\n        max_calls: 1\n        outputs:\n          done: {type: string}\n",
            &format!(
                "export default async function (ctx) {{\n  try {{\n    {body}\n  }} catch (caught) {{\n    return {{ done: `caught ${{caught}}` }};\n  }}\n  return {{ done: \"past it\" }};\n}}\n"
            ),
        )?;
        let ended = run(temp.path(), skill_dir)?;
        let output = object(&ended.output)?;
        assert_eq!(
            text(output, "outcome"),
            Some("failed"),
            "{limit}: {output:?}"
        );
        let result = object(output.get("result").ok_or("no result")?)?;
        assert!(
            result.get("done").is_none(),
            "{limit}: the catch ran: {output:?}"
        );
    }
    Ok(())
}

/// An engine fault never reaches a script's catch: a child skill changed
/// while its call waited, so the run's resume is refused before the script
/// is given anything, and the run stays open, unsealed, for the package it
/// started from.
#[test]
fn an_engine_fault_never_reaches_a_scripts_catch() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        r#"export default async function (ctx, { value }) {
  try {
    const confirmed = await ctx.skill("../confirm-child", { value });
    return { confirmed: Object.keys(confirmed) };
  } catch (rejected) {
    return { confirmed: ["caught", rejected.reason] };
  }
}
"#,
        "          confirmed: {type: array}
",
    )?;
    let started = run(temp.path(), skill_dir)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    assert_eq!(requests(&started)?, ["prepare/0#confirm"]);
    let child = temp.path().join("confirm-child/run.yaml");
    let declared = std::fs::read_to_string(&child)?;
    std::fs::write(
        &child,
        declared.replace("Confirm the digest.", "Confirm it."),
    )?;

    let refused = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {"prepare/0#confirm": true}}),
    );
    assert!(
        refused.is_err(),
        "{:?}",
        refused.map(|result| result.output)
    );
    assert_eq!(
        call_events(temp.path(), &run_id)?,
        [(0, "child")],
        "nothing settled the call, so nothing reached the catch"
    );
    Ok(())
}

/// Four scripts, each calling the next as a child skill, the last returning
/// where it stands: `level-0` is the root, and each answers `n>` and what
/// the next reached, or `nx` and why its call rejected.
fn write_levels(root: &Path) -> TestResult<PathBuf> {
    for level in 0..=3 {
        let (calls, script) = if level == 3 {
            (
                "          tools: [data.digest]\n".to_owned(),
                "export default async function () {\n  return { reached: \"3\" };\n}\n".to_owned(),
            )
        } else {
            let next = level + 1;
            (
                format!("          skills: [../level-{next}]\n"),
                format!(
                    "export default async function (ctx) {{\n  try {{\n    const next = await ctx.skill(\"../level-{next}\", {{}});\n    return {{ reached: `{level}>${{next.reached}}` }};\n  }} catch (rejected) {{\n    return {{ reached: `{level}x${{rejected.reason}}` }};\n  }}\n}}\n"
                ),
            )
        };
        let skill = root.join(format!("level-{level}"));
        crate::support::write_test_skill_package(
            &skill,
            &format!(
                "---\nname: level-{level}\ndescription: Calls the next level.\n---\n# Level {level}\n\nCall the next.\n"
            ),
            &format!(
                "minimum_runx: 0.12.0\nmethods:\n  prepare:\n    steps:\n      - id: prepare\n        script: {{module: prepare.mjs}}\n        calls:\n{calls}        max_calls: 1\n        outputs:\n          reached: {{type: string}}\n"
            ),
        )?;
        std::fs::write(skill.join("prepare.mjs"), script)?;
    }
    Ok(root.join("level-0"))
}

/// Run `skill_dir` under a delegation whose children nest at most `depth`
/// deep, or under none.
fn run_nested(root: &Path, skill_dir: PathBuf, depth: Option<u32>) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: depth.map(|depth| runx_runtime::RunAuthority {
                max_child_depth: Some(depth),
                ..runx_runtime::RunAuthority::default()
            }),
            act: None,
            skill_path: skill_dir,
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
    )?)
}

/// Depth through script calls is counted by the step that made each call:
/// a script's child is one deeper than its run, a child's child two, and the
/// delegation's `max_child_depth` refuses the call that would nest past it,
/// as a rejection its script catches, not a fault.
#[test]
fn nested_depth_through_script_calls_is_counted_and_bounded() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_levels(temp.path())?;
    let unbounded = run_nested(temp.path(), skill_dir.clone(), None)?;
    let output = object(&unbounded.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(
        text(object(output.get("result").ok_or("no result")?)?, "reached"),
        Some("0>1>2>3")
    );
    let run_id = text(output, "run_id").ok_or("missing run_id")?.to_owned();
    let read = runx_runtime::run_record::read_run(&env(temp.path()), temp.path(), &run_id, None)?;
    let runs = read
        .events
        .iter()
        .filter_map(|record| match &record.event {
            RunEventFact::Sealed { .. } => Some(record.stream.as_str().to_owned()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    // Each child run sits one step below the call that made it.
    assert_eq!(
        runs,
        [
            run_id.clone(),
            format!("{run_id}/prepare/0"),
            format!("{run_id}/prepare/0/prepare/0"),
            format!("{run_id}/prepare/0/prepare/0/prepare/0"),
        ]
        .into_iter()
        .collect()
    );

    let bounded = run_nested(temp.path(), skill_dir, Some(2))?;
    let output = object(&bounded.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(
        text(object(output.get("result").ok_or("no result")?)?, "reached"),
        Some("0>1>2xrefused"),
        "the call that would nest a third deep is refused: {output:?}"
    );
    Ok(())
}

/// Whether `stream` is run `run_id`'s own or one of its steps' calls
/// streams.
fn of_run(stream: &str, run_id: &str) -> bool {
    stream == run_id || runx_runtime::run_record::is_calls_stream_of(stream, run_id)
}

/// What each call at the root of `run_id` was answered of reuse, by index.
fn reuse_decisions(
    root: &Path,
    run_id: &str,
) -> TestResult<BTreeMap<u32, runx_contracts::ReuseDecision>> {
    let read = runx_runtime::run_record::read_run(&env(root), root, run_id, None)?;
    Ok(read
        .events
        .into_iter()
        .filter(|record| of_run(record.stream.as_str(), run_id))
        .filter_map(|record| match record.event {
            RunEventFact::Call {
                index,
                reuse: Some(reuse),
                ..
            } => Some((index, reuse.decision)),
            _ => None,
        })
        .collect())
}

fn recomputed(reason: runx_contracts::RecomputeReason) -> runx_contracts::ReuseDecision {
    runx_contracts::ReuseDecision::Recomputed { reason }
}

/// A package, `digest-child`, whose one method digests a value.
fn write_digest_child(root: &Path) -> TestResult {
    crate::support::write_test_skill_package(
        &root.join("digest-child"),
        "---\nname: digest-child\ndescription: Digests a value.\n---\n# Digest child\n\nDigest it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  digest:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {value: $input.value, encoding: utf8_text}\n",
    )?;
    Ok(())
}

/// A call that asks to reuse a result makes the child it runs the newest
/// source of what it asked, which a later call asking the same finds. A
/// plain call indexes nothing, so the call after it that asks finds no
/// candidate (reuse, D1).
#[test]
fn a_call_that_asks_indexes_its_child_and_a_plain_call_does_not() -> TestResult {
    use runx_contracts::RecomputeReason::{Ineligible, NoCandidate};
    let temp = tempdir()?;
    write_digest_child(temp.path())?;
    let skill_dir = write_one_step(
        temp.path(),
        "        inputs:\n          value: $input.value\n        calls:\n          skills: [../digest-child]\n        max_calls: 2\n        outputs:\n          done: {type: boolean}\n",
        r#"export default async function (ctx, { value }) {
  await ctx.skill("../digest-child", { value });
  await ctx.skill("../digest-child", { value }, { reuse: { max_age_seconds: 604800 } });
  return { done: true };
}
"#,
    )?;
    let first = run(temp.path(), skill_dir.clone())?;
    let first_id = text(object(&first.output)?, "run_id").ok_or("missing run_id")?;
    assert_eq!(
        reuse_decisions(temp.path(), first_id)?,
        BTreeMap::from([(1, recomputed(NoCandidate))])
    );
    // The child the asking call ran is the source a later asking call finds,
    // and recomputes until its eligibility is checked.
    let second = run(temp.path(), skill_dir)?;
    let second_id = text(object(&second.output)?, "run_id").ok_or("missing run_id")?;
    assert_eq!(
        reuse_decisions(temp.path(), second_id)?,
        BTreeMap::from([(1, recomputed(Ineligible))])
    );
    Ok(())
}

/// Only a child that completed is a source: one that failed leaves the
/// index as it was, so the call after it that asks still finds no candidate
/// rather than a failed run.
#[test]
fn a_child_that_failed_is_not_indexed() -> TestResult {
    use runx_contracts::RecomputeReason::NoCandidate;
    let temp = tempdir()?;
    let child = temp.path().join("fault-child");
    crate::support::write_test_skill_package(
        &child,
        "---\nname: fault-child\ndescription: Fails.\n---\n# Fault child\n\nFail.\n",
        "minimum_runx: 0.12.0\nmethods:\n  fail:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: fault\n        tool: {module: fault.mjs}\n        outputs: {done: {type: boolean}}\n",
    )?;
    std::fs::write(
        child.join("fault.mjs"),
        "export default function () { throw new Error(\"the child failed\"); }\n",
    )?;
    let skill_dir = write_one_step(
        temp.path(),
        "        inputs:\n          value: $input.value\n        calls:\n          skills: [../fault-child]\n        max_calls: 2\n        outputs:\n          done: {type: boolean}\n",
        r#"export default async function (ctx, { value }) {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      await ctx.skill("../fault-child", { value }, { reuse: { max_age_seconds: 604800 } });
    } catch {}
  }
  return { done: true };
}
"#,
    )?;
    let ran = run(temp.path(), skill_dir)?;
    let run_id = text(object(&ran.output)?, "run_id").ok_or("missing run_id")?;
    assert_eq!(
        reuse_decisions(temp.path(), run_id)?,
        BTreeMap::from([(0, recomputed(NoCandidate)), (1, recomputed(NoCandidate))])
    );
    Ok(())
}

/// A child that settles after a resume is indexed as one that ran straight
/// through is: its call's identity is on its record, so the driver that
/// settles it needn't be the one that started it.
#[test]
fn a_child_that_settles_after_a_resume_is_indexed() -> TestResult {
    use runx_contracts::RecomputeReason::{Ineligible, NoCandidate};
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        r#"export default async function (ctx, { value }) {
  const confirmed = await ctx.skill("../confirm-child", { value }, { reuse: { max_age_seconds: 604800 } });
  return { confirmed: Object.keys(confirmed) };
}
"#,
        "          confirmed: {type: array}\n",
    )?;
    let started = run(temp.path(), skill_dir.clone())?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let waiting = requests(&started)?;
    assert_eq!(waiting, ["prepare/0#confirm"]);
    let sealed = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {waiting[0].as_str(): true}}),
    )?;
    assert_eq!(text(object(&sealed.output)?, "status"), Some("sealed"));
    assert_eq!(
        reuse_decisions(temp.path(), &run_id)?,
        BTreeMap::from([(0, recomputed(NoCandidate))])
    );
    let again = run(temp.path(), skill_dir)?;
    let again_id = text(object(&again.output)?, "run_id").ok_or("missing run_id")?;
    assert_eq!(
        reuse_decisions(temp.path(), again_id)?,
        BTreeMap::from([(0, recomputed(Ineligible))])
    );
    Ok(())
}

/// Run `skill_dir` under runx's managed driver, whose turns only runx's
/// managed agent works, on the kernel's local effects.
fn run_managed(root: &Path, skill_dir: PathBuf) -> TestResult<RunResult> {
    run_managed_under(root, skill_dir, None)
}

/// [`run_managed`], under the host's `run_authority`.
fn run_managed_under(
    root: &Path,
    skill_dir: PathBuf,
    run_authority: Option<serde_json::Value>,
) -> TestResult<RunResult> {
    let orchestrator = LocalOrchestrator::with_effects_and_environment(
        runx_runtime::RuntimeEffectRegistry::local()?,
        env(root),
    );
    Ok(orchestrator.run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: run_authority.map(serde_json::from_value).transpose()?,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::from([("value".to_owned(), JsonValue::String("hello".to_owned()))]),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(2)?,
        },
        None,
        None,
        None,
    )?)
}

/// A source runx drove whose every step it saw whole, a native tool that
/// only reads, is eligible, and one that reads nothing this run may not is
/// allowed: a later call asking the same finds it and, until check 3
/// compares what each step was given, recomputes as changed. The same
/// source under a host's driver isn't eligible (reuse.md, D2).
#[test]
fn a_source_runx_drove_and_saw_whole_is_eligible() -> TestResult {
    use runx_contracts::RecomputeReason::{Changed, NoCandidate};
    let temp = tempdir()?;
    write_digest_child(temp.path())?;
    let skill_dir = write_one_step(
        temp.path(),
        "        inputs:\n          value: $input.value\n        calls:\n          skills: [../digest-child]\n        max_calls: 1\n        outputs:\n          done: {type: boolean}\n",
        r#"export default async function (ctx, { value }) {
  await ctx.skill("../digest-child", { value }, { reuse: { max_age_seconds: 604800 } });
  return { done: true };
}
"#,
    )?;
    for expected in [NoCandidate, Changed] {
        let ran = run_managed(temp.path(), skill_dir.clone())?;
        let run_id = text(object(&ran.output)?, "run_id").ok_or("missing run_id")?;
        assert_eq!(
            reuse_decisions(temp.path(), run_id)?,
            BTreeMap::from([(0, recomputed(expected))])
        );
    }
    Ok(())
}

/// A parent whose one script step asks to reuse `child`'s result.
fn write_reusing_parent(root: &Path, child: &str) -> TestResult<PathBuf> {
    write_one_step(
        root,
        &format!(
            "        inputs:\n          value: $input.value\n        calls:\n          skills: [../{child}]\n        max_calls: 1\n        outputs:\n          done: {{type: boolean}}\n"
        ),
        &format!(
            "export default async function (ctx, {{ value }}) {{\n  await ctx.skill(\"../{child}\", {{ value }}, {{ reuse: {{ max_age_seconds: 604800 }} }});\n  return {{ done: true }};\n}}\n"
        ),
    )
}

/// What the first call at the root of `ran` was answered of reuse.
fn first_decision(root: &Path, ran: &RunResult) -> TestResult<runx_contracts::ReuseDecision> {
    let run_id = text(object(&ran.output)?, "run_id").ok_or("missing run_id")?;
    Ok(reuse_decisions(root, run_id)?
        .remove(&0)
        .ok_or("the call was answered nothing of reuse")?)
}

/// A source's file read is checked as its tool checks a path before it
/// reads: under `reach: workspace` a secret file the source read under
/// `inputs` isn't allowed, and a plain file it read still is, though the
/// reach narrowed (reuse.md, check 2).
#[test]
fn a_file_read_this_runs_reach_refuses_is_not_allowed() -> TestResult {
    use runx_contracts::RecomputeReason::{Changed, NoCandidate, NotAllowed};
    let temp = tempdir()?;
    std::fs::write(temp.path().join("notes.txt"), "kept\n")?;
    std::fs::write(temp.path().join(".env"), "TOKEN=kept\n")?;
    for (child, path, narrowed) in [
        ("secret-child", ".env", NotAllowed),
        ("notes-child", "notes.txt", Changed),
    ] {
        crate::support::write_test_skill_package(
            &temp.path().join(child),
            &format!("---\nname: {child}\ndescription: Reads a file.\n---\n# Child\n\nRead it.\n"),
            &format!(
                "minimum_runx: 0.12.0\nmethods:\n  read:\n    inputs:\n      value: {{type: string}}\n    steps:\n      - id: read\n        tool: fs.read\n        inputs: {{path: {path}}}\n"
            ),
        )?;
        let skill_dir = write_reusing_parent(temp.path(), child)?;
        for (authority, expected) in [
            (None, NoCandidate),
            (Some(serde_json::json!({"reach": "workspace"})), narrowed),
        ] {
            let ran = run_managed_under(temp.path(), skill_dir.clone(), authority)?;
            assert_eq!(
                first_decision(temp.path(), &ran)?,
                recomputed(expected),
                "{child}"
            );
        }
    }
    Ok(())
}

/// A source's reads are checked under the authority its child would hold:
/// this run's, narrowed by the step that calls it. A script step narrowed to
/// `reach: workspace` can't reuse a secret file a source read when its step
/// wasn't narrowed (reuse.md, check 2).
#[test]
fn a_read_the_calling_steps_reach_refuses_is_not_allowed() -> TestResult {
    use runx_contracts::RecomputeReason::{NoCandidate, NotAllowed};
    let temp = tempdir()?;
    std::fs::write(temp.path().join(".env"), "TOKEN=kept\n")?;
    crate::support::write_test_skill_package(
        &temp.path().join("secret-child"),
        "---\nname: secret-child\ndescription: Reads a file.\n---\n# Child\n\nRead it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  read:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: read\n        tool: fs.read\n        inputs: {path: .env}\n",
    )?;
    let skill_dir = write_reusing_parent(temp.path(), "secret-child")?;
    let ran = run_managed(temp.path(), skill_dir.clone())?;
    assert_eq!(first_decision(temp.path(), &ran)?, recomputed(NoCandidate));
    let run_yaml = skill_dir.join("run.yaml");
    let narrowed = std::fs::read_to_string(&run_yaml)?.replace(
        "        script: {module: prepare.mjs}\n",
        "        script: {module: prepare.mjs}\n        reach: workspace\n",
    );
    std::fs::write(&run_yaml, narrowed)?;
    let ran = run_managed(temp.path(), skill_dir)?;
    assert_eq!(first_decision(temp.path(), &ran)?, recomputed(NotAllowed));
    Ok(())
}

/// A source whose root no longer exists isn't refused: authority didn't
/// change, what it read did, so it recomputes as changed (reuse.md, checks
/// 2 and 3).
#[test]
fn a_source_whose_root_is_gone_recomputes_as_changed() -> TestResult {
    use runx_contracts::RecomputeReason::{Changed, NoCandidate};
    let temp = tempdir()?;
    std::fs::create_dir_all(temp.path().join("sub"))?;
    std::fs::write(temp.path().join("sub/notes.txt"), "kept\n")?;
    crate::support::write_test_skill_package(
        &temp.path().join("sub-child"),
        "---\nname: sub-child\ndescription: Reads a file under sub.\n---\n# Sub child\n\nRead it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  read:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: read\n        tool: fs.read\n        inputs: {repo_root: sub, path: notes.txt}\n",
    )?;
    let skill_dir = write_reusing_parent(temp.path(), "sub-child")?;
    let ran = run_managed(temp.path(), skill_dir.clone())?;
    assert_eq!(first_decision(temp.path(), &ran)?, recomputed(NoCandidate));
    std::fs::remove_dir_all(temp.path().join("sub"))?;
    let ran = run_managed(temp.path(), skill_dir)?;
    assert_eq!(first_decision(temp.path(), &ran)?, recomputed(Changed));
    Ok(())
}

/// A source's operation read is authorized again under this run's authority
/// and the chain its child would run under: a delegation that leaves the
/// store's event read out doesn't allow it, and without one it is
/// (reuse.md, check 2).
#[test]
fn an_operation_read_this_runs_authority_leaves_out_is_not_allowed() -> TestResult {
    use runx_contracts::RecomputeReason::{Changed, NoCandidate, NotAllowed};
    let temp = tempdir()?;
    crate::support::write_test_skill_package(
        &temp.path().join("store-child"),
        "---\nname: store-child\ndescription: Reads the notes.\n---\n# Store child\n\nRead them.\n",
        "minimum_runx: 0.12.0\nmethods:\n  read:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: events\n        operation: store/events.read\n        inputs: {stream: notes, key: k}\n",
    )?;
    let skill_dir = write_reusing_parent(temp.path(), "store-child")?;
    let inspect_only = || {
        Some(
            serde_json::json!({"delegations": [{"connector": "store", "terms": [{
                "verbs": ["read"],
                "bounds": {
                    "operations": ["store/key.inspect"],
                    "scopes": ["store.read"],
                    "targets": ["project:store-child/notes"],
                    "packages": ["*/*", "project:*"],
                },
            }]}]}),
        )
    };
    for (authority, expected) in [
        (None, NoCandidate),
        (inspect_only(), NotAllowed),
        (None, Changed),
    ] {
        let ran = run_managed_under(temp.path(), skill_dir.clone(), authority)?;
        assert_eq!(first_decision(temp.path(), &ran)?, recomputed(expected));
    }
    Ok(())
}

/// A source that asked a person isn't eligible, even under runx's managed
/// driver: a person's answer is something runx can't see again.
#[test]
fn a_source_that_asked_a_person_is_ineligible() -> TestResult {
    use runx_contracts::RecomputeReason::{Ineligible, NoCandidate};
    let temp = tempdir()?;
    let skill_dir = write_script_skill(
        temp.path(),
        r#"export default async function (ctx, { value }) {
  const confirmed = await ctx.skill("../confirm-child", { value }, { reuse: { max_age_seconds: 604800 } });
  return { confirmed: Object.keys(confirmed) };
}
"#,
        "          confirmed: {type: array}\n",
    )?;
    let started = run_managed(temp.path(), skill_dir.clone())?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let waiting = requests(&started)?;
    let sealed = resume(
        temp.path(),
        &run_id,
        serde_json::json!({"approvals": {waiting[0].as_str(): true}}),
    )?;
    assert_eq!(text(object(&sealed.output)?, "status"), Some("sealed"));
    assert_eq!(
        reuse_decisions(temp.path(), &run_id)?,
        BTreeMap::from([(0, recomputed(NoCandidate))])
    );
    let again = run_managed(temp.path(), skill_dir)?;
    let again_id = text(object(&again.output)?, "run_id").ok_or("missing run_id")?;
    assert_eq!(
        reuse_decisions(temp.path(), again_id)?,
        BTreeMap::from([(0, recomputed(Ineligible))])
    );
    Ok(())
}

/// A source, `store-child`, whose method's one step is `step`, beside
/// `module` when its step is a script, asked twice under runx's managed
/// driver: what the second ask was answered.
fn second_ask_of(step: &str, module: Option<&str>) -> TestResult<runx_contracts::ReuseDecision> {
    let temp = tempdir()?;
    crate::support::write_test_skill_package(
        &temp.path().join("store-child"),
        "---\nname: store-child\ndescription: Uses its own store.\n---\n# Store child\n\nUse it.\n",
        &format!(
            "minimum_runx: 0.12.0\nmethods:\n  use:\n    inputs:\n      value: {{type: string}}\n    steps:\n      - id: use\n{step}"
        ),
    )?;
    if let Some(module) = module {
        std::fs::write(temp.path().join("store-child/use.mjs"), module)?;
    }
    let skill_dir = write_one_step(
        temp.path(),
        "        inputs:\n          value: $input.value\n        calls:\n          skills: [../store-child]\n        max_calls: 1\n        outputs:\n          done: {type: boolean}\n          why: {type: string, optional: true}\n",
        r#"export default async function (ctx, { value }) {
  try {
    await ctx.skill("../store-child", { value }, { reuse: { max_age_seconds: 604800 } });
    return { done: true };
  } catch (rejected) {
    return { done: false, why: rejected.message };
  }
}
"#,
    )?;
    let mut decisions = Vec::new();
    for _ in 0..2 {
        let ran = run_managed(temp.path(), skill_dir.clone())?;
        let run_id = text(object(&ran.output)?, "run_id").ok_or("missing run_id")?;
        // The source's call resolved, so what it's answered turns on what
        // its operation did, never on a failure.
        let result = object(object(&ran.output)?.get("result").ok_or("no result")?)?;
        assert_eq!(
            result.get("done"),
            Some(&JsonValue::Bool(true)),
            "{result:?}"
        );
        decisions.extend(reuse_decisions(temp.path(), run_id)?.into_values());
    }
    assert_eq!(
        decisions[0],
        recomputed(runx_contracts::RecomputeReason::NoCandidate)
    );
    Ok(decisions[1].clone())
}

/// A read runx makes itself is eligible: the source whose script read its
/// own store is, and its read is authorized for this run, so it recomputes
/// as changed until check 3 compares what each step was given. A source
/// whose step wrote anything isn't eligible (reuse.md, checks 1 and 2).
#[test]
fn a_source_that_read_is_eligible_and_one_that_wrote_is_not() -> TestResult {
    use runx_contracts::RecomputeReason::{Changed, Ineligible};
    assert_eq!(
        second_ask_of(
            "        script: {module: use.mjs}\n        inputs: {value: $input.value}\n        calls:\n          operations: [store/events.read]\n        max_calls: 1\n        outputs:\n          done: {type: boolean}\n",
            Some(
                "export default async function (ctx, { value }) {\n  await ctx.operation(\"store/events.read\", { stream: \"notes\", key: value, limit: 1 });\n  return { done: true };\n}\n"
            ),
        )?,
        recomputed(Changed)
    );
    assert_eq!(
        second_ask_of(
            "        operation: store/events.append\n        idempotency_key: $input.value\n        inputs: {stream: notes, key: $input.value, expected_version: 0, type: note.added, data: {}}\n",
            None,
        )?,
        recomputed(Ineligible)
    );
    Ok(())
}
