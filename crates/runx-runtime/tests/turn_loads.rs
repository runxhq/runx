//! A turn's loads (agent-sessions §4.1): a catalog rule admits what a load
//! finds, a package below its tier waits on the person's trust once per
//! closure, and what a load bound is what its tools run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{
    JsonObject, JsonValue, LoadedPackage, ResolutionRequest, TrustTier, TurnCallOutcome,
    TurnCallResult, TurnLoad, TurnTarget,
};
use runx_runtime::registry::{FileRegistryStore, IngestSkillOptions, ingest_skill_markdown};
use runx_runtime::{
    LocalOrchestrator, ManagedAgentPolicy, ResumeRequest, RunResult, SkillRunRequest, TurnRequest,
    TurnSession,
};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The act of the session skill's one-turn runner.
const TURN: &str = "work";

/// A runner's body that digests its value.
const DIGEST_RUNNER: &str = "    inputs:\n      value: { type: string, required: true }\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n";

/// A project with a local registry holding `acme/echo` at each tier asked.
struct Project {
    root: PathBuf,
    registry: PathBuf,
}

impl Project {
    fn new(root: &Path) -> TestResult<Self> {
        let registry = root.join("registry");
        std::fs::create_dir_all(&registry)?;
        Ok(Self {
            root: root.to_path_buf(),
            registry,
        })
    }

    /// Publish `acme/<name>@<version>` at `tier`: one runner, `say`, that
    /// digests its value.
    fn publish(&self, name: &str, version: &str, tier: TrustTier, note: &str) -> TestResult {
        self.publish_runners(
            name,
            version,
            tier,
            note,
            &format!("  say:\n    default: true\n{DIGEST_RUNNER}"),
        )
    }

    /// Publish `acme/<name>@<version>` at `tier` with the `runners` of its
    /// profile.
    fn publish_runners(
        &self,
        name: &str,
        version: &str,
        tier: TrustTier,
        note: &str,
        runners: &str,
    ) -> TestResult {
        ingest_skill_markdown(
            &FileRegistryStore::new(&self.registry),
            &format!("---\nname: {name}\ndescription: Says it, digested. {note}\n---\n# {name}\n"),
            IngestSkillOptions {
                owner: Some("acme".to_owned()),
                version: Some(version.to_owned()),
                created_at: Some("2026-10-16T00:00:00Z".to_owned()),
                profile_document: Some(format!("skill: {name}\nrunners:\n{runners}")),
                trust_tier: Some(tier),
                ..IngestSkillOptions::default()
            },
        )?;
        crate::skill_run::sign_registry_version(&self.registry, &format!("acme/{name}"), version)?;
        Ok(())
    }

    /// A session skill whose one turn loads under `rules`.
    fn session(&self, rules: serde_json::Value) -> TestResult<PathBuf> {
        self.session_with(rules, &[])
    }

    /// A session skill whose one turn loads under `rules` and may call
    /// `tools`.
    fn session_with(&self, rules: serde_json::Value, tools: &[&str]) -> TestResult<PathBuf> {
        let skill_dir = self.root.join("session");
        crate::support::write_test_skill_package(
            &skill_dir,
            "---\nname: session\ndescription: Works with what it loads.\n---\n# Session\n\nLoad what you need, then summarize.\n",
            &format!(
                "skill: session\ntools: {}\nskills: {}\nrunners:\n  work:\n    default: true\n    outputs:\n      summary: string\n",
                serde_json::to_string(tools)?,
                serde_json::to_string(&rules)?
            ),
        )?;
        Ok(skill_dir)
    }

    /// A session skill as runx bundles its own: a registry package bundle
    /// carrying no connector, so it reaches only what its turn loads.
    fn bundled_session(&self, rules: serde_json::Value) -> TestResult<PathBuf> {
        let skill_dir = self.session(rules)?;
        std::fs::write(
            skill_dir.join("runx.package.json"),
            r#"{"schema": "runx.registry.package_bundle.v1", "dependencies": [], "connectors": []}"#,
        )?;
        Ok(skill_dir)
    }

