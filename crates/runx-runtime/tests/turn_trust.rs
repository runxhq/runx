//! A load adds no capability by where a file sits (D12), and a person's trust
//! is theirs, kept in their runx home (identity contract, "Trust").
//!
//! The probe of 6 November, as regression tests: a session started as
//! Fieldwork starts one, the bundled agent skill's `read` method with
//! `effects: ask`, `reach: workspace` and no standing approval, in a
//! workspace holding a connector whose `read` is a process that writes a
//! marker, and a skill that reaches it. No network, no credential, no model.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{JsonObject, ResolutionRequest, TurnCallOutcome, TurnCallResult};
use runx_runtime::{
    LocalOrchestrator, ManagedAgentPolicy, ResumeRequest, RunResult, SkillRunRequest, TurnRequest,
    TurnSession,
};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The turn of the agent skill's `read` method.
const TURN: &str = "read";

const MARKER_CONNECTOR: &str = r#"schema: runx.connector.v1
name: marker
title: Marker
description: Writes a marker file through a process, while its operation declares it reads.
auth:
  none: {}
placements:
  local:
    auth: [none]
operations:
  marker.write:
    description: Write a marker at the path given.
    verb: read
    scopes: [marker.read]
    target:
      template: {"$argument": "/path"}
    input_schema:
      type: object
      required: [path]
      additionalProperties: false
      properties:
        path: {type: string, minLength: 1}
    result_fields: [wrote]
    implementation:
      process:
        tool: marker.write
"#;

const MARKER_TOOL: &str = r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "marker.write",
  "version": "0.1.0",
  "description": "Write a marker file: the marker connector's marker.write implementation.",
  "process": {
    "command": "sh",
    "args": [
      "./write.sh",
      "{{path}}"
    ]
  },
  "inputs": {
    "path": {"type": "string", "description": "Where the marker goes."}
  }
}
"#;

const MARKER_SCRIPT: &str =
    "#!/bin/sh\nset -eu\nprintf 'ran\\n' > \"$1\"\nprintf '{\"wrote\":\"%s\"}\\n' \"$1\"\n";

/// A workspace holding the marker connector, `skills/reach`, which reaches
/// it, and `skills/own`, which reaches no connector and runs a process.
struct Workspace {
    root: PathBuf,
    /// The runx home its runs use; none leaves the home to fall back.
    home: Option<PathBuf>,
}

impl Workspace {
    fn new(root: &Path, home: Option<&Path>) -> TestResult<Self> {
        let tool = root.join("connectors/marker/tools/marker/write");
        std::fs::create_dir_all(&tool)?;
        std::fs::write(
            root.join("connectors/marker/connector.yaml"),
            MARKER_CONNECTOR,
        )?;
        std::fs::write(tool.join("manifest.json"), MARKER_TOOL)?;
        std::fs::write(tool.join("write.sh"), MARKER_SCRIPT)?;
        crate::support::write_test_skill_package(
            &root.join("skills/reach"),
            "---\nname: reach\ndescription: Marks the workspace through the marker connector.\n---\n# Reach\n\nMark the path given.\n",
            "version: \"0.1.0\"\nminimum_runx: 0.12.0\nmethods:\n  mark:\n    inputs:\n      path: {type: string}\n    steps:\n      - id: mark\n        operation: marker/marker.write\n        inputs: {path: $input.path}\n",
        )?;
        let own = root.join("skills/own");
        crate::support::write_test_skill_package(
            &own,
            "---\nname: own\ndescription: Runs a process of the project's own.\n---\n# Own\n\nRun it.\n",
            "version: \"0.1.0\"\nminimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: run\n        tool: {command: sh, args: [./run.sh]}\n",
        )?;
        std::fs::write(
            own.join("run.sh"),
            format!(
                "#!/bin/sh\nprintf 'ran\\n' > '{}'\nprintf '{{}}'\n",
                root.join("own-marker").display()
            ),
        )?;
        let marks = root.join("skills/marks");
        crate::support::write_test_skill_package(
            &marks,
            "---\nname: marks\ndescription: Marks the workspace from a script that changes nothing.\n---\n# Marks\n\nMark the path given.\n",
            "version: \"0.1.0\"\nminimum_runx: 0.12.0\nmethods:\n  mark:\n    inputs:\n      path: {type: string}\n    steps:\n      - id: mark\n        script: {module: mark.mjs}\n        inputs: {path: $input.path}\n        calls:\n          operations: [marker/marker.write]\n        max_calls: 1\n        effects: none\n        outputs:\n          wrote: {type: string}\n",
        )?;
        std::fs::write(
            marks.join("mark.mjs"),
            "export default async function (ctx, { path }) {\n  const made = await ctx.operation(\"marker/marker.write\", { path });\n  return { wrote: made.provider_operation.result.wrote };\n}\n",
        )?;
        Ok(Self {
            root: root.to_path_buf(),
            home: home.map(Path::to_path_buf),
        })
    }

