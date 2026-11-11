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

use crate::registry_fixture::LoopbackRegistry;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The first-party `skill-review` package.
fn skill_review() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/skill-review")
}

/// A package at `root/skills/<name>`, version 1.0.0, that digests a value.
fn digest_package(root: &Path, name: &str, description: &str) -> TestResult<PathBuf> {
    let dir = root.join("skills").join(name);
    crate::support::write_test_skill_package(
        &dir,
        &format!(
            "---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nDigest the value.\n"
        ),
        "version: 1.0.0\nminimum_runx: 0.12.0\nmethods:\n  digest:\n    description: Digest one value.\n    inputs:\n      value: {type: string}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
    )?;
    Ok(dir)
}

/// Publish the package at `dir`, written outside any workspace under
/// `source`, to `registry` as acme's, as `runx registry publish` sends it,
/// and return its distribution digest, the one a load of it binds.
fn release(registry: &LoopbackRegistry, source: &Path, dir: &Path) -> TestResult<String> {
    let env = registry.env()?;
    registry.publish_directory(dir, "acme", &env, source)?;
    let subject = dir.to_string_lossy();
    Ok(runx_runtime::registry::prepare_registry_publish_package(
        runx_runtime::registry::RegistryPublishPackageRequest {
            subject: &subject,
            env: &env,
            cwd: source,
        },
    )?
    .into_parts()
    .package_digest()?)
}

/// The environment of the workspace at `root`, whose home is `root/.home`,
/// reaching `registry`.
fn workspace_env(root: &Path, registry: &LoopbackRegistry) -> TestResult<BTreeMap<String, String>> {
    let mut env = registry.env()?;
    env.extend([
        ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
        (
            "RUNX_HOME".to_owned(),
            root.join(".home").to_string_lossy().into_owned(),
        ),
        ("RUNX_REGISTRY_URL".to_owned(), registry.url.clone()),
    ]);
    Ok(env)
}

/// The environment of a run in the workspace at `root` that reaches
/// `registry`, its turns worked by the scripted model at `endpoint`.
fn eval_env(
    root: &Path,
    registry: &LoopbackRegistry,
    endpoint: &str,
) -> TestResult<BTreeMap<String, String>> {
    let mut env = workspace_env(root, registry)?;
    env.extend(managed_agent_env(endpoint, "fixture-model"));
    crate::support::insert_test_signing_env(&mut env);
    Ok(env)
}

/// The person states their trust in `reference` ahead of a measurement, as
/// `runx trust` does: recorded in the home for each closure a load of it
/// binds, so a measurement's loads of it bind without waiting.
fn trust_ahead(root: &Path, registry: &LoopbackRegistry, reference: &str) -> TestResult {
    runx_runtime::state_trust(reference, None, &workspace_env(root, registry)?, root)?;
    Ok(())
}