    fn env(&self) -> TestResult<BTreeMap<String, String>> {
        let mut env = crate::skill_run::trusted_manifest_env_for_owner("acme", None)?;
        env.insert(
            "RUNX_CWD".to_owned(),
            self.root.to_string_lossy().into_owned(),
        );
        env.insert(
            "RUNX_REGISTRY_DIR".to_owned(),
            self.registry.to_string_lossy().into_owned(),
        );
        crate::support::insert_test_signing_env(&mut env);
        Ok(env)
    }

    /// The orchestrator a CLI run uses, with the local effects.
    fn orchestrator(&self) -> TestResult<LocalOrchestrator> {
        Ok(LocalOrchestrator::with_effects_and_environment(
            runx_runtime::RuntimeEffectRegistry::local()?,
            self.env()?,
        ))
    }

    /// Start the session and stop at its turn.
    fn start(&self, skill_dir: PathBuf) -> TestResult<String> {
        let started = self.orchestrator()?.run_skill(&SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: Some(serde_json::from_value(serde_json::json!({
                "purpose": "Answer the person's question about the project."
            }))?),
            skill_path: skill_dir,
            receipt_dir: Some(self.root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env: self.env()?,
            cwd: self.root.clone(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        })?;
        started
            .output
            .as_object()
            .and_then(|output| output.get("run_id"))
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "the run names its id".into())
    }

    fn open(&self, run_id: &str) -> TestResult<TurnSession> {
        Ok(self.orchestrator()?.open_turns(&TurnRequest {
            run_id: run_id.to_owned(),
            receipt_dir: Some(self.root.join("receipts")),
            env: self.env()?,
            cwd: self.root.clone(),
        })?)
    }

    fn resume(&self, run_id: &str, answers: serde_json::Value) -> TestResult<RunResult> {
        let env = self.env()?;
        Ok(self.orchestrator()?.resume(&ResumeRequest {
            run_id: run_id.to_owned(),
            answers: crate::support::bind_resume(&env, &self.root, run_id, answers)?,
            receipt_dir: Some(self.root.join("receipts")),
            env,
            cwd: self.root.clone(),
            managed_agent: ManagedAgentPolicy::HostDriven,
            package_digest: None,
            execution_closure_digest: None,
        })?)
    }
}

fn input(value: serde_json::Value) -> TestResult<JsonObject> {
    Ok(serde_json::from_value(value)?)
}

fn loaded(call: &TurnCallResult) -> TestResult<TurnLoad> {
    let TurnCallOutcome::Completed { output, .. } = &call.outcome else {
        return Err(format!("the load completed: {call:?}").into());
    };
    Ok(JsonValue::Object(output.clone()).deserialize_into()?)
}

fn refused(call: &TurnCallResult) -> TestResult<String> {
    match &call.outcome {
        TurnCallOutcome::Refused { reason } => Ok(reason.as_str().to_owned()),
        _ => Err(format!("the call was refused: {call:?}").into()),
    }
}

/// The trust approval a pending load waits on, at `address`.
fn trust_gate(call: &TurnCallResult, address: &str) -> TestResult<runx_contracts::ApprovalGate> {
    let TurnCallOutcome::Pending { waiting } = &call.outcome else {
        return Err(format!("the load waits on the person: {call:?}").into());
    };
    match waiting.as_slice() {
        [ResolutionRequest::Approval { id, gate }] if id.as_str() == address => {
            Ok(gate.as_ref().clone())
        }
        _ => Err(format!("one trust approval at {address}: {waiting:?}").into()),
    }
}

fn tool_names(turn: &TurnSession) -> TestResult<Vec<String>> {
    Ok(turn
        .status(TURN)?
        .tools
        .iter()
        .map(|tool| tool.name.as_str().to_owned())
        .collect())
}

