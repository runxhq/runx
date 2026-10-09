use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{JsonObject, JsonValue};
use runx_runtime::{
    LocalOrchestrator, RunResult, SkillRunRequest, TurnAction, TurnCall, TurnRequest, TurnTarget,
};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A skill whose one runner is one agent turn: it may digest data and run a
/// child skill that does the same.
fn write_turn_skill(root: &Path) -> TestResult<PathBuf> {
    crate::support::write_test_skill_package(
        &root.join("digest-child"),
        "---\nname: digest-child\ndescription: Digests a value.\n---\n# Digest child\n\nDigest the value.\n",
        r#"skill: digest-child
runners:
  digest:
    default: true
    inputs:
      value: { type: string, required: true }
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: $input.value
          encoding: utf8_text
"#,
    )?;
    let skill_dir = root.join("turn-demo");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: turn-demo\ndescription: Answers with what it digested.\n---\n# Turn demo\n\nDigest what you need, then answer.\n",
        r#"skill: turn-demo
tools: [data.digest]
skills: [../digest-child]
runners:
  answer:
    default: true
    inputs:
      question: { type: string, required: true }
    outputs:
      answer: string
"#,
    )?;
    Ok(skill_dir)
}

struct Turn {
    root: PathBuf,
    skill_dir: PathBuf,
    receipts: PathBuf,
    run_id: String,
}

impl Turn {
    fn open(root: &Path) -> TestResult<Self> {
        let skill_dir = write_turn_skill(root)?;
        let receipts = root.join("receipts");
        let started = run(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            skill_path: skill_dir.clone(),
            receipt_dir: Some(receipts.clone()),
            run_id: None,
            answers_path: None,
            inputs: BTreeMap::from([(
                "question".to_owned(),
                JsonValue::String("what digests?".to_owned()),
            )]),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: Default::default(),
        })?;
        let output = object(&started.output)?;
        assert_eq!(text(output, "status"), Some("needs_agent"));
        let request = output
            .get("requests")
            .and_then(JsonValue::as_array)
            .and_then(|requests| requests.first())
            .and_then(JsonValue::as_object)
            .ok_or("the run asks for its turn")?;
        assert_eq!(text(request, "id"), Some("agent.answer.output"));
        let allowance = request
            .get("invocation")
            .and_then(JsonValue::as_object)
            .and_then(|invocation| invocation.get("allowance"))
            .ok_or("the turn carries its allowance")?;
        assert_eq!(
            serde_json::to_value(allowance)?,
            serde_json::json!({"tools": ["data.digest"], "skills": ["../digest-child"]})
        );
        Ok(Self {
            root: root.to_path_buf(),
            skill_dir,
            receipts,
            run_id: text(output, "run_id").ok_or("missing run_id")?.to_owned(),
        })
    }

    fn act(&self, action: TurnAction) -> TestResult<JsonObject> {
        let result = LocalOrchestrator::default().act_on_turn(&TurnRequest {
            run_id: self.run_id.clone(),
            turn: Some("agent.answer.output".to_owned()),
            action,
            receipt_dir: Some(self.receipts.clone()),
            env: env(&self.root),
            cwd: self.root.clone(),
        })?;
        assert!(result.needs_resolution(), "the run still waits on its turn");
        Ok(object(&result.output)?.clone())
    }

    fn call(&self, target: TurnTarget, input: serde_json::Value) -> TestResult<JsonObject> {
        let output = self.act(TurnAction::Call(TurnCall {
            target,
            input: serde_json::from_value(input)?,
        }))?;
        assert_eq!(text(&output, "schema"), Some("runx.turn.call.v1"));
        Ok(output
            .get("call")
            .and_then(JsonValue::as_object)
            .ok_or("missing call")?
            .clone())
    }

    fn resume(&self, answers: serde_json::Value) -> TestResult<RunResult> {
        let path = self.root.join("answers.json");
        fs::write(&path, answers.to_string())?;
        run(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            skill_path: self.skill_dir.clone(),
            receipt_dir: Some(self.receipts.clone()),
            run_id: Some(self.run_id.clone()),
            answers_path: Some(path),
            inputs: BTreeMap::new(),
            env: env(&self.root),
            cwd: self.root.clone(),
            managed_agent: Default::default(),
        })
    }
}