    /// Where the marker connector's process writes.
    fn marker(&self) -> PathBuf {
        self.root.join("marker")
    }

    /// The environment of the workspace's runs, and nothing of the test
    /// process's: no `HOME`, no credential, and a registry on a loopback port
    /// nothing listens on, so no load reaches a network.
    fn env(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::from([
            (
                "RUNX_CWD".to_owned(),
                self.root.to_string_lossy().into_owned(),
            ),
            (
                "RUNX_REGISTRY_URL".to_owned(),
                "http://127.0.0.1:9".to_owned(),
            ),
        ]);
        if let Some(home) = &self.home {
            env.insert("RUNX_HOME".to_owned(), home.to_string_lossy().into_owned());
        }
        crate::support::insert_test_signing_env(&mut env);
        env
    }

    fn orchestrator(&self) -> TestResult<LocalOrchestrator> {
        Ok(LocalOrchestrator::with_effects_and_environment(
            runx_runtime::RuntimeEffectRegistry::local()?,
            self.env(),
        ))
    }

    /// Start the agent skill's `read` method as Fieldwork starts a session:
    /// agent bounds, no standing approval. Return its run's id.
    fn start(&self) -> TestResult<String> {
        self.start_as(TURN)
    }

    /// Start the agent skill's `method` the same way. Its turn is named for
    /// the method.
    fn start_as(&self, method: &str) -> TestResult<String> {
        let started = self.orchestrator()?.run_skill_with_binding(
            &SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: Some(serde_json::from_value(
                    serde_json::json!({"effects": "ask", "reach": "workspace"}),
                )?),
                act: Some(serde_json::from_value(serde_json::json!({
                    "purpose": "Answer the person's question about the project."
                }))?),
                skill_path: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/agent"),
                receipt_dir: Some(self.root.join("receipts")),
                run_id: None,
                answers: None,
                inputs: BTreeMap::new(),
                env: self.env(),
                cwd: self.root.clone(),
                managed_agent: ManagedAgentPolicy::HostDriven,
            },
            Some(method),
            None,
            None,
        )?;
        started
            .output
            .as_object()
            .and_then(|output| output.get("run_id"))
            .and_then(runx_contracts::JsonValue::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "the run names its id".into())
    }

    fn open(&self, run_id: &str) -> TestResult<TurnSession> {
        Ok(self.orchestrator()?.open_turns(&TurnRequest {
            run_id: run_id.to_owned(),
            receipt_dir: Some(self.root.join("receipts")),
            env: self.env(),
            cwd: self.root.clone(),
        })?)
    }

    /// The person answers the approval at `address`.
    fn answer(&self, run_id: &str, address: &str, approved: bool) -> TestResult<RunResult> {
        let env = self.env();
        Ok(self.orchestrator()?.resume(&ResumeRequest {
            run_id: run_id.to_owned(),
            answers: crate::support::bind_resume(
                &env,
                &self.root,
                run_id,
                serde_json::json!({"approvals": {address: approved}}),
            )?,
            receipt_dir: Some(self.root.join("receipts")),
            env,
            cwd: self.root.clone(),
            managed_agent: ManagedAgentPolicy::HostDriven,
            package_digest: None,
            closure_digest: None,
        })?)
    }

    /// The `index`th call of the run's turn, as recorded now.
    fn recorded(&self, run_id: &str, index: usize) -> TestResult<TurnCallResult> {
        self.open(run_id)?
            .status(TURN)?
            .calls
            .get(index)
            .cloned()
            .ok_or_else(|| format!("the turn recorded call {index}").into())
    }
}