#[test]
fn a_catalog_skill_loads_under_its_rule_and_runs_as_it_was_bound() -> TestResult {
    let temp = tempdir()?;
    let project = Project::new(temp.path())?;
    project.publish("echo", "1.0.0", TrustTier::Verified, "")?;
    let run_id =
        project.start(project.session(serde_json::json!([{"catalog": {"trust": "verified"}}]))?)?;
    let turn = project.open(&run_id)?;
    assert_eq!(tool_names(&turn)?, ["load"], "a rule offers the load call");
    assert_eq!(
        turn.status(TURN)?
            .invocation
            .envelope
            .act
            .map(|act| act.purpose.as_str().to_owned())
            .as_deref(),
        Some("Answer the person's question about the project."),
        "the act carries why its run exists"
    );

    let load = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "acme/echo@1.0.0"}))?,
    )?;
    let bound = loaded(&load)?;
    assert!(
        matches!(
            &bound.package,
            LoadedPackage::Skill { skill, trust_tier: Some(TrustTier::Verified), publisher: Some(publisher), .. }
                if skill.as_str() == "acme/echo@1.0.0" && publisher.as_str() == "acme"
        ),
        "{bound:?}"
    );
    let [tool] = bound.tools.as_slice() else {
        return Err(format!("one loaded runner: {bound:?}").into());
    };
    let TurnTarget::Skill {
        skill,
        runner: Some(runner),
        bound: Some(_),
    } = &tool.target
    else {
        return Err(format!("a bound runner: {tool:?}").into());
    };
    assert_eq!(tool.name.as_str(), "skill__acme_echo__say");
    assert!(
        tool.input_schema["properties"]
            .as_object()
            .is_some_and(|properties| properties.contains_key("value")),
        "the load records the runner's inputs: {tool:?}"
    );
    assert_eq!(runner.as_str(), "say");
    assert_eq!(skill.as_str(), ".runx/added-skills/acme/echo/1.0.0");
    // A skill of one runner binds it without naming it, and its manual comes
    // with it at the digest of its exact bytes.
    let [say] = bound.runners.as_slice() else {
        return Err(format!("the skill's one runner: {bound:?}").into());
    };
    assert!(say.bound && say.name.as_str() == "say", "{say:?}");
    let manual = bound
        .manual
        .as_ref()
        .ok_or("a skill's load carries its manual")?;
    let on_disk = std::fs::read(
        project
            .root
            .join(".runx/added-skills/acme/echo/1.0.0/SKILL.md"),
    )?;
    assert_eq!(manual.content.as_bytes(), on_disk.as_slice());
    assert_eq!(
        manual.sha256.as_str(),
        runx_contracts::sha256_prefixed(&on_disk)
    );
    assert_eq!(
        tool_names(&turn)?,
        ["load", "skill__acme_echo__say"],
        "the loaded runner is a tool of the turn"
    );

    let said = turn.call(
        TURN,
        "skill__acme_echo__say",
        None,
        input(serde_json::json!({"value": "hi"}))?,
    )?;
    assert!(
        matches!(said.outcome, TurnCallOutcome::Completed { .. }),
        "{said:?}"
    );

    // What the load bound is what runs: a changed package is refused.
    let manual = project
        .root
        .join(".runx/added-skills/acme/echo/1.0.0/SKILL.md");
    std::fs::write(
        &manual,
        format!("{}\nChanged.\n", std::fs::read_to_string(&manual)?),
    )?;
    let changed = turn.call(
        TURN,
        "skill__acme_echo__say",
        None,
        input(serde_json::json!({"value": "hi"}))?,
    )?;
    assert!(
        refused(&changed)?.contains("is not what the turn loaded"),
        "{changed:?}"
    );
    Ok(())
}

