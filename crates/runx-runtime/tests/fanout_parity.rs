#![cfg(feature = "cli-tool")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use runx_contracts::{
    FanoutReceiptDecision, FanoutReceiptStrategy, FanoutReceiptSyncPoint, JsonObject, JsonValue,
};
use runx_core::state_machine::{GraphStatus, GraphStepStatus};
use runx_parser::SkillSource;
use runx_receipts::validate_receipt_tree;
use runx_runtime::{
    InvocationOutput, RUNX_CWD_ENV, RUNX_MAX_FANOUT_CONCURRENCY_ENV, Runtime, RuntimeError,
    RuntimeOptions, SkillAdapter, SkillInvocation,
};
use serde::Deserialize;

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FanoutFixture {
    all_success: ExpectedRun,
    quorum_continue: ExpectedRun,
    threshold_pause: ExpectedPause,
    generated: GeneratedFixture,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedFixture {
    partial_failure: ExpectedRun,
    retry: ExpectedRetry,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedRun {
    graph: String,
    graph_path: Option<String>,
    status: String,
    steps: Vec<ExpectedStep>,
    sync_points: Vec<FanoutReceiptSyncPoint>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedStep {
    id: String,
    status: String,
    attempt: Option<u32>,
    fanout_group: Option<String>,
    #[serde(default)]
    contract: JsonObject,
    failure: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedPause {
    graph: String,
    status: String,
    step_id: String,
    sync_point: FanoutReceiptSyncPoint,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedRetry {
    graph: String,
    graph_path: String,
    status: String,
    branch_count: usize,
    retry_step_id: String,
    retry_attempts: u32,
    checkpoint_steps: Vec<ExpectedStep>,
    sync_point: FanoutReceiptSyncPoint,
}

#[test]
fn fanout_all_success_runs_group_then_synthesizes() -> Result<(), Box<dyn std::error::Error>> {
    let expected = fixture()?.all_success;
    let run = run_fixture_graph_file(Path::new("../../fixtures/graphs/fanout/all.yaml"))?;

    assert_eq!(run.graph.name, expected.graph);
    assert_eq!(graph_status(&run.state.status), expected.status);
    assert_steps(&run, &expected.steps);
    assert_step_state(&run, "market", GraphStepStatus::Succeeded)?;
    assert_step_state(&run, "risk", GraphStepStatus::Succeeded)?;
    assert_step_state(&run, "finance", GraphStepStatus::Succeeded)?;
    assert_output(
        &run,
        "finance",
        "result.data.budget",
        JsonValue::String("approved".to_owned()),
    )?;
    assert_sync_points(&run, &expected.sync_points);
    assert_receipt_tree(&run);
    Ok(())
}

#[test]
fn fanout_parallel_cli_tool_mode_preserves_plan_order() -> Result<(), Box<dyn std::error::Error>> {
    let expected = fixture()?.all_success;
    let mut options = fixture_runtime_options();
    options
        .env
        .insert(RUNX_MAX_FANOUT_CONCURRENCY_ENV.to_owned(), "4".to_owned());
    let run = Runtime::new(runx_runtime::adapters::cli_tool::CliToolAdapter, options)
        .run_graph_file(Path::new("../../fixtures/graphs/fanout/all.yaml"))?;

    assert_eq!(run.graph.name, expected.graph);
    assert_eq!(graph_status(&run.state.status), expected.status);
    assert_steps(&run, &expected.steps);
    assert_sync_points(&run, &expected.sync_points);
    assert_receipt_tree(&run);
    Ok(())
}

#[test]
fn fanout_scheduler_keeps_slots_saturated_across_uneven_jobs()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    write_saturation_fixture(directory.path(), 8)?;
    let state = SaturationState::default();
    let adapter = SaturationAdapter {
        state: state.clone(),
    };
    let mut options = fixture_runtime_options();
    options
        .env
        .insert(RUNX_MAX_FANOUT_CONCURRENCY_ENV.to_owned(), "4".to_owned());

    let run =
        Runtime::new(adapter, options).run_graph_file(&directory.path().join("graph.yaml"))?;

    assert_eq!(
        run.steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>(),
        (0..8)
            .map(|index| format!("branch_{index}"))
            .collect::<Vec<_>>()
    );
    assert!(
        state.opened_at.load(Ordering::Acquire) >= 5,
        "the slow first branch opened before a later job reused an available slot"
    );
    Ok(())
}

#[test]
fn fanout_quorum_continue_tolerates_failed_branch() -> Result<(), Box<dyn std::error::Error>> {
    let expected = fixture()?.quorum_continue;
    let run = run_fixture_graph_file(Path::new("../../fixtures/graphs/fanout/graph.yaml"))?;

    assert_eq!(run.graph.name, expected.graph);
    assert_eq!(graph_status(&run.state.status), expected.status);
    assert_steps(&run, &expected.steps);
    assert_step_state(&run, "market", GraphStepStatus::Succeeded)?;
    assert_step_state(&run, "risk", GraphStepStatus::Succeeded)?;
    assert_step_state(&run, "finance", GraphStepStatus::Failed)?;
    assert_step_state(&run, "synthesize", GraphStepStatus::Succeeded)?;
    assert_sync_points(&run, &expected.sync_points);
    assert_receipt_tree(&run);
    Ok(())
}

#[test]
fn fanout_threshold_pause_blocks_followup() -> Result<(), Box<dyn std::error::Error>> {
    let expected = fixture()?.threshold_pause;
    let error =
        match run_fixture_graph_file(Path::new("../../fixtures/graphs/fanout/threshold.yaml")) {
            Ok(run) => return Err(format!("threshold fanout should pause: {run:#?}").into()),
            Err(error) => error,
        };

    let RuntimeError::GraphPaused {
        step_id,
        reason,
        sync_decision,
    } = error
    else {
        return Err(format!("expected GraphPaused, got {error:?}").into());
    };

    assert_eq!(expected.graph, "fanout-threshold");
    assert_eq!(expected.status, "paused");
    assert_eq!(step_id, expected.step_id);
    assert_eq!(reason, expected.sync_point.reason);
    assert_eq!(
        sync_point_without_receipts(&sync_decision),
        expected_without_receipts(&expected.sync_point)
    );

    let runtime = Runtime::new(
        runx_runtime::adapters::cli_tool::CliToolAdapter,
        fixture_runtime_options(),
    );
    let checkpoint = runtime
        .run_graph_file_until_steps(Path::new("../../fixtures/graphs/fanout/threshold.yaml"), 2)?;
    let checkpoint_ids = checkpoint
        .steps
        .iter()
        .map(|step| step.receipt.id.clone())
        .collect::<Vec<_>>();
    // Branch receipt ids are content-addressed; assert count + content address.
    assert_eq!(
        checkpoint_ids.len(),
        expected.sync_point.branch_receipts.len()
    );
    assert!(checkpoint_ids.iter().all(|id| id.starts_with("sha256:")));
    Ok(())
}

#[test]
fn generated_n_branch_partial_failure_uses_sync_point_oracle()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = fixture()?.generated.partial_failure;
    let graph_path = expected
        .graph_path
        .as_deref()
        .ok_or("generated partial-failure fixture is missing graphPath")?;
    let run = run_fixture_graph_file(Path::new(graph_path))?;

    assert_eq!(run.graph.name, expected.graph);
    assert_eq!(graph_status(&run.state.status), expected.status);
    assert_steps(&run, &expected.steps);
    assert_sync_points(&run, &expected.sync_points);
    assert_receipt_tree(&run);
    Ok(())
}

#[test]
fn generated_retry_records_attempts_before_halt() -> Result<(), Box<dyn std::error::Error>> {
    let expected = fixture()?.generated.retry;
    let runtime = Runtime::new(
        runx_runtime::adapters::cli_tool::CliToolAdapter,
        fixture_runtime_options(),
    );
    let checkpoint = runtime.run_graph_file_until_steps(
        Path::new(&expected.graph_path),
        expected.checkpoint_steps.len(),
    )?;

    assert_eq!(checkpoint.graph_name, expected.graph);
    assert_steps_in_checkpoint(&checkpoint, &expected.checkpoint_steps);
    assert_eq!(
        checkpoint
            .state
            .steps
            .iter()
            .find(|step| step.step_id == expected.retry_step_id)
            .map(|step| step.attempts),
        Some(expected.retry_attempts)
    );

    let error = match run_fixture_graph_file(Path::new(&expected.graph_path)) {
        Ok(_) => return Err("retry fanout should halt after exhausting retry budget".into()),
        Err(error) => error,
    };
    let RuntimeError::GraphPlanningFailed { reason, .. } = error else {
        return Err(format!("expected GraphPlanningFailed, got {error:?}").into());
    };
    assert_eq!(expected.status, "failed");
    assert_eq!(expected.branch_count, expected.sync_point.branch_count);
    assert_eq!(reason, expected.sync_point.reason);
    Ok(())
}

#[test]
fn fanout_runtime_error_branch_records_failure_and_continues()
-> Result<(), Box<dyn std::error::Error>> {
    let run = run_fixture_graph_file(Path::new(
        "../../fixtures/runtime/fanout/generated/fanout-generated-missing-skill.yaml",
    ))?;

    assert_eq!(run.graph.name, "fanout-generated-missing-skill");
    assert_eq!(run.state.status, GraphStatus::Succeeded);
    assert_step_state(&run, "market", GraphStepStatus::Succeeded)?;
    assert_step_state(&run, "missing", GraphStepStatus::Failed)?;
    assert_step_state(&run, "risk", GraphStepStatus::Succeeded)?;
    assert_step_state(&run, "synthesize", GraphStepStatus::Succeeded)?;
    let missing_step = run
        .steps
        .iter()
        .find(|step| step.step_id == "missing")
        .ok_or("missing fanout branch result")?;
    let failure = missing_step
        .outcome
        .failure_message()
        .ok_or("missing fanout branch failure diagnostic")?;
    assert!(
        failure.contains("runtime I/O failed while reading")
            && failure.contains("skills/not-found"),
        "unexpected missing-branch error: {failure}"
    );
    assert_eq!(run.sync_points.len(), 1);
    assert_eq!(run.sync_points[0].decision, FanoutReceiptDecision::Proceed);
    assert_eq!(run.sync_points[0].success_count, 2);
    assert_eq!(run.sync_points[0].failure_count, 1);
    assert_receipt_tree(&run);
    Ok(())
}

#[test]
fn fanout_successful_retry_feeds_downstream_with_latest_outputs()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let graph_path = write_retry_latest_wins_graph(temp.path())?;
    let run = run_fixture_graph_file(&graph_path)?;

    assert_eq!(run.graph.name, "fanout-retry-latest-wins");
    assert_eq!(run.state.status, GraphStatus::Succeeded);
    assert_step_state(&run, "flaky", GraphStepStatus::Succeeded)?;
    assert_step_state(&run, "downstream", GraphStepStatus::Succeeded)?;
    assert_eq!(
        run.steps
            .iter()
            .filter(|step| step.step_id == "flaky")
            .map(|step| (step.attempt, output_status(step)))
            .collect::<Vec<_>>(),
        vec![(1, "failure"), (2, "success")]
    );
    assert_output(
        &run,
        "downstream",
        "message.data",
        JsonValue::String("fresh".to_owned()),
    )?;
    assert_terminal_receipt_child(&run, "flaky", 2)?;
    assert_receipt_tree(&run);
    Ok(())
}

#[test]
fn sequential_successful_retry_feeds_downstream_with_latest_outputs()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let graph_path = write_sequential_retry_latest_wins_graph(temp.path())?;
    let run = run_fixture_graph_file(&graph_path)?;

    assert_eq!(run.graph.name, "sequential-retry-latest-wins");
    assert_eq!(run.state.status, GraphStatus::Succeeded);
    assert_step_state(&run, "flaky", GraphStepStatus::Succeeded)?;
    assert_step_state(&run, "downstream", GraphStepStatus::Succeeded)?;
    assert_eq!(
        run.steps
            .iter()
            .filter(|step| step.step_id == "flaky")
            .map(|step| (step.attempt, output_status(step)))
            .collect::<Vec<_>>(),
        vec![(1, "failure"), (2, "success")]
    );
    assert_output(
        &run,
        "downstream",
        "message.data",
        JsonValue::String("fresh".to_owned()),
    )?;
    assert_terminal_receipt_child(&run, "flaky", 2)?;
    Ok(())
}

#[derive(Clone, Default)]
struct SaturationState {
    started: Arc<(Mutex<usize>, Condvar)>,
    opened_at: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct SaturationAdapter {
    state: SaturationState,
}

impl SkillAdapter for SaturationAdapter {
    fn adapter_type(&self) -> &'static str {
        "fanout-saturation-test"
    }

    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        let branch = request
            .inputs
            .get("branch")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| RuntimeError::SkillFailed {
                skill_name: request.skill_name.clone(),
                message: "branch input is missing".to_owned(),
            })?;
        let (started, available) = &*self.state.started;
        let mut count = started.lock().map_err(|_| RuntimeError::SkillFailed {
            skill_name: request.skill_name.clone(),
            message: "saturation test state was poisoned".to_owned(),
        })?;
        *count += 1;
        available.notify_all();
        if branch == "0" {
            let deadline = Instant::now() + Duration::from_millis(500);
            while *count < 5 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(RuntimeError::SkillFailed {
                        skill_name: request.skill_name,
                        message: "fanout worker slots stopped at a chunk barrier".to_owned(),
                    });
                }
                let waited = available.wait_timeout(count, remaining).map_err(|_| {
                    RuntimeError::SkillFailed {
                        skill_name: request.skill_name.clone(),
                        message: "saturation test wait was poisoned".to_owned(),
                    }
                })?;
                count = waited.0;
            }
            self.state.opened_at.store(*count, Ordering::Release);
        }
        drop(count);
        Ok(InvocationOutput::runtime_success(
            JsonValue::Object(JsonObject::new()),
            0,
            JsonObject::new(),
        ))
    }

    fn isolated_fanout_adapter(
        &self,
        _source: &SkillSource,
    ) -> Option<Box<dyn SkillAdapter + Send + Sync>> {
        Some(Box::new(self.clone()))
    }
}