fn input(value: serde_json::Value) -> TestResult<JsonObject> {
    Ok(serde_json::from_value(value)?)
}

fn load(turn: &TurnSession, target: serde_json::Value) -> TestResult<TurnCallResult> {
    load_in(turn, TURN, target)
}

/// A load at the turn named `at`.
fn load_in(turn: &TurnSession, at: &str, target: serde_json::Value) -> TestResult<TurnCallResult> {
    Ok(turn.call(at, "load", None, input(target)?)?)
}

fn completed(call: &TurnCallResult) -> bool {
    matches!(call.outcome, TurnCallOutcome::Completed { .. })
}

/// The one approval `call` waits on, of gate type `gate_type`.
fn waits_on(call: &TurnCallResult, gate_type: &str) -> TestResult<runx_contracts::ApprovalGate> {
    let TurnCallOutcome::Pending { waiting } = &call.outcome else {
        return Err(format!("the call waits on the person: {call:?}").into());
    };
    match waiting.as_slice() {
        [ResolutionRequest::Approval { gate, .. }]
            if gate.gate_type.as_deref() == Some(gate_type) =>
        {
            Ok(gate.as_ref().clone())
        }
        _ => Err(format!("one {gate_type} approval: {waiting:?}").into()),
    }
}

fn refused(call: &TurnCallResult) -> TestResult<String> {
    match &call.outcome {
        TurnCallOutcome::Refused { reason } => Ok(reason.as_str().to_owned()),
        _ => Err(format!("the call was refused: {call:?}").into()),
    }
}

fn declined(call: &TurnCallResult) -> TestResult<String> {
    match &call.outcome {
        TurnCallOutcome::Declined { reason } => Ok(reason.as_str().to_owned()),
        _ => Err(format!("the person declined the call: {call:?}").into()),
    }
}

/// The closures a trust approval covers.
fn closures(gate: &runx_contracts::ApprovalGate) -> TestResult<Vec<String>> {
    let payload = serde_json::to_value(gate.payload.as_ref().ok_or("the gate's payload")?)?;
    Ok(serde_json::from_value(payload["closures"].clone())?)
}

/// A run of `skills/marks`, whose script under `effects: none` reads
/// through the marker connector, as it ended or paused.
fn mark_under_none(ws: &Workspace) -> TestResult<serde_json::Value> {
    let ran = ws.orchestrator()?.run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: ws.root.join("skills/marks"),
            receipt_dir: Some(ws.root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::from([(
                "path".to_owned(),
                runx_contracts::JsonValue::String(ws.marker().to_string_lossy().into_owned()),
            )]),
            env: ws.env(),
            cwd: ws.root.clone(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        Some("mark"),
        None,
        None,
    )?;
    Ok(serde_json::to_value(&ran.output)?)
}

/// Where a home keeps the person's trust of `closure`.
fn trust_record(home: &Path, closure: &str) -> PathBuf {
    home.join("trust")
        .join(format!("{}.json", closure.trim_start_matches("sha256:")))
}

fn call_marker(turn: &TurnSession, ws: &Workspace) -> TestResult<TurnCallResult> {
    Ok(turn.call(
        TURN,
        "marker__marker_write",
        None,
        input(serde_json::json!({"input": {"path": ws.marker().to_string_lossy()}}))?,
    )?)
}

#[test]
fn a_project_connector_waits_on_trust_and_runs_once_trusted() -> TestResult {
    let temp = tempdir()?;
    let home = temp.path().join("home");
    let ws = Workspace::new(&temp.path().join("ws"), Some(&home))?;

    let run_id = ws.start()?;
    let turn = ws.open(&run_id)?;
    let held = load(&turn, serde_json::json!({"connector": "marker"}))?;
    let gate = waits_on(&held, "trust")?;
    assert!(
        gate.reason
            .as_str()
            .contains("the turn does not already reach"),
        "{gate:?}"
    );
    let [closure] = closures(&gate)?.try_into().map_err(|closures| {
        format!("a connector's trust covers its one implementation: {closures:?}")
    })?;
    assert!(
        !turn
            .status(TURN)?
            .tools
            .iter()
            .any(|tool| tool.name.as_str() == "marker__marker_write"),
        "nothing it declares is offered before the person trusts it"
    );
    assert!(!ws.marker().exists(), "no process ran");
    drop(turn);

    ws.answer(&run_id, "read/0/trust", true)?;
    assert!(
        completed(&ws.recorded(&run_id, 0)?),
        "trusted, the load binds"
    );
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(trust_record(&home, &closure))?)?;
    assert_eq!(record["closure_digest"], closure.as_str());
    assert_eq!(record["package"], "connector marker");
    assert!(
        record.get("tier").is_none(),
        "the project's own has no tier: {record}"
    );
    assert!(
        !ws.root.join(".runx/trust").exists(),
        "nothing about trust is written in the workspace"
    );

    let turn = ws.open(&run_id)?;
    let ran = call_marker(&turn, &ws)?;
    assert!(completed(&ran), "{ran:?}");
    assert_eq!(std::fs::read_to_string(ws.marker())?, "ran\n");
    drop(turn);

    // Trust covers the implementation: a changed process under the same
    // connector.yaml is another closure, and waits again.
    std::fs::write(
        ws.root
            .join("connectors/marker/tools/marker/write/write.sh"),
        MARKER_SCRIPT.replace("ran", "ran again"),
    )?;
    let turn = ws.open(&ws.start()?)?;
    let changed = waits_on(
        &load(&turn, serde_json::json!({"connector": "marker"}))?,
        "trust",
    )?;
    assert_ne!(closures(&changed)?, [closure]);
    Ok(())
}

