#![cfg(feature = "agent")]

//! `skill-review`'s evals, on a scripted model served from this machine
//! (`scripted_model`): the machinery proved with no spend. Each selection is
//! read from the run's record, never from what a turn says it did.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use runx_runtime::{LocalOrchestrator, ManagedAgentPolicy, SkillRunRequest};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::scripted_model::{ScriptedModel, managed_agent_env, tool_call};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The first-party `skill-review` package.
fn skill_review() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/skill-review")
}

/// A package of the workspace at `root`, `skills/<name>`, that digests a
/// value.
fn digest_package(root: &Path, name: &str, description: &str) -> TestResult<PathBuf> {
    let dir = root.join("skills").join(name);
    crate::support::write_test_skill_package(
        &dir,
        &format!(
            "---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nDigest the value.\n"
        ),
        "minimum_runx: 0.12.0\nmethods:\n  digest:\n    description: Digest one value.\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
    )?;
    Ok(dir)
}

/// Run `skill-review#<method>` with `inputs` in the workspace at `root`,
/// every turn worked by the scripted model at `endpoint`.
fn review(root: &Path, method: &str, inputs: Value, endpoint: &str) -> TestResult<Value> {
    let mut env = BTreeMap::from([
        ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
        (
            "RUNX_HOME".to_owned(),
            root.join(".home").to_string_lossy().into_owned(),
        ),
        // Nothing listens here, so no turn reaches a registry.
        (
            "RUNX_REGISTRY_URL".to_owned(),
            "http://127.0.0.1:9".to_owned(),
        ),
    ]);
    env.extend(managed_agent_env(endpoint, "fixture-model"));
    crate::support::insert_test_signing_env(&mut env);
    let Value::Object(inputs) = inputs else {
        return Err("inputs are an object".into());
    };
    let result = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_review(),
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: inputs
                .into_iter()
                .map(|(name, value)| Ok((name, serde_json::from_value(value)?)))
                .collect::<Result<_, serde_json::Error>>()?,
            env,
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(4)?,
        },
        Some(method),
        None,
        None,
    )?;
    Ok(serde_json::to_value(&result.output)?)
}

/// The query a request's turn was asked, by the marker it starts with.
fn marker(request: &Value) -> Option<&'static str> {
    let sent = request.to_string();
    ["Always", "Twice", "Never", "Once", "Elsewhere"]
        .into_iter()
        .find(|marker| sent.contains(&format!("{marker}:")))
}

/// Whether the turn already loaded something: the request replays its
/// result.
fn loaded(request: &Value) -> bool {
    request["messages"].as_array().is_some_and(|messages| {
        messages
            .iter()
            .any(|message| message["role"] == "tool" && message["tool_call_id"] == "call-load")
    })
}

/// A trigger run measures how often a fresh turn of the agent skill's read
/// method loads the package for each of its queries, and reads every load
/// from the run's record: a load of another package doesn't count, and
/// neither does a turn that only says it loaded the package.
#[test]
fn a_trigger_run_measures_each_querys_rate_from_what_its_turns_loaded() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let subject = digest_package(root, "subject", "Digests a value for the person.")?;
    digest_package(root, "other", "Digests something else.")?;
    fs::write(
        subject.join("eval_queries.json"),
        json!([
            {"query": "Always: digest proof for me.", "should_trigger": true},
            {"query": "Twice: digest this value.", "should_trigger": true},
            {"query": "Never: what's the weather?", "should_trigger": false},
            {"query": "Once: tell me a joke.", "should_trigger": false},
            {"query": "Elsewhere: digest something else.", "should_trigger": false},
        ])
        .to_string(),
    )?;
    let digest = runx_runtime::load_validated_skill_package(&subject)?
        .package
        .package_digest
        .clone();
    // How many turns each query has started.
    let started = Arc::new(Mutex::new(BTreeMap::<&'static str, u32>::new()));
    let seen = Arc::clone(&started);
    let model = ScriptedModel::serve(move |request| {
        let complete =
            |summary: &str| tool_call("call-final", "turn_complete", &json!({"summary": summary}));
        if loaded(request) {
            return complete("Loaded it and stopped.");
        }
        let Some(marker) = marker(request) else {
            return complete("No query.");
        };
        let turn = {
            let mut seen = seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let count = seen.entry(marker).or_default();
            *count += 1;
            *count
        };
        let load = |skill: &str| tool_call("call-load", "load", &json!({"skill": skill}));
        match (marker, turn) {
            ("Always", _) | ("Twice", 1 | 3) | ("Once", 1) => load("skills/subject"),
            ("Elsewhere", _) => load("skills/other"),
            // A turn's own claim is never the evidence.
            _ => complete("I loaded skills/subject and used it."),
        }
    })?;

    let output = review(
        root,
        "triggers",
        json!({"skill": "skills/subject"}),
        model.endpoint(),
    );
    model.finish()?;
    let output = output?;
    assert_eq!(output["status"], "sealed", "{output}");
    let measured = &output["result"]["trigger_eval"];
    assert_eq!(measured["package_digest"], digest.as_str(), "{output}");
    assert_eq!(measured["repeats"], 3, "{measured}");
    let rows = measured["queries"].as_array().ok_or("a row per query")?;
    let row = |marker: &str| {
        rows.iter()
            .find(|row| {
                row["query"]
                    .as_str()
                    .is_some_and(|query| query.starts_with(marker))
            })
            .cloned()
            .ok_or_else(|| format!("{marker} is measured: {measured}"))
    };
    for (marker, selected, passed) in [
        ("Always", 3, true),
        ("Twice", 2, true),
        ("Never", 0, true),
        ("Once", 1, true),
        ("Elsewhere", 0, true),
    ] {
        let row = row(marker)?;
        assert_eq!(row["selected"], selected, "{row}");
        assert_eq!(row["runs"], 3, "{row}");
        assert_eq!(row["passed"], passed, "{row}");
    }
    assert_eq!(measured["passed"], 5, "{measured}");
    assert_eq!(measured["should_trigger"]["passed"], 2, "{measured}");
    assert_eq!(measured["should_not_trigger"]["passed"], 3, "{measured}");
    Ok(())
}
