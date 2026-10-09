// Module rationale: native dev skill/graph replay keeps
// fixture preparation, expectation projection, and harness invocation
// together.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runx_contracts::{ClosureDisposition, JsonNumber, JsonObject, JsonValue};
use runx_parser::DevFixtureTargetKind;

use super::r#loop::{
    assert_fixture_expectation, failed_fixture, prepare_fixture_workspace,
    resolve_fixture_execution_roots,
};
use super::materialize::materialize_fixture_value;
use super::support::elapsed_ms;
use super::types::{
    DevError, DevFixtureAssertion, DevFixtureAssertionKind, DevFixtureResult, DevFixtureStatus,
    LoadedDevFixture, PreparedDevFixtureWorkspace,
};
use crate::adapters::process_tool::ProcessToolAdapter;
use crate::harness::{HarnessExpectedStatus, HarnessReplayError, HarnessReplayOutput};
use crate::{HarnessOptions, run_harness_fixture_with_adapter};

pub(super) fn run_skill_or_graph_fixture(
    root: &Path,
    fixture: &LoadedDevFixture,
    env: &BTreeMap<String, String>,
) -> Result<DevFixtureResult, DevError> {
    let started = Instant::now();
    let kind = fixture.target().kind;
    let reference = fixture.target().reference.as_str();
    let Some(target_path) = resolve_target_path(root, kind, reference) else {
        return Ok(unknown_target_ref(fixture, started, kind, reference));
    };
    let workspace =
        prepare_fixture_workspace(root, &fixture.path, fixture.definition.workspace.as_ref())?;
    let result = run_skill_or_graph_fixture_inner(
        root,
        fixture,
        kind,
        &target_path,
        &workspace,
        env,
        started,
    );
    if let Some(workspace_root) = &workspace.root {
        let _ = fs::remove_dir_all(workspace_root);
    }
    result
}

fn run_skill_or_graph_fixture_inner(
    root: &Path,
    fixture: &LoadedDevFixture,
    kind: DevFixtureTargetKind,
    target_path: &Path,
    workspace: &PreparedDevFixtureWorkspace,
    env: &BTreeMap<String, String>,
    started: Instant,
) -> Result<DevFixtureResult, DevError> {
    let Some(execution_roots) =
        resolve_fixture_execution_roots(root, fixture.lane(), workspace.root.as_deref())
    else {
        return Ok(missing_execution_roots(fixture, started));
    };
    let harness_fixture_path =
        write_harness_replay_fixture(fixture, kind, target_path, workspace, &execution_roots)?;
    let output = run_dev_harness_fixture(&harness_fixture_path, env);
    let _ = fs::remove_file(&harness_fixture_path);
    if let Some(parent) = harness_fixture_path.parent() {
        let _ = fs::remove_dir(parent);
    }
    match output {
        Ok(output) => Ok(result_from_harness_output(fixture, started, output)),
        Err(error) => Ok(failed_fixture(
            fixture,
            started,
            vec![DevFixtureAssertion {
                path: "target.ref".to_owned(),
                expected: Some(JsonValue::String("native harness replay".to_owned())),
                actual: Some(JsonValue::String(error.to_string())),
                kind: DevFixtureAssertionKind::ExactMismatch,
                message: "Native skill or graph dev fixture execution failed.".to_owned(),
            }],
        )),
    }
}

fn run_dev_harness_fixture(
    path: &Path,
    env: &BTreeMap<String, String>,
) -> Result<HarnessReplayOutput, HarnessReplayError> {
    let options = HarnessOptions::from_env(env.clone()).map_err(HarnessReplayError::Runtime)?;
    run_harness_fixture_with_adapter(path, ProcessToolAdapter, options)
}