fn write_saturation_fixture(root: &Path, branches: usize) -> Result<(), std::io::Error> {
    let worker = root.join("worker");
    fs::create_dir_all(&worker)?;
    fs::write(
        worker.join("SKILL.md"),
        "---\nname: fanout-saturation-worker\ndescription: Exercise bounded fanout scheduling.\n---\n\n# Fanout saturation worker\n",
    )?;
    fs::write(
        worker.join("X.yaml"),
        "skill: fanout-saturation-worker\nrunners:\n  run:\n    default: true\n    type: cli-tool\n    command: never-executed\n    inputs:\n      branch:\n        type: string\n        required: true\n",
    )?;
    let steps = (0..branches)
        .map(|index| {
            format!(
                "  - id: branch_{index}\n    mode: fanout\n    fanout_group: workers\n    skill: ./worker\n    inputs:\n      branch: \"{index}\"\n"
            )
        })
        .collect::<String>();
    fs::write(
        root.join("graph.yaml"),
        format!(
            "name: fanout-saturation\nfanout:\n  groups:\n    workers:\n      strategy: all\n      on_branch_failure: halt\nsteps:\n{steps}"
        ),
    )
}

fn fixture() -> Result<FanoutFixture, serde_json::Error> {
    serde_json::from_str(include_str!(
        "../../../fixtures/runtime/fanout/expected.json"
    ))
}