#[test]
fn a_package_below_the_rule_waits_on_trust_once_per_closure() -> TestResult {
    let temp = tempdir()?;
    let project = Project::new(temp.path())?;
    project.publish("rough", "1.0.0", TrustTier::Community, "")?;
    project.publish("rough", "1.0.1", TrustTier::Community, "Revised.")?;
    let session = project.session(serde_json::json!([{"catalog": {"trust": "verified"}}]))?;

    let run_id = project.start(session.clone())?;
    let turn = project.open(&run_id)?;
    let held = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "acme/rough@1.0.0"}))?,
    )?;
    let gate = trust_gate(&held, "work/0/trust")?;
    assert_eq!(gate.gate_type.as_deref(), Some("trust"));
    let payload = serde_json::to_value(
        gate.payload
            .as_ref()
            .ok_or("the gate carries what it trusts")?,
    )?;
    assert_eq!(payload["package"]["skill"], "acme/rough@1.0.0");
    assert_eq!(payload["package"]["trust_tier"], "community");
    assert_eq!(payload["declares"]["runners"], serde_json::json!(["say"]));
    drop(turn);

    project.resume(
        &run_id,
        serde_json::json!({"approvals": {"work/0/trust": true}}),
    )?;
    let turn = project.open(&run_id)?;
    let status = turn.status(TURN)?;
    loaded(status.calls.first().ok_or("the load is recorded")?)?;
    drop(turn);

    // The project trusts that closure now: a later session loads it at once.
    let again = project.start(session.clone())?;
    let turn = project.open(&again)?;
    loaded(&turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "acme/rough@1.0.0"}))?,
    )?)?;

    // Another closure asks again.
    let revised = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "acme/rough@1.0.1"}))?,
    )?;
    trust_gate(&revised, "work/1/trust")?;
    drop(turn);

    // A person who does not trust it refuses the load.
    project.resume(
        &again,
        serde_json::json!({"approvals": {"work/1/trust": false}}),
    )?;
    let turn = project.open(&again)?;
    let status = turn.status(TURN)?;
    refused(status.calls.get(1).ok_or("the second load is recorded")?)?;
    Ok(())
}

#[test]
fn a_skill_of_several_runners_is_read_without_trust_and_binds_the_runner_named() -> TestResult {
    let temp = tempdir()?;
    let project = Project::new(temp.path())?;
    project.publish_runners(
        "voice",
        "1.0.0",
        TrustTier::Community,
        "",
        &format!(
            "  say:\n    default: true\n    description: Say the value as given.\n{DIGEST_RUNNER}  shout:\n    description: Say the value loudly, when the person asks for emphasis.\n{DIGEST_RUNNER}"
        ),
    )?;
    let run_id =
        project.start(project.session(serde_json::json!([{"catalog": {"trust": "verified"}}]))?)?;
    let turn = project.open(&run_id)?;

    // Loaded without a runner, a skill of several binds none: the load reads
    // its manual and runners, binds no closure, and so asks no trust.
    let reading = loaded(&turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "acme/voice@1.0.0"}))?,
    )?)?;
    assert!(reading.tools.is_empty(), "{reading:?}");
    assert!(reading.connectors.is_empty(), "{reading:?}");
    assert!(
        reading
            .manual
            .as_ref()
            .is_some_and(|manual| manual.content.contains("# voice")),
        "{reading:?}"
    );
    let index = reading
        .runners
        .iter()
        .map(|runner| {
            (
                runner.name.as_str(),
                runner.purpose.as_ref().map(|purpose| purpose.as_str()),
                runner.bound,
                runner
                    .inputs
                    .iter()
                    .map(|input| {
                        (
                            input.name.as_str(),
                            input.input_type.as_str(),
                            input.required,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        index,
        [
            (
                "say",
                Some("Say the value as given."),
                false,
                vec![("value", "string", true)]
            ),
            (
                "shout",
                Some("Say the value loudly, when the person asks for emphasis."),
                false,
                vec![("value", "string", true)]
            ),
        ]
    );
    assert_eq!(tool_names(&turn)?, ["load"], "a reading adds no tool");

    // Naming a runner binds it alone, and binding a closure below the rule's
    // tier still waits on the person's trust.
    let held = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "acme/voice@1.0.0", "runner": "shout"}))?,
    )?;
    let gate = trust_gate(&held, "work/1/trust")?;
    let payload = serde_json::to_value(gate.payload.as_ref().ok_or("the gate's payload")?)?;
    assert_eq!(payload["declares"]["runners"], serde_json::json!(["shout"]));
    drop(turn);
    project.resume(
        &run_id,
        serde_json::json!({"approvals": {"work/1/trust": true}}),
    )?;
    let turn = project.open(&run_id)?;
    let status = turn.status(TURN)?;
    let bound = loaded(status.calls.get(1).ok_or("the second load is recorded")?)?;
    let [tool] = bound.tools.as_slice() else {
        return Err(format!("one bound runner: {bound:?}").into());
    };
    assert_eq!(tool.name.as_str(), "skill__acme_voice__shout");
    assert_eq!(
        tool.description.as_str(),
        "Say the value loudly, when the person asks for emphasis.",
        "a bound runner is described by its own purpose"
    );
    assert_eq!(
        bound
            .runners
            .iter()
            .map(|runner| (runner.name.as_str(), runner.bound))
            .collect::<Vec<_>>(),
        [("say", false), ("shout", true)]
    );
    assert_eq!(tool_names(&turn)?, ["load", "skill__acme_voice__shout"]);
    Ok(())
}

