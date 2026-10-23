//! A run executes what it admitted, however its files change while it runs,
//! and a child run that stops short seals as a run of it would, its
//! receipts carried into its parent's.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{JsonValue, Receipt, RunEventFact, RunOutcome};
use runx_runtime::{LocalOrchestrator, RunResult, SkillRunRequest};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn env(root: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([("RUNX_CWD".to_owned(), root.display().to_string())]);
    crate::support::insert_test_signing_env(&mut env);
    env
}

fn run(root: &Path, skill: &Path, run_id: Option<&str>) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().run_skill(&SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill.to_path_buf(),
        receipt_dir: Some(root.join("receipts")),
        run_id: run_id.map(str::to_owned),
        answers: None,
        inputs: BTreeMap::new(),
        env: env(root),
        cwd: root.to_path_buf(),
        managed_agent: Default::default(),
    })?)
}

fn manual(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Checks what a run executes.\n---\n# {name}\n\nReturn the result.\n"
    )
}

fn module(answer: &str) -> String {
    format!("export default function () {{ return {{ answer: \"{answer}\" }}; }}\n")
}

/// A step that rewrites `path` to `contents`, as anything may while a run
/// is between its steps.
fn rewrite_step(path: &Path, contents: &str) -> TestResult<String> {
    let tool = serde_json::json!({
        "command": "sh",
        "args": [
            "-c",
            "cat >/dev/null; printf '%s' \"$0\" > \"$1\"; printf '{\"done\":true}'",
            contents,
            path.display().to_string(),
        ],
        "input_mode": "stdin",
    });
    Ok(format!(
        "      - id: rewrite\n        tool: {}\n        outputs: {{done: boolean}}\n",
        serde_json::to_string(&tool)?
    ))
}

fn answer(result: &RunResult) -> TestResult<String> {
    let output = result.output.as_object().ok_or("an output object")?;
    assert_eq!(
        output.get("status").and_then(JsonValue::as_str),
        Some("sealed"),
        "{output:?}"
    );
    Ok(output
        .get("result")
        .and_then(JsonValue::as_object)
        .and_then(|result| result.get("answer"))
        .and_then(JsonValue::as_str)
        .ok_or("an answer")?
        .to_owned())
}

#[test]
fn a_run_executes_the_modules_it_admitted_however_its_files_change() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let skill = root.join("source");
    let compute = skill.join("compute.mjs");
    crate::support::write_test_skill_package(
        &skill,
        &manual("source"),
        &format!(
            "skill: source\nrunners:\n  default:\n    default: true\n    result_from: [compute]\n    steps:\n{}      - id: compute\n        tool: {{module: compute.mjs}}\n        outputs: {{answer: string}}\n",
            rewrite_step(&compute, &module("changed"))?
        ),
    )?;
    fs::write(&compute, module("original"))?;

    let result = run(root, &skill, None)?;
    assert_eq!(
        fs::read_to_string(&compute)?,
        module("changed"),
        "the module changed between the run's steps"
    );
    assert_eq!(answer(&result)?, "original");
    Ok(())
}

#[test]
fn a_child_skill_runs_the_runner_its_parent_admitted() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let child = root.join("child");
    // Both runners are valid: one runs the module's default export, the
    // other its `other` export.
    let child_profile = |export: &str| {
        format!(
            "skill: child\nrunners:\n  default:\n    default: true\n    result_from: [answer]\n    steps:\n      - id: answer\n        tool: {{module: answer.mjs, export: {export}}}\n        outputs: {{answer: string}}\n"
        )
    };
    crate::support::write_test_skill_package(&child, &manual("child"), &child_profile("default"))?;
    fs::write(
        child.join("answer.mjs"),
        format!(
            "{}export function other() {{ return {{ answer: \"changed\" }}; }}\n",
            module("original")
        ),
    )?;
    let parent = root.join("parent");
    crate::support::write_test_skill_package(
        &parent,
        &manual("parent"),
        &format!(
            "skill: parent\nrunners:\n  default:\n    default: true\n    result_from: [child]\n    steps:\n{}      - id: child\n        skill: ../child\n",
            rewrite_step(&child.join("run.yaml"), &child_profile("other"))?
        ),
    )?;

    let result = run(root, &parent, None)?;
    assert_eq!(
        fs::read_to_string(child.join("run.yaml"))?,
        child_profile("other"),
        "the child's runner changed between the parent's steps"
    );
    assert_eq!(answer(&result)?, "original");
    Ok(())
}

fn receipt(root: &Path, id: &str) -> TestResult<Receipt> {
    crate::support::read_test_signed_receipt(&root.join("receipts"), id)
}

/// The receipts `receipt` names as its children.
fn children(root: &Path, parent: &Receipt) -> TestResult<Vec<Receipt>> {
    parent
        .lineage
        .children
        .iter()
        .map(|child| {
            let id = child
                .uri
                .strip_prefix("runx:receipt:")
                .ok_or("a receipt uri")?;
            receipt(root, id)
        })
        .collect()
}