/// Trust covers the implementation for as long as the turn runs: a loaded
/// connector's calls run only against the exact binding the load recorded,
/// so a process changed under the same connector.yaml after the load runs
/// nothing until the connector is loaded, and trusted, again.
#[test]
fn a_loaded_connector_runs_only_as_it_was_loaded() -> TestResult {
    let temp = tempdir()?;
    let ws = Workspace::new(&temp.path().join("ws"), Some(&temp.path().join("home")))?;
    let run_id = ws.start()?;
    let turn = ws.open(&run_id)?;
    waits_on(
        &load(&turn, serde_json::json!({"connector": "marker"}))?,
        "trust",
    )?;
    drop(turn);
    ws.answer(&run_id, "read/0/trust", true)?;

    std::fs::write(
        ws.root
            .join("connectors/marker/tools/marker/write/write.sh"),
        MARKER_SCRIPT.replace("ran", "ran again"),
    )?;
    let turn = ws.open(&run_id)?;
    let reason = refused(&call_marker(&turn, &ws)?)?;
    assert!(reason.contains("changed since it was loaded"), "{reason}");
    assert!(!ws.marker().exists(), "the changed process ran nothing");

    // Loaded again, it is another implementation to trust, and runs once
    // trusted.
    waits_on(
        &load(&turn, serde_json::json!({"connector": "marker"}))?,
        "trust",
    )?;
    drop(turn);
    ws.answer(&run_id, "read/2/trust", true)?;
    let turn = ws.open(&run_id)?;
    let ran = call_marker(&turn, &ws)?;
    assert!(completed(&ran), "{ran:?}");
    assert_eq!(std::fs::read_to_string(ws.marker())?, "ran again\n");
    Ok(())
}

#[test]
fn a_project_skill_reaching_a_connector_waits_on_trust_and_runs_once_trusted() -> TestResult {
    let temp = tempdir()?;
    let ws = Workspace::new(&temp.path().join("ws"), Some(&temp.path().join("home")))?;

    let run_id = ws.start()?;
    let turn = ws.open(&run_id)?;
    let held = load(&turn, serde_json::json!({"skill": "skills/reach"}))?;
    let gate = waits_on(&held, "trust")?;
    assert!(gate.reason.as_str().contains("marker"), "{gate:?}");
    assert!(!ws.marker().exists(), "no process ran");
    drop(turn);

    ws.answer(&run_id, "read/0/trust", true)?;
    assert!(completed(&ws.recorded(&run_id, 0)?));
    let turn = ws.open(&run_id)?;
    let ran = turn.call(
        TURN,
        "skill__reach__mark",
        None,
        input(serde_json::json!({"path": ws.marker().to_string_lossy()}))?,
    )?;
    assert!(completed(&ran), "{ran:?}");
    assert_eq!(std::fs::read_to_string(ws.marker())?, "ran\n");

    // A person who does not trust it refuses the load, and nothing runs.
    std::fs::remove_file(ws.marker())?;
    let run_id = ws.start()?;
    let turn = ws.open(&run_id)?;
    std::fs::write(
        ws.root
            .join("connectors/marker/tools/marker/write/write.sh"),
        MARKER_SCRIPT.replace("ran", "ran again"),
    )?;
    waits_on(
        &load(&turn, serde_json::json!({"skill": "skills/reach"}))?,
        "trust",
    )?;
    drop(turn);
    // The person's no is theirs, never runx's refusal (changed 10 November:
    // it read as refused).
    ws.answer(&run_id, "read/0/trust", false)?;
    declined(&ws.recorded(&run_id, 0)?)?;
    assert!(!ws.marker().exists());
    Ok(())
}

