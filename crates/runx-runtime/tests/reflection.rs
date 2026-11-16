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
const RULES: &str = "minimum_runx: 0.12.0\nmethods:\n  keep:\n    inputs: {text: {type: string}, level: {type: string, default: workspace, optional: true}}\n    steps:\n      - id: remember\n        skill: runx/memory#remember\n        inputs: {level: $input.level, text: $input.text, summary: Greetings name the team., purpose: mandate, date: 2026-11-16, sources: [conversation]}\n  index:\n    steps:\n      - id: index\n        skill: runx/memory#index\n        inputs: {levels: [{level: personal}, {level: workspace}]}\n";

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

/// The workspace's record, made by real runs: a success, a run a guard
/// stopped, an ask declined with the person's reason, and a rule kept in
/// memory through `runx/memory#remember`. Returns the first three runs.
fn record(root: &Path) -> TestResult<[serde_json::Value; 3]> {
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

    Ok([hello, broken, asked])
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
    let [hello, broken, asked] = record(root)?;
    let first = survey(root)?;
    let second = survey(root)?;
    assert_eq!(
        first["criteria"]["revision"], second["criteria"]["revision"],
        "{first}\n{second}"
    );
    assert_eq!(first["subject"]["revision"], second["subject"]["revision"]);
    assert_eq!(first["head"], git(root, &["rev-parse", "HEAD"])?);
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

/// `reflect#reflect` in the workspace at `root`, its turns worked by the
/// scripted model at `endpoint`, as runx's managed agent drives them.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn reflect(root: &Path, endpoint: &str) -> TestResult<serde_json::Value> {
    reflect_as(root, endpoint, "nightly")
}

/// `reflect#reflect` as `reader`, its turns worked by the scripted model.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn reflect_as(root: &Path, endpoint: &str, reader: &str) -> TestResult<serde_json::Value> {
    let mut environment = env(root);
    environment.extend(runx_runtime::scripted_model::managed_agent_env(
        endpoint,
        "fixture-model",
    ));
    let ran = LocalOrchestrator::with_effects_and_environment(
        runx_runtime::RuntimeEffectRegistry::local()?,
        environment.clone(),
    )
    .run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: Some(serde_json::from_value(reflecting())?),
            act: None,
            skill_path: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/reflect"),
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: serde_json::from_value(
                serde_json::json!({"reader": reader, "until": "2099-01-01T00:00:00Z"}),
            )?,
            env: environment,
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(4)?,
        },
        Some("reflect"),
        None,
        None,
    )?;
    Ok(serde_json::to_value(&ran.output)?)
}

/// The inputs a turn was given, from the context its request carries.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn turn_inputs(request: &serde_json::Value) -> serde_json::Value {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_str())
        .find_map(|text| {
            let start = text.find("\n{")?;
            serde_json::from_str::<serde_json::Value>(&text[start + 1..]).ok()
        })
        .map(|context| context["inputs"].clone())
        .unwrap_or_default()
}