#[test]
fn a_child_that_fails_after_its_work_seals_and_its_parent_keeps_every_receipt() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let child = root.join("child");
    crate::support::write_test_skill_package(
        &child,
        &manual("child"),
        "skill: child\nrunners:\n  default:\n    default: true\n    steps:\n      - id: first\n        tool: {module: first.mjs}\n        outputs: {done: boolean}\n      - id: fault\n        tool: {module: fault.mjs}\n        outputs: {done: boolean}\n",
    )?;
    fs::write(
        child.join("first.mjs"),
        "export default function () { return { done: true }; }\n",
    )?;
    fs::write(
        child.join("fault.mjs"),
        "export default function () { throw new Error(\"child failed after work\"); }\n",
    )?;
    let parent = root.join("parent");
    crate::support::write_test_skill_package(
        &parent,
        &manual("parent"),
        "skill: parent\nrunners:\n  default:\n    default: true\n    steps:\n      - id: invoke\n        skill: ../child\n",
    )?;

    let result = run(root, &parent, Some("nested-failure"))?;
    let output = result.output.as_object().ok_or("an output object")?;
    assert_eq!(
        output.get("outcome").and_then(JsonValue::as_str),
        Some("failed"),
        "{output:?}"
    );

    // The child run is sealed, as a run of it would be.
    let read = runx_runtime::run_record::read_run(&env(root), root, "nested-failure", None)?;
    let child_receipt = read
        .events
        .iter()
        .find_map(|record| match &record.event {
            RunEventFact::Sealed {
                outcome: RunOutcome::Failed,
                receipt_id,
            } if record.stream.as_str() == "nested-failure/invoke" => Some(receipt_id.to_string()),
            _ => None,
        })
        .ok_or("the child run is sealed failed")?;

    // The parent's step closes over the child's receipt, which names both
    // of the child's steps.
    let graph = receipt(
        root,
        output
            .get("receipt_id")
            .and_then(JsonValue::as_str)
            .ok_or("a receipt id")?,
    )?;
    let [invoke] = children(root, &graph)?
        .try_into()
        .map_err(|_| "one parent step")?;
    assert_eq!(invoke.seal.outcome, RunOutcome::Failed);
    let [child_graph] = children(root, &invoke)?
        .try_into()
        .map_err(|_| "the child's run receipt")?;
    assert_eq!(child_graph.id.as_str(), child_receipt);
    let steps = children(root, &child_graph)?;
    assert_eq!(
        steps
            .iter()
            .map(|step| step.seal.outcome)
            .collect::<Vec<_>>(),
        [RunOutcome::Completed, RunOutcome::Failed],
        "the child's work and its failure are both in the tree"
    );
    Ok(())
}

#[test]
fn a_child_a_guard_refuses_stops_its_parent_with_the_refusal() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let child = root.join("child");
    crate::support::write_test_skill_package(
        &child,
        &manual("child"),
        "skill: child\nrunners:\n  default:\n    default: true\n    steps:\n      - id: first\n        tool: {module: first.mjs}\n        outputs: {done: boolean}\n      - id: guarded\n        tool: {module: first.mjs}\n        outputs: {done: boolean}\n    policy:\n      guards:\n        - step: guarded\n          value: $steps.first.done\n          equals: false\n",
    )?;
    fs::write(
        child.join("first.mjs"),
        "export default function () { return { done: true }; }\n",
    )?;
    let parent = root.join("parent");
    crate::support::write_test_skill_package(
        &parent,
        &manual("parent"),
        "skill: parent\nrunners:\n  default:\n    default: true\n    steps:\n      - id: invoke\n        skill: ../child\n",
    )?;

    let result = run(root, &parent, None)?;
    let output = result.output.as_object().ok_or("an output object")?;
    assert_eq!(
        output.get("outcome").and_then(JsonValue::as_str),
        Some("blocked"),
        "{output:?}"
    );
    assert_eq!(
        output.get("seal_reason").and_then(JsonValue::as_str),
        Some("graph_blocked"),
        "{output:?}"
    );
    // Why the child's guard refused rides its parent's output, as it would
    // the child's own.
    let refusal = output
        .get("refusal")
        .and_then(JsonValue::as_object)
        .ok_or("the parent returns its child's refusal")?;
    assert!(
        refusal
            .get("message")
            .and_then(JsonValue::as_str)
            .is_some_and(|message| message.contains("guarded")),
        "{refusal:?}"
    );
    Ok(())
}

#[test]
fn a_process_step_runs_only_while_its_package_is_the_admitted_one() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let skill = root.join("process");
    let second = skill.join("second.sh");
    let ran = root.join("ran.txt");
    let process = |script: &str| {
        serde_json::json!({"command": "sh", "args": [script], "input_mode": "stdin"}).to_string()
    };
    crate::support::write_test_skill_package(
        &skill,
        &manual("process"),
        &format!(
            "skill: process\nrunners:\n  default:\n    default: true\n    steps:\n{}      - id: second\n        tool: {}\n        outputs: {{done: boolean}}\n",
            rewrite_step(
                &second,
                &format!(
                    "cat >/dev/null; echo changed > '{}'; printf '{{\"done\":true}}'",
                    ran.display()
                )
            )?,
            process(&second.display().to_string()),
        ),
    )?;
    fs::write(&second, "cat >/dev/null; printf '{\"done\":true}'\n")?;

    let result = run(root, &skill, None)?;
    let output = result.output.as_object().ok_or("an output object")?;
    assert_eq!(
        output.get("outcome").and_then(JsonValue::as_str),
        Some("failed"),
        "{output:?}"
    );
    assert!(
        serde_json::to_string(output)?.contains("changed since the run admitted it"),
        "{output:?}"
    );
    assert!(!ran.exists(), "the changed script never ran");
    Ok(())
}