/// A load waiting on the person's trust has already bound its package, and
/// the run's record names it, by its digest, while it waits.
#[test]
fn a_load_waiting_on_trust_reads_back_the_package_it_bound() -> TestResult {
    let temp = tempdir()?;
    let ws = Workspace::new(&temp.path().join("ws"), Some(&temp.path().join("home")))?;
    let digest = runx_runtime::load_validated_skill_package(&ws.root.join("skills/reach"))?
        .package
        .package_digest
        .clone();

    let run_id = ws.start()?;
    let turn = ws.open(&run_id)?;
    waits_on(
        &load(&turn, serde_json::json!({"skill": "skills/reach"}))?,
        "trust",
    )?;
    drop(turn);

    let read = serde_json::to_value(runx_runtime::run_record::read_run(
        &ws.env(),
        &ws.root,
        &run_id,
        None,
    )?)?;
    let load = read["events"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|record| &record["event"])
        .find(|event| event["kind"] == "call" && event["target"]["kind"] == "load")
        .ok_or_else(|| format!("the load is recorded: {read}"))?;
    assert_eq!(load["outcome"]["status"], "pending", "{load}");
    assert_eq!(load["loaded"]["skill"], "skills/reach", "{load}");
    assert_eq!(load["loaded"]["package_digest"], digest.as_str(), "{load}");
    Ok(())
}

#[test]
fn a_project_skill_reaching_only_what_the_turn_reaches_loads_and_the_floor_holds_its_process()
-> TestResult {
    // A write session: a read turn refuses the process outright (D16), so
    // only a turn that may change things shows the floor holding it.
    const WRITE: &str = "write";
    let temp = tempdir()?;
    let ws = Workspace::new(&temp.path().join("ws"), Some(&temp.path().join("home")))?;
    let run_id = ws.start_as(WRITE)?;
    let turn = ws.open(&run_id)?;
    waits_on(
        &load_in(&turn, WRITE, serde_json::json!({"connector": "marker"}))?,
        "trust",
    )?;
    drop(turn);
    ws.answer(&run_id, "write/0/trust", true)?;

    // The turn reaches the marker connector now, so a skill reaching only it
    // adds nothing, and loads at once.
    let turn = ws.open(&run_id)?;
    let reach = load_in(&turn, WRITE, serde_json::json!({"skill": "skills/reach"}))?;
    assert!(completed(&reach), "{reach:?}");

    // A skill reaching no connector loads at once, and its process still
    // waits on the floor.
    let own = load_in(&turn, WRITE, serde_json::json!({"skill": "skills/own"}))?;
    assert!(completed(&own), "{own:?}");
    let run = turn.call(WRITE, "skill__own__run", None, JsonObject::new())?;
    waits_on(&run, "tool")?;
    assert!(!ws.root.join("own-marker").exists(), "the floor held it");
    Ok(())
}

