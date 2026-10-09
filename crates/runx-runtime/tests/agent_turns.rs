use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{JsonObject, JsonValue, TurnCallOutcome, TurnStatus, TurnTarget};
use runx_runtime::{
    LocalOrchestrator, ManagedAgentPolicy, ResumeRequest, RunResult, SkillRunRequest, TurnRequest,
    TurnSession,
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

/// A skill that delegates to the one-turn skill: its turn opens in a child run.
fn write_parent_skill(root: &Path) -> TestResult<PathBuf> {
    write_turn_skill(root)?;
    let skill_dir = root.join("turn-parent");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: turn-parent\ndescription: Delegates its question.\n---\n# Turn parent\n\nDelegate the question.\n",
        r#"skill: turn-parent
runners:
  ask:
    default: true
    inputs:
      question: { type: string, required: true }
    steps:
      - id: delegate
        skill: ../turn-demo
        inputs:
          question: $input.question
"#,
    )?;
    Ok(skill_dir)
}

struct Paused {
    root: PathBuf,
    receipts: PathBuf,
    run_id: String,
}

impl Paused {
    /// Start `skill_dir` and stop at its turn.
    fn start(root: &Path, skill_dir: PathBuf) -> TestResult<Self> {
        let receipts = root.join("receipts");
        let started = LocalOrchestrator::default().run_skill(&SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            skill_path: skill_dir,
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
        assert_eq!(requests(&started)?, ["agent.answer.output"]);
        Ok(Self {
            root: root.to_path_buf(),
            receipts,
            run_id: text(object(&started.output)?, "run_id")
                .ok_or("missing run_id")?
                .to_owned(),
        })
    }

    fn open(&self) -> TestResult<TurnSession> {
        Ok(LocalOrchestrator::default().open_turn(&TurnRequest {
            run_id: self.run_id.clone(),
            receipt_dir: Some(self.receipts.clone()),
            env: env(&self.root),
            cwd: self.root.clone(),
        })?)
    }

    fn resume(&self, answers: serde_json::Value) -> TestResult<RunResult> {
        let path = self.root.join("answers.json");
        fs::write(&path, answers.to_string())?;
        Ok(LocalOrchestrator::default().resume(&ResumeRequest {
            run_id: self.run_id.clone(),
            answers_path: path,
            receipt_dir: Some(self.receipts.clone()),
            env: env(&self.root),
            cwd: self.root.clone(),
            managed_agent: ManagedAgentPolicy::HostDriven,
            package_digest: None,
            execution_closure_digest: None,
        })?)
    }

    fn sealed_step_children(&self, sealed: &RunResult) -> TestResult<usize> {
        let output = object(&sealed.output)?;
        assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
        let receipt_id = text(output, "receipt_id").ok_or("missing receipt_id")?;
        let graph = crate::support::read_test_signed_receipt(&self.receipts, receipt_id)?;
        let step = crate::support::read_test_first_step_receipt(&self.receipts, &graph)?;
        Ok(step
            .lineage
            .as_ref()
            .map(|lineage| lineage.children.len())
            .unwrap_or_default())
    }
}

fn input(value: serde_json::Value) -> TestResult<JsonObject> {
    Ok(serde_json::from_value(value)?)
}

fn tool(name: &str) -> TurnTarget {
    TurnTarget::Tool { tool: name.into() }
}

fn statuses(status: &TurnStatus) -> Vec<&'static str> {
    status
        .calls
        .iter()
        .map(|call| match call.outcome {
            TurnCallOutcome::Completed { .. } => "completed",
            TurnCallOutcome::Failed { .. } => "failed",
            TurnCallOutcome::Refused { .. } => "refused",
            TurnCallOutcome::Pending { .. } => "pending",
        })
        .collect()
}

