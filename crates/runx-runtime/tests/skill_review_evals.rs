#![cfg(all(feature = "agent", feature = "scripted-model"))]

//! `skill-review`'s evals, on a scripted model served from this machine
//! (`runx_runtime::scripted_model`): the machinery proved with no spend. Each
//! selection is read from the run's record, never from what a turn says it
//! did.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use runx_runtime::{LocalOrchestrator, ManagedAgentPolicy, SkillRunRequest};
use serde_json::{Value, json};
use tempfile::tempdir;

use runx_runtime::scripted_model::{ScriptedModel, managed_agent_env, tool_call};

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
/// every turn worked by the scripted model at `endpoint`, under `authority`
/// when one is given.
fn review(
    root: &Path,
    method: &str,
    inputs: Value,
    endpoint: &str,
    authority: Option<Value>,
) -> TestResult<Value> {
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
            run_authority: authority.map(serde_json::from_value).transpose()?,
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
        None,
    );
    model.finish()?;
    let output = output?;
    assert_eq!(output["status"], "sealed", "{output}");
    let evidence = &output["result"]["evidence"]["data"];
    assert_eq!(evidence["schema"], "runx.eval_evidence.v1", "{output}");
    assert_eq!(evidence["package"]["revision"], digest.as_str(), "{output}");
    assert_eq!(evidence["repeats"], 3, "{evidence}");
    assert_eq!(evidence["ran_on"], "local", "{evidence}");
    // One host answered every turn, the managed agent through the scripted
    // provider, so its numbers are the evidence's one entry.
    let hosts = evidence["hosts"].as_array().ok_or("a host's numbers")?;
    assert_eq!(hosts.len(), 1, "{evidence}");
    assert_eq!(
        hosts[0]["host"],
        json!({"kind": "managed_agent", "provider": "scripted"}),
        "{evidence}"
    );
    assert_eq!(hosts[0]["models"], json!(["fixture-model"]), "{evidence}");
    let measured = &hosts[0]["results"];
    assert_eq!(measured["kind"], "selection", "{measured}");
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

/// The criteria set an eval is judged against, as the eval run builds it:
/// its expected output, then each expectation, all required.
fn criteria_of(eval: &Value) -> TestResult<runx_contracts::Criteria> {
    let mut criteria = vec![json!({
        "id": "expected_output",
        "required": true,
        "question": eval["expected_output"],
    })];
    for (index, expectation) in eval["expectations"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        criteria.push(json!({
            "id": format!("expectation_{}", index + 1),
            "required": true,
            "question": expectation,
        }));
    }
    Ok(runx_contracts::Criteria::new(serde_json::from_value(
        Value::Array(criteria),
    )?)?)
}

/// The eval a request is about, by the marker its prompt starts with.
fn eval_marker(request: &Value) -> Option<&'static str> {
    let sent = request.to_string();
    ["Meets", "Misses", "Partial", "Wrong", "Claims", "Write"]
        .into_iter()
        .find(|marker| sent.contains(&format!("{marker}:")))
}