#[test]
fn a_rule_admits_only_the_origin_it_names() -> TestResult {
    let temp = tempdir()?;
    let project = Project::new(temp.path())?;
    project.publish("echo", "1.0.0", TrustTier::Verified, "")?;
    crate::support::write_test_skill_package(
        &project.root.join("skills/notes"),
        "---\nname: notes\ndescription: The project's own.\n---\n# Notes\n",
        "skill: notes\nrunners:\n  jot:\n    default: true\n    inputs:\n      value: { type: string, required: true }\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
    )?;

    let run_id = project.start(project.session(serde_json::json!([{"project": true}]))?)?;
    let turn = project.open(&run_id)?;
    let own = loaded(&turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "skills/notes"}))?,
    )?)?;
    assert!(
        matches!(
            &own.package,
            LoadedPackage::Skill {
                trust_tier: None,
                publisher: None,
                ..
            }
        ),
        "the project's own has no tier: {own:?}"
    );
    assert!(tool_names(&turn)?.contains(&"skill__notes__jot".to_owned()));
    let registry = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "acme/echo@1.0.0"}))?,
    )?;
    assert!(
        refused(&registry)?.contains("admit only the project's own"),
        "{registry:?}"
    );
    // The install record keeps the acquired copy the registry's.
    let copy = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": ".runx/added-skills/acme/echo/1.0.0"}))?,
    )?;
    assert!(
        refused(&copy)?.contains("admit only the project's own"),
        "{copy:?}"
    );
    drop(turn);

    let run_id = project
        .start(project.session(serde_json::json!([{"catalog": {"trust": "community"}}]))?)?;
    let turn = project.open(&run_id)?;
    let own = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "skills/notes"}))?,
    )?;
    assert!(refused(&own)?.contains("admit only the catalog"), "{own:?}");
    let malformed = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "skills/notes", "connector": "store"}))?,
    )?;
    refused(&malformed)?;
    Ok(())
}

#[test]
fn a_loaded_connectors_operations_are_tools_of_the_turn() -> TestResult {
    let temp = tempdir()?;
    let project = Project::new(temp.path())?;
    let session = project.session(serde_json::json!([{"catalog": {"trust": "verified"}}]))?;
    loads_and_calls_store(&project, session)
}

#[test]
fn a_bundled_session_calls_the_connector_it_loads_from_the_project() -> TestResult {
    let temp = tempdir()?;
    let project = Project::new(temp.path())?;
    let session =
        project.bundled_session(serde_json::json!([{"catalog": {"trust": "verified"}}]))?;
    loads_and_calls_store(&project, session)
}