/// Read-only reflection on scripted answers: each resistance is diagnosed
/// and each sampled success asked the purpose-only question, each a fresh
/// turn. A diagnosis citing what the record doesn't hold is dropped. The
/// window is decided by the one rule: what nothing has kept yet is not met,
/// and the loop repeats exactly.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
#[test]
fn reflection_judges_the_window_and_drops_a_finding_the_record_lacks() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    record(root)?;
    let model = reflection_model(Diagnoses::MisdirectTheStoppedRun)?;
    let endpoint = model.endpoint().to_owned();
    // The rule's promotion waits on the person; approved once, it's in the
    // workspace's AGENTS.md. The first reflection moves its reader's mark, so
    // the same record is judged again by another reader, which writes nothing.
    let first = drive(root, &endpoint, reflect(root, &endpoint)?, |_| {
        serde_json::json!(true)
    })?;
    let second = reflect_as(root, &endpoint, "again")?;
    assert_eq!(first["status"], "sealed", "{first}");
    assert_eq!(second["status"], "sealed", "{second}");
    model.finish()?;

    let findings = first["result"]["findings"].as_array().ok_or("findings")?;
    let finding = |criterion: &str| {
        findings
            .iter()
            .find(|entry| entry["criterion"] == criterion)
            .cloned()
            .unwrap_or_default()
    };
    let resistance = |kind: &str| {
        first["context"]["step_outputs"]["survey"]["resistance"]
            .as_array()
            .and_then(|items| items.iter().position(|item| item["kind"] == kind))
            .map(|index| format!("resisted_{}", index + 1))
            .unwrap_or_default()
    };
    assert!(
        finding(&resistance("sealed_blocked"))["dropped"]
            .as_str()
            .is_some_and(|why| why.contains("doesn't hold")),
        "{first}"
    );
    assert!(
        finding(&resistance("declined_ask"))["finding"].is_object(),
        "{first}"
    );
    assert_eq!(finding("sampled_1")["finding"]["met"], true, "{first}");

    let verdict = &first["result"]["verdict"];
    assert_eq!(verdict["verdict"], "not_met", "{verdict}");
    let answer = |id: &str| {
        verdict["assessments"]
            .as_array()
            .and_then(|all| all.iter().find(|entry| entry["id"] == id).cloned())
            .unwrap_or_default()
    };
    assert_eq!(answer("record_read")["verdict"], "met");
    assert_eq!(answer("sampled_1")["verdict"], "met");
    // Promoted, and observed: a decline is kept as what it found.
    assert_eq!(answer("unheld_1")["verdict"], "met");
    assert_eq!(answer(&resistance("declined_ask"))["verdict"], "met");
    // Its diagnosis dropped, the stopped run is kept as nothing.
    assert_eq!(answer(&resistance("sealed_blocked"))["verdict"], "not_met");
    assert_eq!(verdict["evaluator"]["method"], "reflect", "{verdict}");
    assert_eq!(first["result"]["decision"]["verdict"], "not_met", "{first}");

    // The same record and the same answers give the same judgement.
    assert_eq!(first["result"]["findings"], second["result"]["findings"]);
    assert_eq!(
        verdict["assessments"],
        second["result"]["verdict"]["assessments"]
    );
    Ok(())
}

/// What skill-build's architect decides for one objective: a no_change for
/// a decline, and for the stopped run an internal extension of the skill
/// that keeps its name.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn architecture(objective: &str) -> serde_json::Value {
    let effects = serde_json::json!([{"effect": "none", "authority_scopes": [], "approval": "none", "provider_boundary": false}]);
    let native_reuse = serde_json::json!({"inspected_capabilities": ["runx.skill.inspect", "runx.skill.plan", "runx.skill.bind", "runx.skill.apply"], "selected_capabilities": [], "missing_capabilities": []});
    let knowledge = serde_json::json!({"purpose": "Greet the team.", "evidence_required": ["The run's record."], "decision_logic": ["Fix only what the record shows."], "stop_conditions": ["Stop when the record shows nothing to fix."], "recovery": ["Resume with the record."]});
    let proofs = serde_json::json!([{"name": "selects", "kind": "trigger_eval", "expected": "A greeting selects greet."}]);
    if !objective.contains("sealed_blocked") {
        return serde_json::json!({
            "schema": "runx.skill.architecture_decision.v1", "choice": "no_change", "objective": objective,
            "operator_value": "Keep a person's decline as theirs.", "knowledge_contract": knowledge,
            "required_behaviors": [{"id": "keep", "outcome": "Leave the skill as it is.", "lane": "manual"}],
            "native_reuse": native_reuse, "effects": effects, "skill_chain": {"context_skills": [], "routes": []},
            "resource_budget": {"max_files": 0, "max_executable_lines": 0, "max_process_spawns": 0, "network_allowed": false},
            "preservation_obligations": ["Keep every method."], "deletions": [], "proof_plan": proofs,
        });
    }
    serde_json::json!({
        "schema": "runx.skill.architecture_decision.v1", "choice": "extend_existing",
        "identity": {"current_name": "greet", "proposed_name": "greet", "action": "keep", "visibility": "internal", "rationale": "The fix is greet's own."},
        "objective": objective, "operator_value": "A greeting that runs to its end.", "knowledge_contract": knowledge,
        "required_behaviors": [{"id": "insist", "outcome": "The broken method runs both its steps.", "lane": "manual"}],
        "native_reuse": native_reuse, "effects": effects, "skill_chain": {"context_skills": [], "routes": []},
        "resource_budget": {"max_files": 8, "max_executable_lines": 0, "max_process_spawns": 0, "network_allowed": false},
        "preservation_obligations": ["Keep every method."], "deletions": [], "proof_plan": proofs,
    })
}

