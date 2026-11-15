//! Reflection (reflection.md), over a workspace laid out as a company's
//! repository: `AGENTS.md`, one skill and a rule kept in memory, committed,
//! its record made by real runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use runx_runtime::orchestrator::{LocalOrchestrator, ManagedAgentPolicy, SkillRunRequest};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const GREET: &str = "minimum_runx: 0.12.0\nmethods:\n  hello:\n    inputs: {name: {type: string}}\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {value: $input.name, encoding: utf8_text}\n  broken:\n    steps:\n      - id: compare\n        tool: data.compare\n        inputs: {expected: a, actual: b}\n      - id: insist\n        tool: data.digest\n        guard:\n          - {value: $steps.compare.comparison.equal, equals: true}\n        inputs: {value: never, encoding: utf8_text}\n  asks:\n    steps:\n      - id: confirm\n        ask: {gate: greet.confirm, reason: Say hello to the world?}\n        inputs: {quote: hello}\n";
const RULES: &str = "minimum_runx: 0.12.0\nmethods:\n  keep:\n    inputs: {text: {type: string}}\n    steps:\n      - id: remember\n        skill: runx/memory#remember\n        inputs: {level: workspace, text: $input.text, summary: Greetings name the team., purpose: mandate, date: 2026-11-16, sources: [conversation], supersedes: none}\n";

fn env(root: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([
        ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
        (
            "RUNX_REGISTRY_URL".to_owned(),
            "http://127.0.0.1:9".to_owned(),
        ),
        (
            "RUNX_HOME".to_owned(),
            root.join("home").to_string_lossy().into_owned(),
        ),
    ]);
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_owned(), path);
    }
    env
}

fn git(root: &Path, args: &[&str]) -> TestResult<String> {
    let output = Command::new("git")
        .args(["-c", "user.name=ada", "-c", "user.email=ada@example.com"])
        .args(args)
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// The workspace: AGENTS.md, a skill whose methods succeed, stop at a guard
/// and ask, and one that keeps a rule in memory, all committed.
fn brain(root: &Path) -> TestResult {
    for (name, run_yaml) in [("greet", GREET), ("rules", RULES)] {
        crate::support::write_test_skill_package(
            &root.join("skills").join(name),
            &format!("---\nname: {name}\ndescription: The {name} skill.\n---\n# {name}\n"),
            run_yaml,
        )?;
    }
    std::fs::write(root.join("AGENTS.md"), "# AGENTS.md\n\nBe brief.\n")?;
    git(root, &["init", "-q"])?;
    git(root, &["add", "-A"])?;
    git(root, &["commit", "-qm", "the workspace"])?;
    Ok(())
}

/// An orchestrator as a host gives one: the workspace's own effects, store
/// and private among them.
fn orchestrator(root: &Path) -> TestResult<LocalOrchestrator> {
    Ok(LocalOrchestrator::with_effects_and_environment(
        runx_runtime::RuntimeEffectRegistry::local()?,
        env(root),
    ))
}

fn run(
    root: &Path,
    skill: PathBuf,
    method: &str,
    inputs: serde_json::Value,
    authority: Option<serde_json::Value>,
) -> TestResult<serde_json::Value> {
    let ran = orchestrator(root)?.run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: authority.map(serde_json::from_value).transpose()?,
            act: None,
            skill_path: skill,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: serde_json::from_value(inputs)?,
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        Some(method),
        None,
        None,
    )?;
    Ok(serde_json::to_value(&ran.output)?)
}

/// Answer the one approval a paused run waits on, and drive it to its seal.
fn answer(
    root: &Path,
    run: &serde_json::Value,
    decision: serde_json::Value,
) -> TestResult<serde_json::Value> {
    let run_id = run["run_id"]
        .as_str()
        .ok_or("a paused run's id")?
        .to_owned();
    let request = run["requests"][0]["id"]
        .as_str()
        .ok_or("the approval it waits on")?;
    let answers = crate::support::bind_resume(
        &env(root),
        root,
        &run_id,
        serde_json::json!({"approvals": {request: decision}}),
    )?;
    let sealed = orchestrator(root)?.resume(&runx_runtime::ResumeRequest {
        run_id,
        answers,
        receipt_dir: Some(root.join("receipts")),
        env: env(root),
        cwd: root.to_path_buf(),
        managed_agent: ManagedAgentPolicy::HostDriven,
        package_digest: None,
        closure_digest: None,
    })?;
    Ok(serde_json::to_value(&sealed.output)?)
}