fn run_fixture_graph_file(
    graph_path: &Path,
) -> Result<runx_runtime::GraphRun, runx_runtime::RuntimeError> {
    Runtime::new(
        runx_runtime::adapters::cli_tool::CliToolAdapter,
        fixture_runtime_options(),
    )
    .run_graph_file(graph_path)
}

fn fixture_runtime_options() -> RuntimeOptions {
    let mut env = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
    env.insert(
        RUNX_CWD_ENV.to_owned(),
        env!("CARGO_MANIFEST_DIR").to_owned(),
    );
    RuntimeOptions {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        created_at: FIXTURE_CREATED_AT.to_owned(),
        ..RuntimeOptions::local_development(env)
    }
}

fn write_retry_latest_wins_graph(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let flaky_dir = root.join("flaky-json");
    fs::create_dir_all(&flaky_dir)?;
    fs::write(
        flaky_dir.join("SKILL.md"),
        r#"---
name: flaky-json
description: Fail once, then emit structured JSON.
---

Fail once, then emit structured JSON.
"#,
    )?;
    fs::write(
        flaky_dir.join("X.yaml"),
        r#"skill: flaky-json
runners:
  default:
    default: true
    type: cli-tool
    command: sh
    args:
      - ./run.sh
    timeout_seconds: 10
    artifacts:
      named_emits:
        message: message
"#,
    )?;
    fs::write(
        flaky_dir.join("run.sh"),
        r#"#!/bin/sh
marker=.runx-flaky-seen
if [ ! -f "$marker" ]; then
  : > "$marker"
  printf '%s' 'transient failure' >&2
  exit 1
fi
printf '%s' '{"message":"fresh"}'
"#,
    )?;

    let echo_dir = root.join("echo");
    fs::create_dir_all(&echo_dir)?;
    fs::write(
        echo_dir.join("SKILL.md"),
        r#"---
name: echo
description: Echo a message.
---

Echo a message.
"#,
    )?;
    fs::write(
        echo_dir.join("X.yaml"),
        r#"skill: echo
runners:
  default:
    default: true
    type: cli-tool
    command: sh
    args:
      - ./run.sh
    timeout_seconds: 10
    inputs:
      message:
        type: string
        required: true
    artifacts:
      named_emits:
        message: message
"#,
    )?;
    fs::write(
        echo_dir.join("run.sh"),
        r#"#!/bin/sh
printf '{"message":"%s"}' "${RUNX_INPUT_MESSAGE:-}"
"#,
    )?;

    let graph_path = root.join("graph.yaml");
    fs::write(
        &graph_path,
        r#"name: fanout-retry-latest-wins
owner: runx
fanout:
  groups:
    retry_branch:
      strategy: all
      on_branch_failure: continue
steps:
  - id: flaky
    mode: fanout
    fanout_group: retry_branch
    skill: flaky-json
    retry:
      max_attempts: 2
      backoff_ms: 0
  - id: downstream
    skill: echo
    context:
      message: flaky.message.data
"#,
    )?;
    Ok(graph_path)
}