#[test]
fn an_agent_turn_records_its_calls_and_seals_them_with_its_answer() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?)?;
    let turn = paused.open()?;
    assert_eq!(
        serde_json::to_value(&turn.invocation().allowance)?,
        serde_json::json!({"tools": ["data.digest"], "skills": ["../digest-child"]})
    );

    let digest = turn.call(
        tool("data.digest"),
        input(serde_json::json!({"value": "hello", "encoding": "utf8_text"}))?,
    )?;
    assert_eq!(digest.index, 0);
    assert!(
        matches!(&digest.outcome, TurnCallOutcome::Completed { output, .. } if output.contains_key("digest_result")),
        "{digest:?}"
    );

    let refused = turn.call(
        tool("fs.read"),
        input(serde_json::json!({"path": "SKILL.md"}))?,
    )?;
    assert!(
        matches!(&refused.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains("allowance")),
        "{refused:?}"
    );

    let child = turn.call(
        TurnTarget::Skill {
            skill: "../digest-child".into(),
        },
        input(serde_json::json!({"value": "child"}))?,
    )?;
    assert!(
        matches!(child.outcome, TurnCallOutcome::Completed { .. }),
        "{child:?}"
    );

    let asked = turn.call(
        TurnTarget::Ask,
        input(serde_json::json!({"reason": "Send it?"}))?,
    )?;
    let TurnCallOutcome::Pending { request } = &asked.outcome else {
        return Err(format!("an ask waits on the human: {asked:?}").into());
    };
    let gate = serde_json::to_value(request)?["id"]
        .as_str()
        .ok_or("the ask names its request")?
        .to_owned();
    assert_eq!(gate, "answer/3");

    // The paused run lists what the human must answer beside the act.
    let waiting = paused.resume(serde_json::json!({}))?;
    assert_eq!(requests(&waiting)?, ["agent.answer.output", "answer/3"]);

    // The human's answer settles the pending call; the turn stays open.
    let settled = paused.resume(serde_json::json!({"approvals": {gate.clone(): true}}))?;
    assert_eq!(requests(&settled)?, ["agent.answer.output"]);
    let status = turn.status()?;
    assert_eq!(status.request_id.as_str(), "agent.answer.output");
    assert_eq!(
        statuses(&status),
        ["completed", "refused", "completed", "completed"]
    );

    let sealed = turn.complete(input(serde_json::json!({"answer": "digested"}))?)?;
    assert_eq!(
        paused.sealed_step_children(&sealed)?,
        3,
        "the turn's receipt names its completed calls"
    );
    Ok(())
}

#[test]
fn a_turn_inside_a_child_skill_is_worked_at_its_own_step() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_parent_skill(temp.path())?)?;
    let turn = paused.open()?;
    let names = turn
        .tools()
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "data_digest",
            "skill__digest-child",
            "ask_human",
            "turn_status",
            "turn_complete"
        ],
        "the child's turn offers the child's limits"
    );

    let digest = turn.call(
        tool("data.digest"),
        input(serde_json::json!({"value": "nested", "encoding": "utf8_text"}))?,
    )?;
    assert!(
        matches!(digest.outcome, TurnCallOutcome::Completed { .. }),
        "{digest:?}"
    );
    assert_eq!(statuses(&turn.status()?), ["completed"]);

    let sealed = turn.complete(input(serde_json::json!({"answer": "nested"}))?)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(
        output
            .get("result")
            .and_then(JsonValue::as_object)
            .and_then(|result| text(result, "answer")),
        Some("nested")
    );
    Ok(())
}

#[test]
fn a_turn_answer_waits_until_its_calls_settle() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?)?;
    let turn = paused.open()?;
    turn.call(
        TurnTarget::Ask,
        input(serde_json::json!({"reason": "Go ahead?"}))?,
    )?;

    let refused = turn
        .complete(input(serde_json::json!({"answer": "early"}))?)
        .err()
        .ok_or("an answer while a call waits is refused")?;
    assert!(
        refused.to_string().contains("still waits on answer/0"),
        "{refused}"
    );

    paused.resume(serde_json::json!({"approvals": {"answer/0": false}}))?;
    let sealed = turn.complete(input(serde_json::json!({"answer": "after"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 1);
    Ok(())
}

#[test]
fn a_turn_lists_its_tools_with_their_contracts() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?)?;
    let turn = paused.open()?;
    assert!(
        turn.invocation()
            .envelope
            .instructions
            .as_str()
            .contains("Turn demo")
    );
    let schema = |name: &str| -> TestResult<serde_json::Value> {
        Ok(serde_json::to_value(
            &turn
                .tools()
                .iter()
                .find(|tool| tool.name == name)
                .ok_or(format!("missing tool {name}"))?
                .input_schema,
        )?)
    };
    assert!(schema("data_digest")?["properties"]["value"].is_object());
    assert!(schema("skill__digest-child")?["properties"]["value"].is_object());
    assert!(schema("turn_complete")?["properties"]["answer"].is_object());
    Ok(())
}