#[test]
fn an_agent_turn_records_its_calls_and_seals_them_with_its_answer() -> TestResult {
    let temp = tempdir()?;
    let turn = Turn::open(temp.path())?;

    let digest = turn.call(
        TurnTarget::Tool {
            tool: "data.digest".to_owned(),
        },
        serde_json::json!({"value": "hello", "encoding": "utf8_text"}),
    )?;
    assert_eq!(text(&digest, "status"), Some("completed"), "{digest:?}");
    assert_eq!(
        digest.get("index").map(serde_json::to_value).transpose()?,
        Some(serde_json::json!(0))
    );
    assert!(digest.contains_key("output"));

    let refused = turn.call(
        TurnTarget::Tool {
            tool: "fs.read".to_owned(),
        },
        serde_json::json!({"path": "SKILL.md"}),
    )?;
    assert_eq!(text(&refused, "status"), Some("refused"));
    assert!(
        text(&refused, "reason").is_some_and(|reason| reason.contains("allowance")),
        "{refused:?}"
    );

    let child = turn.call(
        TurnTarget::Skill {
            skill: "../digest-child".to_owned(),
        },
        serde_json::json!({"value": "child"}),
    )?;
    assert_eq!(text(&child, "status"), Some("completed"), "{child:?}");

    let asked = turn.call(TurnTarget::Ask, serde_json::json!({"reason": "Send it?"}))?;
    assert_eq!(text(&asked, "status"), Some("pending"), "{asked:?}");
    let gate = text(&asked, "request_id").ok_or("the ask names its request")?;
    assert_eq!(gate, "answer/3");

    // The human's answer settles the pending call; the turn stays open.
    let settled = turn.resume(serde_json::json!({"approvals": {gate: true}}))?;
    assert!(settled.needs_resolution());

    let status = turn.act(TurnAction::Status)?;
    assert_eq!(text(&status, "request_id"), Some("agent.answer.output"));
    let statuses = status
        .get("calls")
        .and_then(JsonValue::as_array)
        .ok_or("missing calls")?
        .iter()
        .map(|call| call.as_object().and_then(|call| text(call, "status")))
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        [
            Some("completed"),
            Some("refused"),
            Some("completed"),
            Some("completed")
        ]
    );

    let sealed = turn.resume(serde_json::json!({
        "answers": {"agent.answer.output": {"answer": "digested"}}
    }))?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    let receipt_id = text(output, "receipt_id").ok_or("missing receipt_id")?;
    let graph = crate::support::read_test_signed_receipt(&turn.receipts, receipt_id)?;
    let step = crate::support::read_test_first_step_receipt(&turn.receipts, &graph)?;
    let children = step
        .lineage
        .as_ref()
        .map(|lineage| lineage.children.len())
        .unwrap_or_default();
    assert_eq!(children, 3, "the turn's receipt names its completed calls");
    Ok(())
}

#[test]
fn a_turn_action_needs_a_run_waiting_on_a_turn() -> TestResult {
    let temp = tempdir()?;
    let turn = Turn::open(temp.path())?;
    turn.resume(serde_json::json!({
        "answers": {"agent.answer.output": {"answer": "done"}}
    }))?;

    let error = LocalOrchestrator::default()
        .act_on_turn(&TurnRequest {
            run_id: turn.run_id.clone(),
            turn: None,
            action: TurnAction::Status,
            receipt_dir: Some(turn.receipts.clone()),
            env: env(&turn.root),
            cwd: turn.root.clone(),
        })
        .err()
        .ok_or("a sealed run has no open turn")?;
    assert!(error.to_string().contains("not paused"), "{error}");
    Ok(())
}

#[test]
fn a_turn_describes_its_tools_and_completes_with_its_output() -> TestResult {
    let temp = tempdir()?;
    let turn = Turn::open(temp.path())?;

    let described = turn.act(TurnAction::Describe)?;
    assert_eq!(text(&described, "schema"), Some("runx.turn.tools.v1"));
    assert!(text(&described, "instructions").is_some_and(|text| text.contains("Turn demo")));
    let tools = described
        .get("tools")
        .and_then(JsonValue::as_array)
        .ok_or("missing tools")?;
    let names = tools
        .iter()
        .filter_map(|tool| tool.as_object().and_then(|tool| text(tool, "name")))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "data_digest",
            "skill__digest-child",
            "ask_human",
            "turn_status",
            "turn_complete"
        ]
    );
    let digest_schema = serde_json::to_value(
        tools[0]
            .as_object()
            .and_then(|tool| tool.get("input_schema"))
            .ok_or("missing input schema")?,
    )?;
    assert!(
        digest_schema["properties"]["value"].is_object(),
        "{digest_schema}"
    );
    let complete_schema = serde_json::to_value(
        tools[4]
            .as_object()
            .and_then(|tool| tool.get("input_schema"))
            .ok_or("missing input schema")?,
    )?;
    assert!(
        complete_schema["properties"]["answer"].is_object(),
        "{complete_schema}"
    );

    let completed = LocalOrchestrator::default().act_on_turn(&TurnRequest {
        run_id: turn.run_id.clone(),
        turn: Some("agent.answer.output".to_owned()),
        action: TurnAction::Complete(serde_json::from_value(
            serde_json::json!({"answer": "complete"}),
        )?),
        receipt_dir: Some(turn.receipts.clone()),
        env: env(&turn.root),
        cwd: turn.root.clone(),
    })?;
    let output = object(&completed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    Ok(())
}

#[test]
fn a_turn_request_for_another_turn_is_refused() -> TestResult {
    let temp = tempdir()?;
    let turn = Turn::open(temp.path())?;
    let error = LocalOrchestrator::default()
        .act_on_turn(&TurnRequest {
            run_id: turn.run_id.clone(),
            turn: Some("agent.other.output".to_owned()),
            action: TurnAction::Status,
            receipt_dir: Some(turn.receipts.clone()),
            env: env(&turn.root),
            cwd: turn.root.clone(),
        })
        .err()
        .ok_or("a stale turn is refused")?;
    assert!(error.to_string().contains("now waits on"), "{error}");
    Ok(())
}

fn run(request: SkillRunRequest) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().run_skill(&request)?)
}

fn env(root: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned())]);
    crate::support::insert_test_signing_env(&mut env);
    env
}

fn object(value: &JsonValue) -> TestResult<&JsonObject> {
    Ok(value.as_object().ok_or("expected an object")?)
}

fn text<'a>(object: &'a JsonObject, field: &str) -> Option<&'a str> {
    object.get(field).and_then(JsonValue::as_str)
}