fn write_sequential_retry_latest_wins_graph(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let graph_path = write_retry_latest_wins_graph(root)?;
    fs::write(
        &graph_path,
        r#"name: sequential-retry-latest-wins
owner: runx
steps:
  - id: flaky
    skill: flaky-json
    retry:
      max_attempts: 2
      backoff_ms: 0
  - id: downstream
    skill: echo
    context:
      message: flaky.message.data
"#,
    )?;
    Ok(graph_path)
}

fn assert_steps(run: &runx_runtime::GraphRun, expected: &[ExpectedStep]) {
    assert_eq!(run.steps.len(), expected.len());
    for (actual, expected) in run.steps.iter().zip(expected) {
        assert_eq!(actual.step_id, expected.id);
        if let Some(attempt) = expected.attempt {
            assert_eq!(actual.attempt, attempt);
        }
        if let Some(fanout_group) = &expected.fanout_group {
            assert_eq!(actual.fanout_group.as_deref(), Some(fanout_group.as_str()));
        }
        assert_eq!(output_status(actual), expected.status);
        assert_eq!(actual.contract, expected.contract);
        assert_eq!(
            actual.outcome.failure_message().as_deref(),
            expected.failure.as_deref()
        );
    }
}

fn assert_steps_in_checkpoint(run: &runx_runtime::GraphCheckpoint, expected: &[ExpectedStep]) {
    assert_eq!(run.steps.len(), expected.len());
    for (actual, expected) in run.steps.iter().zip(expected) {
        assert_eq!(actual.step_id, expected.id);
        if let Some(attempt) = expected.attempt {
            assert_eq!(actual.attempt, attempt);
        }
        if let Some(fanout_group) = &expected.fanout_group {
            assert_eq!(actual.fanout_group.as_deref(), Some(fanout_group.as_str()));
        }
        assert_eq!(output_status(actual), expected.status);
        assert_eq!(actual.contract, expected.contract);
        assert_eq!(
            actual.outcome.failure_message().as_deref(),
            expected.failure.as_deref()
        );
    }
}