/// Run `skill-review#<method>` with `inputs` in the workspace at `root`,
/// reaching `registry`, every turn worked by the scripted model at
/// `endpoint`, under `authority` when one is given.
fn review(
    root: &Path,
    registry: &LoopbackRegistry,
    method: &str,
    inputs: Value,
    endpoint: &str,
    authority: Option<Value>,
) -> TestResult<Value> {
    let env = eval_env(root, registry, endpoint)?;
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
    let source = tempdir()?;
    let registry = LoopbackRegistry::serve()?;
    // The candidate is released, and the workspace holds no copy of it.
    let subject = digest_package(source.path(), "subject", "Digests a value for the person.")?;
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
    let digest = release(&registry, source.path(), &subject)?;
    trust_ahead(root, &registry, "acme/subject@1.0.0")?;
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
            ("Always", _) | ("Twice", 1 | 3) | ("Once", 1) => load("acme/subject@1.0.0"),
            ("Elsewhere", _) => load("skills/other"),
            // A turn's own claim is never the evidence.
            _ => complete("I loaded acme/subject@1.0.0 and used it."),
        }
    })?;

    let output = review(
        root,
        &registry,
        "triggers",
        json!({"skill": "acme/subject@1.0.0"}),
        model.endpoint(),
        None,
    );
    // A workspace path names a source directory, which has no one identity
    // to measure, so it is never measured.
    let unreleased = review(
        root,
        &registry,
        "triggers",
        json!({"skill": "skills/other"}),
        model.endpoint(),
        None,
    );
    model.finish()?;
    let refused = unreleased.err().ok_or("a workspace path is refused")?;
    assert!(
        refused.to_string().contains("pattern"),
        "the input names a registry reference: {refused}"
    );
    let output = output?;
    assert_eq!(output["status"], "sealed", "{output}");
    let evidence = &output["result"]["evidence"];
    assert_eq!(evidence["schema"], "runx.eval_evidence.v1", "{output}");
    assert_eq!(
        evidence["package"],
        json!({"ref": "acme/subject@1.0.0", "revision": digest}),
        "{output}"
    );
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
    [
        "Meets",
        "Misses",
        "Partial",
        "Wrong",
        "Claims",
        "Write",
        "Uncalled",
        "LoadOnly",
        "Other",
        "OtherMethod",
    ]
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
    let source = tempdir()?;
    let registry = LoopbackRegistry::serve()?;
    let subject = digest_package(source.path(), "subject", "Digests a value for the person.")?;
    let manifest = subject.join("run.yaml");
    fs::write(
        &manifest,
        format!(
            "{}  alternate:\n    description: Digest through another method.\n    inputs:\n      value: {{type: string}}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
            fs::read_to_string(&manifest)?
        ),
    )?;
    let evals = json!([
        {"id": 1, "prompt": "Meets: digest proof.", "expected_output": "The digest of proof.", "files": ["evals/files/prior.json"], "expectations": ["It names the digest."], "method": "digest"},
        {"id": 2, "prompt": "Misses: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 3, "prompt": "Partial: digest proof.", "expected_output": "The digest of proof.", "files": [], "expectations": ["It names the digest."]},
        {"id": 4, "prompt": "Wrong: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 5, "prompt": "Claims: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 6, "prompt": "Write: write the digest to a file.", "expected_output": "The digest, written nowhere.", "files": []},
        {"id": 7, "prompt": "Uncalled: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 8, "prompt": "LoadOnly: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 9, "prompt": "Other: digest proof.", "expected_output": "The digest of proof.", "files": []},
        {"id": 10, "prompt": "OtherMethod: digest proof.", "expected_output": "The digest of proof.", "files": [], "method": "digest"},
    ]);
    fs::create_dir_all(subject.join("evals/files"))?;
    fs::write(
        subject.join("evals/evals.json"),
        json!({"skill_name": "subject", "evals": evals}).to_string(),
    )?;
    // The prior run the first eval builds on, which its attempt is given.
    fs::write(
        subject.join("evals/files/prior.json"),
        r#"{"prior": "the value is proof"}"#,
    )?;
    let digest = release(&registry, source.path(), &subject)?;
    let other = digest_package(
        source.path(),
        "other",
        "Digests a different package's value.",
    )?;
    release(&registry, source.path(), &other)?;
    for package in ["acme/subject@1.0.0", "acme/other@1.0.0"] {
        trust_ahead(root, &registry, package)?;
    }
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
                &format!("acme/subject@1.0.0/evals/evals.json#{}", eval["id"]),
                criteria_of(eval)?.revision.as_str(),
            ),
        );
    }
    let written = root.join("written.txt");
    let judged = digest.clone();
    // Whether the first eval's attempt was given its prior run.
    let given_prior = Arc::new(Mutex::new(false));
    let saw_prior = Arc::clone(&given_prior);
    let graded = Arc::new(Mutex::new(Vec::new()));
    let saw_grade = Arc::clone(&graded);
    let supplied = Arc::new(Mutex::new(Vec::new()));
    let saw_supplied = Arc::clone(&supplied);
    let model = ScriptedModel::serve(move |request| {
        let complete = |answer: Value| tool_call("call-final", "turn_complete", &answer);
        let Some(marker) = eval_marker(request) else {
            return complete(json!({"summary": "No eval."}));
        };
        let sent = request.to_string();
        if !sent.contains("Judge the attempt") {
            // The first eval's attempt reads the prior run its file holds,
            // given as a context entry its turn lists by its path.
            if marker == "Meets" && sent.contains("evals/files/prior.json") {
                if !sent.contains("call-prior") {
                    return tool_call("call-prior", "turn_read", &json!({"locator": "current:0"}));
                }
                if sent.contains("the value is proof") {
                    *saw_prior
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                }
            }
            // Functional callers cannot do the subject's job with general
            // tools. Only the measured loaded method performs that work.
            if request["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|tool| {
                    matches!(
                        tool["function"]["name"].as_str(),
                        Some("fs_write" | "fs_read" | "registry_search")
                    )
                })
            {
                return complete(
                    json!({"summary": "The evaluation caller incorrectly offers general task tools."}),
                );
            }
            if marker == "Meets" && !sent.contains("Exercise acme/subject@1.0.0#digest for eval 1.")
            {
                return complete(json!({"summary": "No measured method was supplied."}));
            }
            if marker == "Uncalled" {
                return complete(json!({"summary": "I used the subject and finished."}));
            }
            if !sent.contains("call-load") {
                return tool_call(
                    "call-load",
                    "load",
                    &json!({
                        "skill": if marker == "Other" { "acme/other@1.0.0" } else { "acme/subject@1.0.0" },
                        "method": if marker == "OtherMethod" { "alternate" } else { "digest" },
                    }),
                );
            }
            if marker != "LoadOnly" && !sent.contains("call-subject") {
                let ending = if marker == "OtherMethod" {
                    "__alternate"
                } else {
                    "__digest"
                };
                let name = request["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool["function"]["name"].as_str())
                    .find(|name| name.ends_with(ending))
                    .unwrap_or("missing_subject");
                return tool_call("call-subject", name, &json!({"value": "proof"}));
            }
            return complete(json!({"summary": format!("{marker}: done.")}));
        }
        saw_grade
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(marker);
        for message in request["messages"].as_array().into_iter().flatten() {
            if message["role"] != "user" {
                continue;
            }
            if let Some(text) = message["content"].as_str()
                && let Some(start) = text.find("\n{")
                && let Ok(context) = serde_json::from_str::<Value>(&text[start + 1..])
            {
                let mut supplied = context["inputs"]["attempt"].clone();
                supplied["offered_schema"] = request["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|tool| tool["function"]["name"] == "turn_complete")
                    .map(|tool| tool["function"]["parameters"].clone())
                    .unwrap_or_default();
                saw_supplied
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((marker, supplied));
            }
        }
        let met = |id: &str| json!({"verdict": "met", "id": id, "evidence": []});
        let mut verdict = json!({
            "schema": "runx.verdict.v1",
            "verdict": "met",
            "subject": pinned("acme/subject@1.0.0", &judged),
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
        &registry,
        "evals",
        json!({"skill": "acme/subject@1.0.0"}),
        model.endpoint(),
        Some(json!({"effects": "none"})),
    );
    model.finish()?;
    let output = output?;
    assert_eq!(output["status"], "sealed", "{output}");
    assert!(
        *given_prior
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        "the first eval's attempt was given the prior run its file holds"
    );
    let evidence = &output["result"]["evidence"];
    assert_eq!(evidence["package"]["revision"], digest.as_str(), "{output}");
    let hosts = evidence["hosts"].as_array().ok_or("a host's numbers")?;
    assert_eq!(hosts.len(), 1, "{evidence}");
    let results = &hosts[0]["results"];
    assert_eq!(results["kind"], "evals", "{results}");
    assert_eq!(results["met"], 2, "{results}");
    assert_eq!(results["refused"], 6, "{results}");
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
    for id in [7, 8, 9, 10] {
        let row = attempt(id)?;
        assert_eq!(row["decision"]["status"], "refused", "{row}");
        assert!(
            row["decision"]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("no subject execution")),
            "{row}"
        );
        assert!(row.get("evaluator").is_none(), "{row}");
    }
    let graded = graded
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        graded
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        6,
        "unexercised attempts spend no grading calls: {graded:?}"
    );
    let supplied = supplied
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_, actual) = supplied
        .iter()
        .find(|(marker, _)| *marker == "Meets")
        .ok_or("the grader received native evidence")?;
    let calls = actual["calls"].as_array().ok_or("typed calls")?;
    assert!(
        actual["offered_schema"]["properties"]["verdict"]["properties"]
            .get("evaluator")
            .is_none(),
        "the model cannot choose the runtime evaluator: {actual}"
    );
    assert_eq!(calls.len(), 2, "one final record per call: {actual}");
    let call = calls
        .iter()
        .find(|call| call["target"]["kind"] == "skill")
        .ok_or("the subject was called")?;
    assert_eq!(call["outcome"]["status"], "completed", "{call}");
    assert_eq!(call["outcome"]["succeeded"], true, "{call}");
    assert!(
        actual["outputs"]
            .as_array()
            .is_some_and(|outputs| !outputs.is_empty()
                && outputs
                    .iter()
                    .all(|output| output.get("content").is_some()
                        && output.get("read_error").is_none()))
            || call["outcome"].get("output").is_some(),
        "the actual output reaches the grader: {actual}"
    );
    assert!(
        attempt(4)?["decision"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("skills/other")),
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
    // The attempt has no independent write capability.
    assert!(!written.exists(), "an eval writes nothing to the world");
    Ok(())
}
