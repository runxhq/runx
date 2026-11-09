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