fn assert_step_state(
    run: &runx_runtime::GraphRun,
    step_id: &str,
    status: GraphStepStatus,
) -> Result<(), String> {
    let step = run
        .state
        .steps
        .iter()
        .find(|candidate| candidate.step_id == step_id)
        .ok_or_else(|| format!("missing step state {step_id}"))?;
    assert_eq!(step.status, status);
    Ok(())
}

fn assert_output(
    run: &runx_runtime::GraphRun,
    step_id: &str,
    key: &str,
    expected: JsonValue,
) -> Result<(), String> {
    let step = run
        .steps
        .iter()
        .find(|candidate| candidate.step_id == step_id)
        .ok_or_else(|| format!("missing step run {step_id}"))?;
    // Walk the contract path (for example `result.data.budget`) into the step's
    // addressable outputs.
    let mut value = step
        .contract
        .get(key.split('.').next().unwrap_or(key))
        .ok_or_else(|| format!("missing output {key} on step {step_id}"))?;
    for segment in key.split('.').skip(1) {
        value = value
            .as_object()
            .and_then(|object| object.get(segment))
            .ok_or_else(|| format!("missing output segment {segment} of {key} on {step_id}"))?;
    }
    assert_eq!(value, &expected);
    Ok(())
}

fn assert_receipt_tree(run: &runx_runtime::GraphRun) {
    let children = current_receipt_children(run);
    assert!(validate_receipt_tree(&run.receipt, &children).is_ok());
}