/// A term reading and writing the workspace's memory, for every package a
/// call is made from.
fn memory(verbs: &[&str], packages: &[&str]) -> serde_json::Value {
    serde_json::json!({"delegations": [{"connector": "private", "terms": [{
        "verbs": verbs,
        "bounds": {
            "operations": ["private/*"],
            "scopes": ["store.read", "store.append"],
            "targets": ["runx/memory/said:workspace:*", "runx/memory/observed:workspace:*"],
            "packages": packages,
        },
    }]}]})
}

fn survey(root: &Path) -> TestResult<serde_json::Value> {
    let surveyed = run(
        root,
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/reflect"),
        "survey",
        serde_json::json!({"reader": "nightly", "until": "2099-01-01T00:00:00Z"}),
        Some(memory(&["read"], &["project:reflect", "runx/memory"])),
    )?;
    assert_eq!(surveyed["status"], "sealed", "{surveyed}");
    Ok(surveyed["result"].clone())
}

/// Survey lays out its checks from the record before anything is judged:
/// the stopped run, the decline with the person's reason, the rule the
/// repository doesn't hold and a success to sample, one check each beside
/// its own. The same record gives the same set, by digest, and a rule is held
/// once AGENTS.md holds its handle at HEAD.
#[test]
fn survey_lays_out_its_checks_from_the_record_before_anything_is_judged() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    brain(root)?;
    let greet = root.join("skills/greet");
    let hello = run(
        root,
        greet.clone(),
        "hello",
        serde_json::json!({"name": "team"}),
        None,
    )?;
    let broken = run(root, greet.clone(), "broken", serde_json::json!({}), None)?;
    let asked = run(root, greet, "asks", serde_json::json!({}), None)?;
    let declined = answer(
        root,
        &asked,
        serde_json::json!({"approved": false, "reason": "Only the team."}),
    )?;
    assert_eq!(declined["status"], "sealed", "{declined}");
    let keeping = run(
        root,
        root.join("skills/rules"),
        "keep",
        serde_json::json!({"text": "Every greeting names the team, never the whole world."}),
        Some(memory(
            &["read", "write"],
            &["project:rules", "runx/memory"],
        )),
    )?;
    let kept = answer(root, &keeping, serde_json::json!(true))?;
    assert_eq!(kept["outcome"], "completed", "{kept}");

    let first = survey(root)?;
    let second = survey(root)?;
    assert_eq!(
        first["criteria"]["revision"], second["criteria"]["revision"],
        "{first}\n{second}"
    );
    assert_eq!(first["subject"]["revision"], second["subject"]["revision"]);
    assert_eq!(first["subject"]["head"], git(root, &["rev-parse", "HEAD"])?);
    assert_eq!(first["read_to_end"], true);
    assert_eq!(first["mark"]["open"], serde_json::json!([]));

    let resisted = first["resistance"].as_array().ok_or("resistance")?;
    let of = |kind: &str| {
        resisted
            .iter()
            .find(|item| item["kind"] == kind)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(of("sealed_blocked")["run"], broken["run_id"], "{first}");
    assert_eq!(of("declined_ask")["run"], asked["run_id"], "{first}");
    assert_eq!(of("declined_ask")["reason"], "Only the team.", "{first}");
    assert!(
        of("declined_ask")["evidence"]
            .as_str()
            .is_some_and(|receipt| receipt.starts_with("sha256:"))
    );
    assert_eq!(resisted.len(), 2, "{first}");
    let sampled = first["sampled"].as_array().ok_or("sampled")?;
    assert!(
        sampled.iter().any(|run| run["run"] == hello["run_id"]),
        "{first}"
    );
    let unheld = first["unheld"].as_array().ok_or("unheld")?;
    let handle = unheld
        .first()
        .and_then(|rule| rule["handle"].as_str())
        .ok_or("one unheld rule")?
        .to_owned();
    assert!(handle.starts_with("memory:said:workspace:"), "{handle}");
    // Its own check first, then one per item.
    let ids = first["criteria"]["criteria"]
        .as_array()
        .ok_or("criteria")?
        .iter()
        .map(|criterion| criterion["id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids[0], "record_read");
    assert_eq!(
        ids.len(),
        1 + resisted.len() + unheld.len() + sampled.len(),
        "{ids:?}"
    );

    // Promoted by hand here: AGENTS.md holds the handle at HEAD, so the rule is held.
    std::fs::write(
        root.join("AGENTS.md"),
        format!(
            "# AGENTS.md\n\nBe brief.\n\n## Rules\n\n- Every greeting names the team, never the whole world. (2026-11-16, {handle})\n"
        ),
    )?;
    git(root, &["commit", "-qam", "the rule"])?;
    let held = survey(root)?;
    assert_eq!(held["unheld"], serde_json::json!([]), "{held}");
    assert_ne!(held["subject"]["revision"], first["subject"]["revision"]);
    Ok(())
}