/// A turn of `session` loads the `store` connector, is offered its
/// operations, and calls one.
fn loads_and_calls_store(project: &Project, session: PathBuf) -> TestResult {
    let run_id = project.start(session)?;
    let turn = project.open(&run_id)?;

    let bound = loaded(&turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"connector": "store"}))?,
    )?)?;
    assert!(
        matches!(
            &bound.package,
            LoadedPackage::Connector { connector, trust_tier: Some(TrustTier::FirstParty), .. }
                if connector.as_str() == "store"
        ),
        "a runtime capability is runx's: {bound:?}"
    );
    assert!(bound.connectors.contains_key("store"));
    let names = tool_names(&turn)?;
    assert!(names.contains(&"store__heads_list".to_owned()), "{names:?}");

    let heads = turn.call(
        TURN,
        "store__heads_list",
        None,
        input(
            serde_json::json!({"input": {"data_source_ref": "local://board", "resource": "items"}}),
        )?,
    )?;
    assert!(
        matches!(heads.outcome, TurnCallOutcome::Completed { .. }),
        "{heads:?}"
    );
    Ok(())
}

#[test]
fn the_catalog_is_searched_and_inspected_without_adding_to_the_project() -> TestResult {
    let temp = tempdir()?;
    let project = Project::new(temp.path())?;
    project.publish("echo", "1.0.0", TrustTier::Verified, "")?;
    crate::support::write_test_skill_package(
        &project.root.join("skills/notes"),
        "---\nname: notes\ndescription: Takes the project's notes.\n---\n# Notes\n",
        "skill: notes\nrunners:\n  jot:\n    default: true\n    description: Jot one note down.\n    inputs:\n      value: { type: string, required: true }\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
    )?;
    let run_id = project.start(project.session_with(
        serde_json::json!([{"catalog": {"trust": "verified"}}]),
        &["registry.search", "registry.inspect"],
    )?)?;
    let turn = project.open(&run_id)?;

    let found = |query: &str| -> TestResult<serde_json::Value> {
        match turn
            .call(
                TURN,
                "registry_search",
                None,
                input(serde_json::json!({"query": query}))?,
            )?
            .outcome
        {
            TurnCallOutcome::Completed { output, .. } => {
                Ok(serde_json::to_value(output)?["registry_search"]["data"].take())
            }
            outcome => Err(format!("the search completed: {outcome:?}").into()),
        }
    };
    let echo = found("digested")?;
    let hit = echo["hits"]
        .as_array()
        .and_then(|hits| hits.iter().find(|hit| hit["ref"] == "acme/echo@1.0.0"))
        .ok_or_else(|| format!("the registry's skill is found: {echo}"))?;
    assert_eq!(hit["source"], "registry");
    assert_eq!(hit["trust_tier"], "verified");
    // The registry lists its runners by name; a purpose comes with the
    // package's own description.
    assert_eq!(hit["runners"], serde_json::json!([{"name": "say"}]));
    let notes = found("notes")?;
    let hit = notes["hits"]
        .as_array()
        .and_then(|hits| hits.iter().find(|hit| hit["ref"] == "skills/notes"))
        .ok_or("the project's own skill is found")?;
    assert_eq!(hit["source"], "project");
    assert_eq!(
        hit["runners"],
        serde_json::json!([{"name": "jot", "purpose": "Jot one note down."}]),
        "a project skill's runners carry their purposes"
    );

    let inspected = turn.call(
        TURN,
        "registry_inspect",
        None,
        input(serde_json::json!({"ref": "acme/echo@1.0.0"}))?,
    )?;
    let TurnCallOutcome::Completed { output, .. } = &inspected.outcome else {
        return Err(format!("the inspection completed: {inspected:?}").into());
    };
    let inspection: runx_contracts::SkillInspection = serde_json::from_value(
        serde_json::to_value(output)?["registry_inspection"]["data"].take(),
    )?;
    assert_eq!(inspection.name, "echo");
    assert_eq!(
        inspection.runner.map(|runner| runner.name).as_deref(),
        Some("say")
    );
    assert!(
        !project.root.join("skills/acme").exists(),
        "inspecting is not using: nothing is added to the project"
    );

    let store = turn.call(
        TURN,
        "registry_inspect",
        None,
        input(serde_json::json!({"ref": "store"}))?,
    )?;
    let TurnCallOutcome::Completed { output, .. } = &store.outcome else {
        return Err(format!("the connector inspection completed: {store:?}").into());
    };
    let output = serde_json::to_value(output)?["registry_inspection"]["data"].take();
    let operations = output["operations"]
        .as_array()
        .ok_or("a connector inspection lists its operations")?;
    assert!(
        operations.iter().any(
            |operation| operation["operation"] == "heads.list" && operation["access"] == "read"
        ),
        "{operations:?}"
    );
    Ok(())
}