/// The author's draft for the stopped run: the compare made to agree, and a
/// case that expects both steps, which the old revision's guard stops.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn fixed_greet() -> serde_json::Value {
    let fixed = GREET
        .replacen(
            "minimum_runx: 0.12.0\n",
            "minimum_runx: 0.12.0\ncatalog:\n  visibility: internal\n",
            1,
        )
        .replacen("{expected: a, actual: b}", "{expected: b, actual: b}", 1);
    serde_json::json!({
        "schema": "runx.skill.change_draft.v1", "decision": "write",
        "summary": "Make broken's compare agree, so its guard lets insist run.",
        "non_goals": ["Change no other method."],
        "writes": [
            {"path": "run.yaml", "contents": fixed},
            {"path": "tests/broken-insists.yaml", "contents": "name: broken-insists\nmethod: broken\nexpect:\n  status: sealed\n  steps: [compare, insist]\n"},
        ],
        "deletes": [], "expected_outputs": [],
    })
}

/// How a scripted reflection's diagnoses are given.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
#[derive(Clone, Copy, PartialEq)]
enum Diagnoses {
    /// Each cites its own evidence, in the same words every time.
    Faithful,
    /// The stopped run's cites a receipt no record holds.
    MisdirectTheStoppedRun,
    /// Each is put in words of its own, as a model's are.
    Reworded,
}