#[test]
fn a_repository_that_ships_trust_gains_nothing() -> TestResult {
    let temp = tempdir()?;
    let trusting = Workspace::new(
        &temp.path().join("trusting"),
        Some(&temp.path().join("one")),
    )?;
    let run_id = trusting.start()?;
    let turn = trusting.open(&run_id)?;
    let gate = waits_on(
        &load(&turn, serde_json::json!({"connector": "marker"}))?,
        "trust",
    )?;
    drop(turn);
    trusting.answer(&run_id, "read/0/trust", true)?;
    let [closure] = closures(&gate)?.try_into().map_err(|_| "one closure")?;
    let record = std::fs::read(trust_record(&temp.path().join("one"), &closure))?;

    // Another workspace ships that exact record in `.runx/trust`, where
    // trust used to live: the person's home holds their trust, not it.
    let shipped = Workspace::new(&temp.path().join("shipped"), Some(&temp.path().join("two")))?;
    let planted = trust_record(&shipped.root.join(".runx"), &closure);
    std::fs::create_dir_all(planted.parent().ok_or("a parent")?)?;
    std::fs::write(&planted, &record)?;
    let turn = shipped.open(&shipped.start()?)?;
    let held = load(&turn, serde_json::json!({"connector": "marker"}))?;
    assert_eq!(closures(&waits_on(&held, "trust")?)?, [closure]);
    drop(turn);

    // With no home of its own, runx's home falls back to `<workspace>/.runx`,
    // where the record now sits; a home that fell back counts for nothing.
    let fallen = Workspace {
        root: shipped.root.clone(),
        home: None,
    };
    let turn = fallen.open(&fallen.start()?)?;
    let reason = refused(&load(&turn, serde_json::json!({"connector": "marker"}))?)?;
    assert!(reason.contains("set HOME"), "{reason}");
    Ok(())
}

#[test]
fn a_home_that_fell_back_neither_reads_nor_records_trust() -> TestResult {
    let temp = tempdir()?;
    let ws = Workspace::new(&temp.path().join("ws"), None)?;
    let turn = ws.open(&ws.start()?)?;
    let reason = refused(&load(&turn, serde_json::json!({"connector": "marker"}))?)?;
    assert!(reason.contains("set HOME"), "{reason}");
    assert!(!ws.root.join(".runx/trust").exists());

    // What reaches only what the turn already reaches asks no trust.
    let own = load(&turn, serde_json::json!({"skill": "skills/own"}))?;
    assert!(completed(&own), "{own:?}");
    Ok(())
}

#[test]
fn trust_recorded_in_a_home_holds_for_the_same_closure_from_another_workspace() -> TestResult {
    let temp = tempdir()?;
    let home = temp.path().join("home");
    let first = Workspace::new(&temp.path().join("first"), Some(&home))?;
    let run_id = first.start()?;
    let turn = first.open(&run_id)?;
    waits_on(
        &load(&turn, serde_json::json!({"connector": "marker"}))?,
        "trust",
    )?;
    drop(turn);
    first.answer(&run_id, "read/0/trust", true)?;

    let second = Workspace::new(&temp.path().join("second"), Some(&home))?;
    let turn = second.open(&second.start()?)?;
    let bound = load(&turn, serde_json::json!({"connector": "marker"}))?;
    assert!(completed(&bound), "{bound:?}");
    let ran = call_marker(&turn, &second)?;
    assert!(completed(&ran), "{ran:?}");
    assert!(second.marker().exists());
    Ok(())
}

/// Under `effects: none` a read a process carries out runs only from a
/// connector that is verified or that a person trusted (D33): its `read` is
/// its author's promise. Untrusted, the project's marker read refuses its
/// run before it starts, and no process runs. Once the person trusts the
/// marker connector, as a turn's load asks them, the same run starts and
/// reads.
#[test]
fn a_process_read_under_none_runs_once_its_connector_is_trusted() -> TestResult {
    let temp = tempdir()?;
    let ws = Workspace::new(&temp.path().join("ws"), Some(&temp.path().join("home")))?;
    let refused = mark_under_none(&ws)?;
    assert_eq!(refused["outcome"], "blocked", "{refused}");
    assert_eq!(
        refused["refusal"]["reason"],
        "the run's effects are none, so it changes nothing: the marker/marker.write read, which a process carries out for a connector neither verified nor trusted, in step mark, is refused before the run starts",
        "{refused}"
    );
    assert!(!ws.marker().exists(), "no process ran");

    let run_id = ws.start()?;
    let turn = ws.open(&run_id)?;
    waits_on(
        &load(&turn, serde_json::json!({"connector": "marker"}))?,
        "trust",
    )?;
    drop(turn);
    ws.answer(&run_id, "read/0/trust", true)?;

    let sealed = mark_under_none(&ws)?;
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    assert_eq!(sealed["outcome"], "completed", "{sealed}");
    assert_eq!(std::fs::read_to_string(ws.marker())?, "ran\n");
    Ok(())
}
