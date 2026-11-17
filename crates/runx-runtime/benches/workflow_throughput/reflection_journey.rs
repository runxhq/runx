//! performance.md's journey 9, reflection, counted: what `runx/reflect#survey`
//! reads for one window of runs, after 10, 100 and 1,000 runs of history its
//! reader's mark is already past.
//!
//! Each size is a workspace of its own, a committed git repository with one
//! skill, and every run in it is a real one. The history is that many runs of
//! the skill. Then `reflect` runs, on a scripted model that finds each sampled
//! run did what its purpose asks, until its survey has read the whole history
//! and its mark is past it; one more run and one more reflection settle the
//! mark past those passes, so every size has the same one reflection after
//! its mark. Then the window: ten more runs, and the survey that is measured,
//! its calls and the store rows it reads, which follow the window and never
//! the history.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use runx_contracts::{JsonNumber, JsonObject, JsonValue, RunEventFact};
use runx_runtime::scripted_model::{ScriptedModel, drive_env, tool_call};
use runx_runtime::work_counts::snapshot;
use runx_runtime::{DrivePolicy, LocalOrchestrator, RuntimeEffectRegistry, SkillRunRequest};
use tempfile::TempDir;

const SIZES: [(&str, usize); 3] = [("10", 10), ("100", 100), ("1000", 1000)];
/// The window survey reads at every size.
const WINDOW: usize = 10;
/// The most reflections a history takes to catch up: survey reads about 390
/// runs a pass.
const CATCH_UP: usize = 6;

#[allow(clippy::expect_used)]
pub(super) fn register() {
    let measured = reflection().expect("the reflection journey must survey a window at each size");
    crate::record_journey("9", measured).expect("the reflection journey must be recorded");
}

fn reflection() -> Result<JsonValue, Box<dyn Error>> {
    let model = ScriptedModel::serve(|request| {
        // Each sampled run did what its purpose asks, as its first event shows.
        let address = request["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|message| message["content"].as_str())
            .find_map(|text| {
                let start = text.find("\n{")?;
                let context = serde_json::from_str::<serde_json::Value>(&text[start + 1..]).ok()?;
                context["inputs"]["run"]["events"][0]["address"]
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        tool_call(
            "call-final",
            "turn_complete",
            &serde_json::json!({"finding": {"met": true, "summary": "It did what its purpose asks.", "evidence": [address]}}),
        )
    })?;
    let mut calls = JsonObject::new();
    let mut rows = JsonObject::new();
    for (label, size) in SIZES {
        let workspace = TempDir::new()?;
        let root = workspace.path();
        lay_out(root)?;
        let env = environment(root, model.endpoint());
        for index in 0..size {
            greet(root, &env, index)?;
        }
        let mut caught_up = false;
        for _ in 0..CATCH_UP {
            if reflect(root, &env)? {
                caught_up = true;
                break;
            }
        }
        if !caught_up {
            return Err(format!("reflection never caught up with {size} runs").into());
        }
        greet(root, &env, size)?;
        reflect(root, &env)?;
        for index in 0..WINDOW {
            greet(root, &env, size + 1 + index)?;
        }
        let before = snapshot();
        let surveyed = run(root, &env, "survey", "survey")?;
        let after = snapshot();
        let run_id = surveyed["run_id"].as_str().ok_or("the survey's run id")?;
        let read = runx_runtime::run_record::read_run(&env, root, run_id, None)?;
        let made = read
            .events
            .iter()
            .filter(|record| matches!(record.event, RunEventFact::Call { .. }))
            .count();
        calls.insert(
            label.to_owned(),
            JsonValue::Number(JsonNumber::U64(u64::try_from(made)?)),
        );
        rows.insert(
            label.to_owned(),
            JsonValue::Number(JsonNumber::U64(
                after.store_rows_read - before.store_rows_read,
            )),
        );
    }
    model.finish()?;
    Ok(JsonValue::Object(JsonObject::from([
        ("survey_calls".to_owned(), JsonValue::Object(calls)),
        ("survey_rows_read".to_owned(), JsonValue::Object(rows)),
    ])))
}

/// A committed workspace: AGENTS.md and one skill that digests a name.
fn lay_out(root: &Path) -> Result<(), Box<dyn Error>> {
    let skill = root.join("skills/greet");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: greet\ndescription: Digests a greeting.\n---\n# greet\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  hello:\n    inputs: {name: {type: string}}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {value: $input.name, encoding: utf8_text}\n",
    )?;
    fs::write(root.join("AGENTS.md"), "# AGENTS.md\n")?;
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "the workspace"],
    ] {
        let status = Command::new("git")
            .args(["-c", "user.name=ada", "-c", "user.email=ada@example.com"])
            .args(&args)
            .current_dir(root)
            .status()?;
        if !status.success() {
            return Err(format!("git {args:?} failed").into());
        }
    }
    Ok(())
}

fn environment(root: &Path, endpoint: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([
        ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
        (
            "RUNX_HOME".to_owned(),
            root.join("home").to_string_lossy().into_owned(),
        ),
        (
            "RUNX_REGISTRY_URL".to_owned(),
            "http://127.0.0.1:9".to_owned(),
        ),
    ]);
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_owned(), path);
    }
    env.extend(drive_env(endpoint, "fixture-model"));
    env
}

fn reflect_package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/reflect")
}

fn greet(root: &Path, env: &BTreeMap<String, String>, index: usize) -> Result<(), Box<dyn Error>> {
    let output = execute(
        root,
        env,
        root.join("skills/greet"),
        "hello",
        serde_json::json!({"name": format!("team {index}")}),
        DrivePolicy::HostDriven,
    )?;
    if output["status"] != "sealed" {
        return Err(format!("greet didn't seal: {output}").into());
    }
    Ok(())
}

/// One reflection; whether its survey read its whole window.
fn reflect(root: &Path, env: &BTreeMap<String, String>) -> Result<bool, Box<dyn Error>> {
    let output = run(root, env, "reflect", "reflect")?;
    Ok(output["context"]["step_outputs"]["survey"]["read_to_end"] == true)
}

fn run(
    root: &Path,
    env: &BTreeMap<String, String>,
    method: &str,
    what: &str,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let output = execute(
        root,
        env,
        reflect_package(),
        method,
        serde_json::json!({"reader": "nightly", "until": "2099-01-01T00:00:00Z"}),
        DrivePolicy::inline(4)?,
    )?;
    if output["status"] != "sealed" || output["outcome"] != "completed" {
        return Err(format!("{what} didn't complete: {output}").into());
    }
    Ok(output)
}

fn execute(
    root: &Path,
    env: &BTreeMap<String, String>,
    skill_path: PathBuf,
    method: &str,
    inputs: serde_json::Value,
    drive: DrivePolicy,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let ran = LocalOrchestrator::with_effects_and_environment(
        RuntimeEffectRegistry::local()?,
        env.clone(),
    )
    .run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: serde_json::from_value(inputs)?,
            env: env.clone(),
            cwd: root.to_path_buf(),
            drive,
        },
        Some(method),
        None,
        None,
    )?;
    Ok(serde_json::to_value(&ran.output)?)
}