fn current_receipt_children(run: &runx_runtime::GraphRun) -> Vec<runx_contracts::Receipt> {
    let child_digests = run
        .receipt
        .lineage
        .as_ref()
        .map(|lineage| {
            lineage
                .children
                .iter()
                .filter_map(|reference| reference.locator.as_ref())
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    run.steps
        .iter()
        .filter(|step| {
            child_digests
                .iter()
                .any(|digest| digest == &step.receipt.digest)
        })
        .map(|step| step.receipt.clone())
        .collect::<Vec<_>>()
}

fn assert_terminal_receipt_child(
    run: &runx_runtime::GraphRun,
    step_id: &str,
    terminal_attempt: u32,
) -> Result<(), String> {
    let current_digests = current_receipt_children(run)
        .into_iter()
        .map(|receipt| receipt.digest)
        .collect::<Vec<_>>();
    for step in run.steps.iter().filter(|step| step.step_id == step_id) {
        let is_current = current_digests
            .iter()
            .any(|digest| digest == &step.receipt.digest);
        if step.attempt == terminal_attempt {
            if !is_current {
                return Err(format!(
                    "terminal attempt {step_id}#{} is missing from graph receipt children",
                    step.attempt
                ));
            }
        } else if is_current {
            return Err(format!(
                "superseded attempt {step_id}#{} must not be an active graph receipt child",
                step.attempt
            ));
        }
    }
    Ok(())
}

fn assert_sync_points(run: &runx_runtime::GraphRun, expected: &[FanoutReceiptSyncPoint]) {
    // `branch_receipts` are content-addressed ids assigned at seal time, so we
    // compare the structural sync points ignoring them, then assert the actual
    // branch receipts are content-addressed and the expected count.
    let strip = |points: &[FanoutReceiptSyncPoint]| {
        points
            .iter()
            .map(expected_without_receipts)
            .collect::<Vec<_>>()
    };
    assert_eq!(strip(&run.sync_points), strip(expected));
    let lineage_sync = run
        .receipt
        .lineage
        .as_ref()
        .map(|lineage| lineage.sync.clone())
        .unwrap_or_default();
    assert_eq!(strip(&lineage_sync), strip(expected));
    for (actual, expected_point) in run.sync_points.iter().zip(expected.iter()) {
        assert_eq!(
            actual.branch_receipts.len(),
            expected_point.branch_receipts.len()
        );
        assert!(
            actual
                .branch_receipts
                .iter()
                .all(|id| id.starts_with("sha256:")),
            "branch receipts must be content-addressed: {:?}",
            actual.branch_receipts
        );
    }
}

fn output_status(step: &runx_runtime::StepRun) -> &'static str {
    if step.outcome.succeeded() {
        "success"
    } else {
        "failure"
    }
}

fn graph_status(status: &GraphStatus) -> &'static str {
    match status {
        GraphStatus::Pending => "pending",
        GraphStatus::Running => "running",
        GraphStatus::Succeeded => "succeeded",
        GraphStatus::Failed => "failed",
        GraphStatus::Paused => "paused",
        GraphStatus::Escalated => "escalated",
    }
}