/// Run the runx-maintained `revert` skill over `call_ids` of run `run_id`.
fn revert(project: &Project, run_id: &str, call_ids: &[&str]) -> TestResult<RunResult> {
    let skill = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/revert");
    Ok(project.orchestrator()?.run_skill(&SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill,
        receipt_dir: Some(project.root.join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::from([
            ("run_id".to_owned(), JsonValue::String(run_id.to_owned())),
            (
                "call_ids".to_owned(),
                serde_json::from_value(serde_json::json!(call_ids))?,
            ),
        ]),
        env: project.env()?,
        cwd: project.root.clone(),
        managed_agent: ManagedAgentPolicy::HostDriven,
    })?)
}

/// How the run came out: `completed`, or `failed`.
fn outcome(result: &RunResult) -> Option<&str> {
    result
        .output
        .as_object()
        .and_then(|output| output.get("outcome"))
        .and_then(JsonValue::as_str)
}

#[test]
fn revert_undoes_a_runs_edits_together_or_not_at_all() -> TestResult {
    let temp = tempdir()?;
    let project = Project::new(temp.path())?;
    let readme = project.root.join("README.md");
    let notes = project.root.join("notes.md");
    let edit = |run_id: &str| -> TestResult {
        std::fs::write(&readme, "old\n")?;
        let turn = project.open(run_id)?;
        for (call_id, inputs) in [
            (
                "w1",
                serde_json::json!({
                    "path": "README.md",
                    "contents": "new\n",
                    "expected_digest": runx_contracts::sha256_prefixed(b"old\n"),
                }),
            ),
            (
                "w2",
                serde_json::json!({"path": "notes.md", "contents": "notes\n", "create": true}),
            ),
        ] {
            let written = turn.call(TURN, "fs_write", Some(call_id), input(inputs)?)?;
            assert!(
                matches!(written.outcome, TurnCallOutcome::Completed { .. }),
                "{written:?}"
            );
        }
        turn.complete(
            TURN,
            input(serde_json::json!({"summary": "Edited two files."}))?,
        )?;
        Ok(())
    };
    let session = || project.session_with(serde_json::json!([]), &["fs.write"]);

    let run_id = project.start(session()?)?;
    edit(&run_id)?;
    let undone = revert(&project, &run_id, &["w1", "w2"])?;
    assert_eq!(outcome(&undone), Some("completed"), "{:?}", undone.output);
    assert_eq!(std::fs::read_to_string(&readme)?, "old\n");
    assert!(!notes.exists(), "a file the edits created is deleted");

    // A file changed since the edits is refused, and nothing is restored.
    let run_id = project.start(session()?)?;
    edit(&run_id)?;
    std::fs::write(&readme, "changed by hand\n")?;
    let refused = revert(&project, &run_id, &["w1", "w2"])?;
    assert_eq!(outcome(&refused), Some("failed"), "{:?}", refused.output);
    assert!(
        serde_json::to_string(&refused.output)?.contains("README.md"),
        "the refusal names the file: {:?}",
        refused.output
    );
    assert_eq!(std::fs::read_to_string(&readme)?, "changed by hand\n");
    assert!(
        notes.exists(),
        "nothing is restored when one file is refused"
    );
    Ok(())
}
