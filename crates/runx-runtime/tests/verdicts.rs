//! A verdict names who judged, and runx names it: whatever kind of step
//! returns a `runx.verdict.v1`, its evaluator is the step's package, method
//! and run, and a verdict that names another evaluator fails its step.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_runtime::{LocalOrchestrator, SkillRunRequest};
use serde_json::{Value, json};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A package `judge` whose `judge` method is one worker tool step returning
/// `verdict` as its verdict output.
fn judge(root: &Path, verdict: &Value) -> TestResult<std::path::PathBuf> {
    let skill = root.join("judge");
    crate::support::write_test_skill_package(
        &skill,
        "---\nname: judge\ndescription: Judges a fixed subject.\n---\n# judge\n\nJudge it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  judge:\n    steps:\n      - id: judge\n        tool: {module: judge.mjs}\n        outputs:\n          verdict: {packet: runx.verdict.v1}\n",
    )?;
    fs::write(
        skill.join("judge.mjs"),
        format!("export default function () {{ return {{ verdict: {verdict} }}; }}\n"),
    )?;
    Ok(skill)
}

fn run(root: &Path, skill: &Path) -> TestResult<Value> {
    let mut env = BTreeMap::from([("RUNX_CWD".to_owned(), root.display().to_string())]);
    crate::support::insert_test_signing_env(&mut env);
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill.to_path_buf(),
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: root.to_path_buf(),
            managed_agent: Default::default(),
        },
        None,
        None,
        None,
    )?;
    Ok(serde_json::to_value(&result.output)?)
}

fn verdict() -> Value {
    json!({
        "schema": "runx.verdict.v1",
        "verdict": "met",
        "subject": {"ref": "subject", "revision": "sha256:subject"},
        "criteria_set": {"ref": "checks", "revision": "sha256:checks"},
        "assessments": [{"verdict": "met", "id": "done", "evidence": []}],
    })
}

/// A worker tool step's verdict that names no evaluator is named the step's
/// own: its package, its method and its run.
#[test]
fn a_tool_steps_verdict_is_named_the_steps_own() -> TestResult {
    let temp = tempdir()?;
    let skill = judge(temp.path(), &verdict())?;

    let output = run(temp.path(), &skill)?;

    assert_eq!(output["status"], "sealed", "{output}");
    let evaluator = &output["result"]["verdict"]["evaluator"];
    assert_eq!(
        evaluator,
        &json!({
            "package": "project:judge",
            "method": "judge",
            "run": output["run_id"],
        }),
        "{output}"
    );
    Ok(())
}

/// A worker tool step's verdict that names another evaluator fails its step,
/// naming the evaluator it claimed.
#[test]
fn a_tool_steps_verdict_naming_another_evaluator_fails_its_step() -> TestResult {
    let temp = tempdir()?;
    let mut claimed = verdict();
    claimed["evaluator"] = json!({"package": "runx/other", "method": "judge", "run": "run_other"});
    let skill = judge(temp.path(), &claimed)?;

    let output = run(temp.path(), &skill)?;

    assert_eq!(output["outcome"], "failed", "{output}");
    assert!(
        output["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("runx/other")),
        "{output}"
    );
    Ok(())
}