/// The scripted model a whole reflection runs on: each diagnosis as
/// `diagnoses` gives it, each sampled run did what its purpose asks, and
/// skill-build writes the fix and its case for the stopped run.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn reflection_model(
    diagnoses: Diagnoses,
) -> TestResult<runx_runtime::scripted_model::ScriptedModel> {
    use runx_runtime::scripted_model::{ScriptedModel, tool_call};
    let diagnosed = std::sync::atomic::AtomicUsize::new(0);
    Ok(ScriptedModel::serve(move |request| {
        let inputs = turn_inputs(request);
        let offered = request["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|tool| tool["function"]["name"] == "turn_complete")
            .map(|tool| tool["function"]["parameters"]["properties"].clone())
            .unwrap_or_default();
        let complete =
            |output: serde_json::Value| tool_call("call-final", "turn_complete", &output);
        if offered.get("architecture_decision").is_some() {
            let objective = inputs["objective"].as_str().unwrap_or_default();
            return complete(serde_json::json!({"architecture_decision": architecture(objective)}));
        }
        if offered.get("change_draft").is_some() {
            return complete(serde_json::json!({"change_draft": fixed_greet()}));
        }
        if let Some(resistance) = inputs.get("resistance") {
            // Misdirected, the stopped run's diagnosis cites a receipt no
            // record holds.
            let evidence = if diagnoses == Diagnoses::MisdirectTheStoppedRun
                && resistance["kind"] == "sealed_blocked"
            {
                serde_json::json!([
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                ])
            } else {
                serde_json::json!([resistance["evidence"]])
            };
            return complete(serde_json::json!({"finding": {
                "summary": format!("The {} stopped the run.", resistance["kind"].as_str().unwrap_or_default()),
                "cause": if diagnoses == Diagnoses::Reworded {
                    format!(
                        "The record shows it, as diagnosis {} reads it.",
                        diagnosed.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    )
                } else {
                    "The record shows it.".to_owned()
                },
                "evidence": evidence,
            }}));
        }
        let address = inputs["run"]["events"][0]["address"].clone();
        complete(
            serde_json::json!({"finding": {"met": true, "summary": "It did what its purpose asks.", "evidence": [address]}}),
        )
    })?)
}

/// Resume a paused reflection with `decision` for the change it waits on,
/// its turns still worked by the scripted model at `endpoint`.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn decide_change(
    root: &Path,
    paused: &serde_json::Value,
    endpoint: &str,
    decision: serde_json::Value,
) -> TestResult<serde_json::Value> {
    let mut environment = env(root);
    environment.extend(runx_runtime::scripted_model::managed_agent_env(
        endpoint,
        "fixture-model",
    ));
    let run_id = paused["run_id"]
        .as_str()
        .ok_or("a paused run's id")?
        .to_owned();
    let request = paused["requests"][0]["id"]
        .as_str()
        .ok_or("the change it waits on")?;
    let answers = crate::support::bind_resume(
        &environment,
        root,
        &run_id,
        serde_json::json!({"approvals": {request: decision}}),
    )?;
    let sealed = LocalOrchestrator::with_effects_and_environment(
        runx_runtime::RuntimeEffectRegistry::local()?,
        environment.clone(),
    )
    .resume(&runx_runtime::ResumeRequest {
        run_id,
        answers,
        receipt_dir: Some(root.join("receipts")),
        env: environment,
        cwd: root.to_path_buf(),
        managed_agent: ManagedAgentPolicy::inline(4)?,
        package_digest: None,
        closure_digest: None,
    })?;
    Ok(serde_json::to_value(&sealed.output)?)
}

/// Reflection's term: it reads the workspace's memory and writes only its
/// observations, never a person's words.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn reflecting() -> serde_json::Value {
    let packages = ["project:reflect", "runx/memory"];
    serde_json::json!({"delegations": [{"connector": "private", "terms": [
        {"verbs": ["read"], "bounds": {"operations": ["private/*"], "scopes": ["store.read"],
            "targets": ["runx/memory/said:workspace:*", "runx/memory/observed:workspace:*"], "packages": packages}},
        {"verbs": ["write"], "bounds": {"operations": ["private/*"], "scopes": ["store.append"],
            "targets": ["runx/memory/observed:workspace:*"], "packages": packages}},
    ]}]})
}

/// Drive a reflection to its seal, answering each change it waits on with
/// what `decide` says of it.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
fn drive(
    root: &Path,
    endpoint: &str,
    mut output: serde_json::Value,
    decide: impl Fn(&serde_json::Value) -> serde_json::Value,
) -> TestResult<serde_json::Value> {
    let mut asked = 0;
    while output["status"] == "needs_approval" {
        asked += 1;
        if asked > 8 {
            return Err(format!("still waiting after eight answers: {output}").into());
        }
        let decision = decide(&output["requests"][0]);
        output = decide_change(root, &output, endpoint, decision)?;
    }
    Ok(output)
}