#[test]
fn a_turn_the_run_has_left_cannot_be_worked() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?)?;
    let turn = paused.open()?;
    paused.resume(serde_json::json!({
        "answers": {"agent.answer.output": {"answer": "done"}}
    }))?;

    let error = turn
        .call(
            tool("data.digest"),
            input(serde_json::json!({"value": "late", "encoding": "utf8_text"}))?,
        )
        .err()
        .ok_or("a sealed run's turn takes no calls")?;
    assert!(
        error.to_string().contains("not waiting on an agent turn"),
        "{error}"
    );
    let error = paused
        .open()
        .err()
        .ok_or("a sealed run has no turn to open")?;
    assert!(error.to_string().contains("not paused"), "{error}");
    Ok(())
}

#[test]
fn an_answer_answers_one_act_even_when_two_steps_share_a_task() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("twice");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: twice\ndescription: Answers twice.\n---\n# Twice\n\nAnswer each step.\n",
        r#"skill: twice
runners:
  answer:
    default: true
    inputs:
      question: { type: string, required: true }
    steps:
      - id: first
        agent: { role: writer, task: answer }
        outputs:
          answer: string
      - id: second
        agent: { role: writer, task: answer }
        outputs:
          answer: string
"#,
    )?;
    let paused = Paused::start(temp.path(), skill_dir)?;
    let first = paused.open()?;
    assert_eq!(first.status()?.step_id.as_str(), "first");
    let next = first.complete(input(serde_json::json!({"answer": "one"}))?)?;
    assert_eq!(
        requests(&next)?,
        ["agent.answer.output"],
        "the second step asks its own act"
    );
    let second = paused.open()?;
    assert_eq!(second.status()?.step_id.as_str(), "second");
    let sealed = second.complete(input(serde_json::json!({"answer": "two"}))?)?;
    assert_eq!(text(object(&sealed.output)?, "status"), Some("sealed"));
    Ok(())
}

#[test]
fn a_call_into_a_skill_whose_agent_shares_the_task_waits_on_that_agent() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    crate::support::write_test_skill_package(
        &root.join("answer-child"),
        "---\nname: answer-child\ndescription: Answers.\n---\n# Answer child\n\nAnswer.\n",
        "skill: answer-child\nrunners:\n  answer:\n    default: true\n    outputs:\n      answer: string\n",
    )?;
    let skill_dir = root.join("answer-parent");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: answer-parent\ndescription: Answers through a child.\n---\n# Answer parent\n\nAsk the child, then answer.\n",
        r#"skill: answer-parent
skills: [../answer-child]
runners:
  answer:
    default: true
    inputs:
      question: { type: string, required: true }
    outputs:
      answer: string
"#,
    )?;
    let paused = Paused::start(root, skill_dir)?;
    let turn = paused.open()?;
    let child = turn.call(
        TurnTarget::Skill {
            skill: "../answer-child".into(),
        },
        JsonObject::new(),
    )?;
    assert!(
        matches!(&child.outcome, TurnCallOutcome::Pending { .. }),
        "the child's act is its own request: {child:?}"
    );

    // An answer settles the pending call first: the turn cannot complete
    // while it waits.
    let settled = paused.resume(serde_json::json!({
        "answers": {"agent.answer.output": {"answer": "from the child"}}
    }))?;
    assert_eq!(requests(&settled)?, ["agent.answer.output"]);
    assert_eq!(statuses(&turn.status()?), ["completed"]);

    let sealed = turn.complete(input(serde_json::json!({"answer": "parent"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 1);
    Ok(())
}

#[test]
fn a_resume_pinned_to_another_package_is_refused() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?)?;
    let path = temp.path().join("answers.json");
    fs::write(&path, "{}")?;
    let error = LocalOrchestrator::default()
        .resume(&ResumeRequest {
            run_id: paused.run_id.clone(),
            answers_path: path,
            receipt_dir: Some(paused.receipts.clone()),
            env: env(temp.path()),
            cwd: temp.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
            package_digest: Some("sha256:another".to_owned()),
            execution_closure_digest: None,
        })
        .err()
        .ok_or("a pinned digest the run did not start under is refused")?;
    assert!(
        error
            .to_string()
            .contains("the package digest sha256:another is not the one run"),
        "{error}"
    );
    Ok(())
}

fn requests(result: &RunResult) -> TestResult<Vec<String>> {
    Ok(result
        .pending_requests
        .iter()
        .map(|request| {
            request
                .as_object()
                .and_then(|request| text(request, "id"))
                .map(str::to_owned)
                .ok_or("a request names its id")
        })
        .collect::<Result<_, _>>()?)
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