fn sync_point_without_receipts(
    decision: &runx_core::state_machine::FanoutSyncDecision,
) -> FanoutReceiptSyncPoint {
    FanoutReceiptSyncPoint {
        group_id: decision.group_id.clone().into(),
        strategy: match decision.strategy {
            runx_core::state_machine::FanoutSyncStrategy::All => FanoutReceiptStrategy::All,
            runx_core::state_machine::FanoutSyncStrategy::Any => FanoutReceiptStrategy::Any,
            runx_core::state_machine::FanoutSyncStrategy::Quorum => FanoutReceiptStrategy::Quorum,
        },
        decision: match decision.decision {
            runx_core::state_machine::FanoutSyncOutcome::Proceed => FanoutReceiptDecision::Proceed,
            runx_core::state_machine::FanoutSyncOutcome::Halt => FanoutReceiptDecision::Halt,
            runx_core::state_machine::FanoutSyncOutcome::Pause => FanoutReceiptDecision::Pause,
            runx_core::state_machine::FanoutSyncOutcome::Escalate => {
                FanoutReceiptDecision::Escalate
            }
        },
        rule_fired: decision.rule_fired.clone().into(),
        reason: decision.reason.clone().into(),
        branch_count: decision.branch_count,
        success_count: decision.success_count,
        failure_count: decision.failure_count,
        required_successes: decision.required_successes,
        branch_receipts: Vec::new(),
        gate: decision_gate(&decision.gate),
    }
}

fn expected_without_receipts(sync_point: &FanoutReceiptSyncPoint) -> FanoutReceiptSyncPoint {
    FanoutReceiptSyncPoint {
        branch_receipts: Vec::new(),
        ..sync_point.clone()
    }
}

fn decision_gate(gate: &Option<runx_core::state_machine::FanoutGate>) -> Option<JsonObject> {
    let value = serde_json::to_value(gate.as_ref()?).ok()?;
    let runx_contracts::JsonValue::Object(object) = serde_json::from_value(value).ok()? else {
        return None;
    };
    Some(object)
}