fn write_harness_replay_fixture(
    fixture: &LoadedDevFixture,
    kind: DevFixtureTargetKind,
    target_path: &Path,
    workspace: &PreparedDevFixtureWorkspace,
    roots: &super::types::DevFixtureExecutionRoots,
) -> Result<PathBuf, DevError> {
    let mut harness = JsonObject::new();
    harness.insert(
        "name".to_owned(),
        JsonValue::String(fixture.name().to_owned()),
    );
    harness.insert(
        "kind".to_owned(),
        JsonValue::String(kind.as_str().to_owned()),
    );
    harness.insert(
        "target".to_owned(),
        JsonValue::String(target_path.to_string_lossy().into_owned()),
    );
    harness.insert(
        "inputs".to_owned(),
        materialize_fixture_value(
            JsonValue::Object(fixture.definition.inputs.clone()),
            &workspace.tokens,
        ),
    );
    let env = fixture_env(fixture, workspace, roots);
    if !env.is_empty() {
        harness.insert("env".to_owned(), JsonValue::Object(env));
    }
    if fixture.definition.caller != runx_parser::HarnessCaller::default() {
        let caller = serde_json::to_value(&fixture.definition.caller)
            .and_then(serde_json::from_value::<JsonValue>)
            .map_err(|source| DevError::Json {
                path: fixture.path.clone(),
                source,
            })?;
        harness.insert("caller".to_owned(), caller);
    }
    let path = unique_harness_fixture_path()?;
    let contents = serde_json::to_string_pretty(&JsonValue::Object(harness)).map_err(|source| {
        DevError::Json {
            path: path.clone(),
            source,
        }
    })?;
    fs::write(&path, format!("{contents}\n")).map_err(|source| DevError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

fn fixture_env(
    fixture: &LoadedDevFixture,
    workspace: &PreparedDevFixtureWorkspace,
    roots: &super::types::DevFixtureExecutionRoots,
) -> JsonObject {
    let mut env = JsonObject::new();
    for (key, value) in materialized_string_map(&fixture.definition.env, &workspace.tokens) {
        env.insert(key, JsonValue::String(value));
    }
    env.insert(
        "RUNX_CWD".to_owned(),
        JsonValue::String(roots.cwd.to_string_lossy().into_owned()),
    );
    env.insert(
        "RUNX_REPO_ROOT".to_owned(),
        JsonValue::String(roots.repo_root.to_string_lossy().into_owned()),
    );
    if let Some(workspace_root) = &workspace.root {
        env.insert(
            "RUNX_FIXTURE_ROOT".to_owned(),
            JsonValue::String(workspace_root.to_string_lossy().into_owned()),
        );
    }
    env
}

fn materialized_string_map(
    object: &JsonObject,
    tokens: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    object
        .iter()
        .filter_map(|(key, value)| materialized_string_entry(key, value, tokens))
        .collect()
}

fn materialized_string_entry(
    key: &str,
    value: &JsonValue,
    tokens: &BTreeMap<String, String>,
) -> Option<(String, String)> {
    match materialize_fixture_value(value.clone(), tokens) {
        JsonValue::Null => None,
        JsonValue::String(value) => Some((key.to_owned(), value)),
        other => Some((
            key.to_owned(),
            serde_json::to_string(&other).unwrap_or_else(|_| "null".to_owned()),
        )),
    }
}

fn result_from_harness_output(
    fixture: &LoadedDevFixture,
    started: Instant,
    output: HarnessReplayOutput,
) -> DevFixtureResult {
    let fixture_output = dev_output_from_harness(&output);
    let exit_code = if output.status == HarnessExpectedStatus::Sealed {
        0
    } else {
        1
    };
    let assertions =
        assert_fixture_expectation(&fixture.definition.expect, exit_code, Some(&fixture_output));
    DevFixtureResult {
        name: fixture.name().to_owned(),
        lane: fixture.lane().as_str().to_owned(),
        target: fixture.target_json(),
        status: if assertions.is_empty() {
            DevFixtureStatus::Success
        } else {
            DevFixtureStatus::Failure
        },
        duration_ms: elapsed_ms(started),
        assertions,
        skip_reason: None,
        output: Some(fixture_output),
        replay_path: None,
    }
}

fn dev_output_from_harness(output: &HarnessReplayOutput) -> JsonValue {
    if let Some(skill_output) = &output.skill_output {
        return skill_output.value.clone();
    }
    let mut object = JsonObject::new();
    object.insert(
        "receipt_id".to_owned(),
        JsonValue::String(output.receipt.id.to_string()),
    );
    object.insert(
        "subject_ref".to_owned(),
        JsonValue::String(output.receipt.subject.reference.uri.clone().into_string()),
    );
    object.insert(
        "status".to_owned(),
        JsonValue::String(harness_status(&output.status).to_owned()),
    );
    object.insert(
        "disposition".to_owned(),
        JsonValue::String(disposition_name(&output.receipt.seal.disposition).to_owned()),
    );
    object.insert(
        "step_count".to_owned(),
        JsonValue::Number(JsonNumber::I64(
            i64::try_from(output.step_receipts.len()).unwrap_or(i64::MAX),
        )),
    );
    object.insert(
        "step_receipt_ids".to_owned(),
        JsonValue::Array(
            output
                .step_receipts
                .iter()
                .map(|receipt| JsonValue::String(receipt.id.to_string()))
                .collect(),
        ),
    );
    JsonValue::Object(object)
}

fn resolve_target_path(
    root: &Path,
    kind: DevFixtureTargetKind,
    reference: &str,
) -> Option<PathBuf> {
    match kind {
        DevFixtureTargetKind::Skill => resolve_skill_dir_from_ref(root, reference),
        DevFixtureTargetKind::Graph => resolve_graph_path_from_ref(root, reference),
        DevFixtureTargetKind::Tool => None,
    }
}

fn resolve_skill_dir_from_ref(root: &Path, reference: &str) -> Option<PathBuf> {
    let candidates = [root.join("skills").join(reference), root.join(reference)];
    candidates
        .into_iter()
        .find(|candidate| candidate.join("SKILL.md").exists())
        .and_then(|candidate| fs::canonicalize(candidate).ok())
}

fn resolve_graph_path_from_ref(root: &Path, reference: &str) -> Option<PathBuf> {
    let reference_path = Path::new(reference);
    let mut candidates = vec![root.join(reference_path)];
    if reference_path.extension().is_none() {
        candidates.push(root.join("graphs").join(format!("{reference}.yaml")));
        candidates.push(root.join("graphs").join(reference).join("graph.yaml"));
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| fs::canonicalize(candidate).ok())
}

fn unknown_target_ref(
    fixture: &LoadedDevFixture,
    started: Instant,
    kind: DevFixtureTargetKind,
    reference: &str,
) -> DevFixtureResult {
    failed_fixture(
        fixture,
        started,
        vec![DevFixtureAssertion {
            path: "target.ref".to_owned(),
            expected: Some(JsonValue::String(format!("existing {}", kind.as_str()))),
            actual: Some(JsonValue::String(reference.to_owned())),
            kind: DevFixtureAssertionKind::ExactMismatch,
            message: format!("{} {reference} was not found.", kind.as_str()),
        }],
    )
}

fn missing_execution_roots(fixture: &LoadedDevFixture, started: Instant) -> DevFixtureResult {
    failed_fixture(
        fixture,
        started,
        vec![DevFixtureAssertion {
            path: "repo".to_owned(),
            expected: Some(JsonValue::String("repo or workspace fixture".to_owned())),
            actual: Some(JsonValue::String("missing".to_owned())),
            kind: DevFixtureAssertionKind::ExactMismatch,
            message: "repo-integration fixtures must declare repo or workspace contents."
                .to_owned(),
        }],
    )
}

fn unique_harness_fixture_path() -> Result<PathBuf, DevError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let directory =
        std::env::temp_dir().join(format!("runx-dev-harness-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&directory).map_err(|source| DevError::Io {
        path: directory.clone(),
        source,
    })?;
    Ok(directory.join("fixture.yaml"))
}

fn harness_status(status: &HarnessExpectedStatus) -> &'static str {
    match status {
        HarnessExpectedStatus::Sealed => "sealed",
        HarnessExpectedStatus::Failure => "failure",
        HarnessExpectedStatus::NeedsAgent => "needs_agent",
        HarnessExpectedStatus::PolicyDenied => "policy_denied",
        HarnessExpectedStatus::Escalated => "escalated",
    }
}

fn disposition_name(disposition: &ClosureDisposition) -> &'static str {
    match disposition {
        ClosureDisposition::Closed => "closed",
        ClosureDisposition::Deferred => "deferred",
        ClosureDisposition::Superseded => "superseded",
        ClosureDisposition::Declined => "declined",
        ClosureDisposition::Blocked => "blocked",
        ClosureDisposition::Failed => "failed",
        ClosureDisposition::Killed => "killed",
        ClosureDisposition::TimedOut => "timed_out",
    }
}