/// An eval run works each eval in a fresh turn of the agent skill's write
/// method under the run's authority, then has each attempt judged by
/// `skill-review#grade`, whose verdict the runtime names as the grader's and
/// the run decides by the one rule. A verdict that names another evaluator,
/// or another subject, is refused, and a refused verdict answers nothing,
/// which isn't `inconclusive`. Under `effects: none` an attempt's write is
/// refused at any depth (D16).
#[test]
fn an_eval_run_grades_each_attempt_and_decides_its_verdict() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let subject = digest_package(root, "subject", "Digests a value for the person.")?;
    let evals = json!([
        {"id": 1, "prompt": "Meets: digest proof.", "expected_output": "The digest of proof.", "files": [], "expectations": ["It names the digest."]},
        {"id": 2, "prompt": "Misses: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 3, "prompt": "Partial: digest proof.", "expected_output": "The digest of proof.", "files": [], "expectations": ["It names the digest."]},
        {"id": 4, "prompt": "Wrong: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 5, "prompt": "Claims: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 6, "prompt": "Write: write the digest to a file.", "expected_output": "The digest, written nowhere.", "files": []},
    ]);
    fs::create_dir_all(subject.join("evals"))?;
    fs::write(
        subject.join("evals/evals.json"),
        json!({"skill_name": "subject", "evals": evals}).to_string(),
    )?;
    let digest = runx_runtime::load_validated_skill_package(&subject)?
        .package
        .package_digest
        .clone();
    let pinned = |reference: &str, revision: &str| json!({"ref": reference, "revision": revision});
    let mut sets = BTreeMap::new();
    for eval in evals.as_array().into_iter().flatten() {
        let marker = eval["prompt"]
            .as_str()
            .and_then(|prompt| prompt.split(':').next())
            .ok_or("an eval's prompt starts with its marker")?;
        sets.insert(
            marker.to_owned(),
            pinned(
                &format!("skills/subject/evals/evals.json#{}", eval["id"]),
                criteria_of(eval)?.revision.as_str(),
            ),
        );
    }
    let written = root.join("written.txt");
    let judged = digest.clone();
    let model = ScriptedModel::serve(move |request| {
        let complete = |answer: Value| tool_call("call-final", "turn_complete", &answer);
        let Some(marker) = eval_marker(request) else {
            return complete(json!({"summary": "No eval."}));
        };
        let sent = request.to_string();
        if !sent.contains("Judge the attempt") {
            // The attempt's turn: the write eval tries to write, and every
            // other one finishes.
            if marker == "Write" && !sent.contains("call-write") {
                return tool_call(
                    "call-write",
                    "fs_write",
                    &json!({"path": "written.txt", "contents": "proof", "create": true}),
                );
            }
            return complete(json!({"summary": format!("{marker}: done.")}));
        }
        let met = |id: &str| json!({"verdict": "met", "id": id, "evidence": []});
        let mut verdict = json!({
            "schema": "runx.verdict.v1",
            "verdict": "met",
            "subject": pinned("skills/subject", &judged),
            "criteria_set": sets[marker],
            "assessments": [met("expected_output")],
        });
        match marker {
            "Meets" => {
                verdict["assessments"] = json!([met("expected_output"), met("expectation_1")]);
            }
            "Misses" => {
                verdict["verdict"] = json!("not_met");
                verdict["assessments"] = json!([{"verdict": "not_met", "id": "expected_output", "evidence": [], "failure": {"expected": "the digest of proof", "actual": "nothing", "where": "the attempt's summary"}}]);
            }
            // It answers only the expectation, and states met for the whole.
            "Partial" => verdict["assessments"] = json!([met("expectation_1")]),
            "Wrong" => verdict["subject"] = pinned("skills/other", &judged),
            "Claims" => {
                verdict["evaluator"] =
                    json!({"package": "runx/other", "method": "judge", "run": "run_other"});
            }
            _ => {}
        }
        complete(json!({"verdict": verdict}))
    })?;

    let output = review(
        root,
        "evals",
        json!({"skill": "skills/subject"}),
        model.endpoint(),
        Some(json!({"effects": "none"})),
    );
    model.finish()?;
    let output = output?;
    assert_eq!(output["status"], "sealed", "{output}");
    let evidence = &output["result"]["evidence"]["data"];
    assert_eq!(evidence["package"]["revision"], digest.as_str(), "{output}");
    let hosts = evidence["hosts"].as_array().ok_or("a host's numbers")?;
    assert_eq!(hosts.len(), 1, "{evidence}");
    let results = &hosts[0]["results"];
    assert_eq!(results["kind"], "evals", "{results}");
    assert_eq!(results["met"], 2, "{results}");
    assert_eq!(results["refused"], 2, "{results}");
    let attempt = |id: u64| -> TestResult<Value> {
        let eval = results["evals"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|eval| eval["id"] == id)
            .ok_or_else(|| format!("eval {id} is run: {results}"))?;
        Ok(eval["attempts"][0].clone())
    };
    assert_eq!(attempt(1)?["decision"]["verdict"], "met", "{results}");
    assert_eq!(attempt(2)?["decision"]["verdict"], "not_met", "{results}");
    // An answer left out makes it inconclusive, whatever the whole it stated.
    assert_eq!(
        attempt(3)?["decision"],
        json!({"status": "decided", "verdict": "inconclusive", "unresolved": ["expected_output"]}),
        "{results}"
    );
    // A refused verdict answers nothing.
    for id in [4, 5] {
        let refused = &attempt(id)?["decision"];
        assert_eq!(refused["status"], "refused", "{refused}");
        assert!(refused.get("verdict").is_none(), "{refused}");
    }
    assert!(
        attempt(4)?["decision"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("skills/other")),
        "{results}"
    );
    assert!(
        attempt(5)?["decision"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("runx/other")),
        "{results}"
    );
    // The runtime names the grader as the evaluator, never the turn.
    let evaluator = attempt(1)?["evaluator"].clone();
    assert_eq!(evaluator["method"], "grade", "{evaluator}");
    let run_id = output["run_id"].as_str().ok_or("the run has an id")?;
    assert!(
        evaluator["run"]
            .as_str()
            .is_some_and(|run| run.starts_with(run_id) && run != run_id),
        "{evaluator}"
    );
    // The write eval's attempt wrote nothing: the run's authority refuses
    // every write, at any depth.
    assert!(!written.exists(), "an eval writes nothing to the world");
    let mut env = BTreeMap::from([("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned())]);
    env.insert(
        "RUNX_HOME".to_owned(),
        root.join(".home").to_string_lossy().into_owned(),
    );
    let read = serde_json::to_value(runx_runtime::run_record::read_run(
        &env, root, run_id, None,
    )?)?;
    assert!(
        read["events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|record| {
                record["event"]["kind"] == "call"
                    && record["event"]["target"]["tool"] == "fs.write"
                    && record["event"]["outcome"]["status"] == "refused"
            }),
        "{read}"
    );
    Ok(())
}