/// The whole loop on scripted answers. Approved, the fix and its case are
/// written once the new case is proven to fail on the old revision and pass
/// on the fix, the rule goes into AGENTS.md word for word, an observation is
/// kept and the mark moves; once the person commits, the next reflection asks
/// nothing, names the commit in the rule's closing disable, and its window is
/// met. Declined with a reason, nothing is written, and another reader over
/// the same record isn't asked about that change again.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
#[test]
fn reflection_keeps_what_it_proves_once_the_person_approves_it() -> TestResult {
    let model = reflection_model(Diagnoses::Reworded)?;
    let endpoint = model.endpoint().to_owned();

    let approved = tempdir()?;
    let root = approved.path();
    record(root)?;
    let sealed = drive(root, &endpoint, reflect(root, &endpoint)?, |_| {
        serde_json::json!(true)
    })?;
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    let result = &sealed["result"];
    let proposals = result["proposals"].as_array().ok_or("proposals")?;
    let fix = proposals
        .iter()
        .find(|entry| entry["applied"].is_object())
        .ok_or("an applied fix")?;
    assert_eq!(fix["proof"]["before"]["broken-insists"], "failed", "{fix}");
    assert_eq!(fix["proof"]["after"]["broken-insists"], "passed", "{fix}");
    assert!(
        proposals
            .iter()
            .any(|entry| entry["dropped"] == "the draft changes nothing"),
        "{sealed}"
    );
    assert!(
        std::fs::read_to_string(root.join("skills/greet/run.yaml"))?
            .contains("{expected: b, actual: b}")
    );
    assert!(root.join("skills/greet/tests/broken-insists.yaml").exists());
    let handle = result["promotion"]["promoted"][0]["handle"]
        .as_str()
        .ok_or("a promoted rule")?
        .to_owned();
    let instructions = std::fs::read_to_string(root.join("AGENTS.md"))?;
    assert!(instructions.contains(&format!("## Rules\n\n- Every greeting names the team, never the whole world. (2026-11-16, {handle})")), "{instructions}");
    let answers = result["verdict"]["assessments"]
        .as_array()
        .ok_or("assessments")?;
    let answered = |id: &str| {
        answers
            .iter()
            .find(|entry| entry["id"] == id)
            .map(|entry| entry["verdict"].clone())
            .unwrap_or_default()
    };
    assert_eq!(
        answered(fix["criterion"].as_str().unwrap_or_default()),
        "met",
        "{sealed}"
    );
    assert_eq!(answered("unheld_1"), "met", "{sealed}");
    assert!(
        result["observed"]
            .as_array()
            .is_some_and(|kept| kept.iter().any(|entry| entry["handle"].is_string())),
        "{sealed}"
    );
    assert_eq!(result["kept"]["mark"]["moved"], true, "{sealed}");

    git(root, &["commit", "-qam", "reflection's fix and rule"])?;
    let next = reflect(root, &endpoint)?;
    assert_eq!(next["status"], "sealed", "{next}");
    let survey = &next["context"]["step_outputs"]["survey"];
    assert_eq!(survey["resistance"], serde_json::json!([]), "{next}");
    assert_eq!(survey["unheld"], serde_json::json!([]), "{next}");
    let head = git(root, &["rev-parse", "HEAD"])?;
    let disable = &next["result"]["kept"]["disable"][0];
    assert_eq!(disable["handle"], handle.as_str(), "{next}");
    assert!(
        disable["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(&head)),
        "{next}"
    );
    assert_eq!(next["result"]["verdict"]["verdict"], "met", "{next}");

    let declined = tempdir()?;
    let root = declined.path();
    record(root)?;
    let fix_or_rule = |request: &serde_json::Value| {
        // The fix is propose's write; the rule is promote's.
        if request["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("propose/"))
        {
            serde_json::json!({"approved": false, "reason": "Fix the caller, not greet."})
        } else {
            serde_json::json!(true)
        }
    };
    let sealed = drive(root, &endpoint, reflect(root, &endpoint)?, fix_or_rule)?;
    let proposals = sealed["result"]["proposals"]
        .as_array()
        .ok_or("proposals")?;
    assert!(
        proposals
            .iter()
            .any(|entry| entry["declined"] == "Fix the caller, not greet."),
        "{sealed}"
    );
    assert!(
        !std::fs::read_to_string(root.join("skills/greet/run.yaml"))?
            .contains("{expected: b, actual: b}")
    );
    assert!(!root.join("skills/greet/tests").exists());
    // Another reader over the same record asks nothing: the decline stands
    // for that evidence.
    let weekly = reflect_as(root, &endpoint, "weekly")?;
    assert_eq!(weekly["status"], "sealed", "{weekly}");
    assert!(
        weekly["result"]["proposals"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|entry| entry["declined"] == "Fix the caller, not greet."
                && entry["again"] == false),
        "{weekly}"
    );
    model.finish()?;
    Ok(())
}

/// Reflection's walls, whatever its term allows: it reads no personal
/// memory, so a person's own rule is never promoted or named, and it writes
/// no person's words, so every said: entry is where it was after a whole
/// reflection the person approves.
#[cfg(all(feature = "agent", feature = "scripted-model"))]
#[test]
fn reflection_reads_no_personal_memory_and_writes_no_persons_words() -> TestResult {
    let model = reflection_model(Diagnoses::Faithful)?;
    let endpoint = model.endpoint().to_owned();
    let temp = tempdir()?;
    let root = temp.path();
    record(root)?;
    let everything = ["runx/memory/said:*", "runx/memory/observed:*"];
    let mine = run(
        root,
        root.join("skills/rules"),
        "keep",
        serde_json::json!({"text": "I greet in Australian English.", "level": "personal"}),
        Some(
            serde_json::json!({"delegations": [{"connector": "private", "terms": [{"verbs": ["read", "write"],
            "bounds": {"operations": ["private/*"], "scopes": ["store.read", "store.append"], "targets": everything, "packages": ["project:rules", "runx/memory"]}}]}]}),
        ),
    )?;
    let personal = answer(root, &mine, serde_json::json!(true))?;
    let personal = personal["result"]["handle"]
        .as_str()
        .ok_or("the person's own rule")?
        .to_owned();
    // Every level the term could reach, read and write.
    let widened = serde_json::json!({"delegations": [{"connector": "private", "terms": [{"verbs": ["read", "write"],
        "bounds": {"operations": ["private/*"], "scopes": ["store.read", "store.append"], "targets": everything, "packages": ["project:reflect", "runx/memory"]}}]}]});
    let mut environment = env(root);
    environment.extend(runx_runtime::scripted_model::managed_agent_env(
        &endpoint,
        "fixture-model",
    ));
    let started = LocalOrchestrator::with_effects_and_environment(
        runx_runtime::RuntimeEffectRegistry::local()?,
        environment.clone(),
    )
    .run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: Some(serde_json::from_value(widened)?),
            act: None,
            skill_path: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/reflect"),
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: serde_json::from_value(
                serde_json::json!({"reader": "nightly", "until": "2099-01-01T00:00:00Z"}),
            )?,
            env: environment,
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::inline(4)?,
        },
        Some("reflect"),
        None,
        None,
    )?;
    let sealed = drive(
        root,
        &endpoint,
        serde_json::to_value(&started.output)?,
        |_| serde_json::json!(true),
    )?;
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    assert!(
        !sealed.to_string().contains(&personal),
        "a person's own rule was read: {sealed}"
    );
    assert!(!std::fs::read_to_string(root.join("AGENTS.md"))?.contains("Australian English"));
    // No said: entry moved: each is current at its first version.
    let index = run(
        root,
        root.join("skills/rules"),
        "index",
        serde_json::json!({}),
        Some(
            serde_json::json!({"delegations": [{"connector": "private", "terms": [{"verbs": ["read"],
            "bounds": {"operations": ["private/*"], "scopes": ["store.read"], "targets": everything, "packages": ["project:rules", "runx/memory"]}}]}]}),
        ),
    )?;
    let entries = index["result"]["entries"].to_string();
    assert!(
        entries.contains(&personal),
        "the person's own rule is current: {index}"
    );
    assert!(!entries.contains("memory.disabled"), "{index}");
    let promoted = sealed["result"]["promotion"]["promoted"][0]["handle"]
        .as_str()
        .ok_or("the workspace's rule")?;
    assert!(promoted.ends_with("@1"), "{promoted}");
    model.finish()?;
    Ok(())
}
