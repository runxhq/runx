use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{
    JsonObject, JsonValue, ResolutionRequest, TurnCallOutcome, TurnCallResult, TurnStatus,
};
use runx_runtime::{
    LocalOrchestrator, ManagedAgentPolicy, PackageHarnessRequest, ResumeRequest, RunResult,
    SkillRunRequest, TurnRequest, TurnSession,
};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The act of a one-turn runner named `answer`, at the root.
const TURN: &str = "agent.answer.output";

/// A skill whose one runner is one agent turn: it may digest data, run a
/// child skill that does the same, and ask the human.
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
asks: true
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

/// A one-turn skill whose turn may run `answer-child`, a one-turn skill with
/// the same task; the child may ask the human.
fn write_answer_skills(root: &Path) -> TestResult<PathBuf> {
    crate::support::write_test_skill_package(
        &root.join("answer-child"),
        "---\nname: answer-child\ndescription: Answers.\n---\n# Answer child\n\nAnswer.\n",
        "skill: answer-child\nasks: true\nrunners:\n  answer:\n    default: true\n    outputs:\n      answer: string\n",
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
    Ok(skill_dir)
}

struct Paused {
    root: PathBuf,
    receipts: PathBuf,
    run_id: String,
}

impl Paused {
    /// Start `skill_dir` and stop where it first waits, at `turn`.
    fn start(root: &Path, skill_dir: PathBuf, turn: &str) -> TestResult<Self> {
        Self::start_with(root, skill_dir, turn, ManagedAgentPolicy::HostDriven)
    }

    fn start_with(
        root: &Path,
        skill_dir: PathBuf,
        turn: &str,
        managed_agent: ManagedAgentPolicy,
    ) -> TestResult<Self> {
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
            managed_agent,
        })?;
        assert_eq!(requests(&started)?, [turn]);
        Ok(Self {
            root: root.to_path_buf(),
            receipts,
            run_id: text(object(&started.output)?, "run_id")
                .ok_or("missing run_id")?
                .to_owned(),
        })
    }

    fn open(&self) -> TestResult<TurnSession> {
        Ok(LocalOrchestrator::default().open_turns(&TurnRequest {
            run_id: self.run_id.clone(),
            receipt_dir: Some(self.receipts.clone()),
            env: env(&self.root),
            cwd: self.root.clone(),
        })?)
    }

    fn resume(&self, answers: serde_json::Value) -> TestResult<RunResult> {
        self.resume_with(answers, ManagedAgentPolicy::HostDriven)
    }

    fn resume_with(
        &self,
        answers: serde_json::Value,
        managed_agent: ManagedAgentPolicy,
    ) -> TestResult<RunResult> {
        let path = self.root.join("answers.json");
        fs::write(&path, answers.to_string())?;
        Ok(LocalOrchestrator::default().resume(&ResumeRequest {
            run_id: self.run_id.clone(),
            answers_path: path,
            receipt_dir: Some(self.receipts.clone()),
            env: env(&self.root),
            cwd: self.root.clone(),
            managed_agent,
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

/// The addresses a pending call waits on.
fn waiting_on(call: &TurnCallResult) -> TestResult<Vec<String>> {
    let TurnCallOutcome::Pending { waiting } = &call.outcome else {
        return Err(format!("the call is not pending: {call:?}").into());
    };
    Ok(waiting
        .iter()
        .map(|request| request_id(request).to_owned())
        .collect())
}

fn request_id(request: &ResolutionRequest) -> &str {
    match request {
        ResolutionRequest::Input { id, .. }
        | ResolutionRequest::Approval { id, .. }
        | ResolutionRequest::AgentAct { id, .. }
        | ResolutionRequest::Retry { id, .. } => id.as_str(),
    }
}

fn tool_names(status: &TurnStatus) -> Vec<&str> {
    status.tools.iter().map(|tool| tool.name.as_str()).collect()
}

#[test]
fn an_agent_turn_records_its_calls_and_seals_them_with_its_answer() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let turn = paused.open()?;
    assert_eq!(
        serde_json::to_value(&turn.status(TURN)?.invocation.allowance)?,
        serde_json::json!({"tools": ["data.digest"], "skills": ["../digest-child"], "asks": true})
    );

    let digest = turn.call(
        TURN,
        "data_digest",
        input(serde_json::json!({"value": "hello", "encoding": "utf8_text"}))?,
    )?;
    assert_eq!(digest.index, 0);
    assert!(
        matches!(&digest.outcome, TurnCallOutcome::Completed { output, .. } if output.contains_key("digest_result")),
        "{digest:?}"
    );

    let refused = turn.call(TURN, "ask_human", JsonObject::new())?;
    assert!(
        matches!(&refused.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains("reason")),
        "{refused:?}"
    );

    let child = turn.call(
        TURN,
        "skill__digest-child",
        input(serde_json::json!({"value": "child"}))?,
    )?;
    assert!(
        matches!(child.outcome, TurnCallOutcome::Completed { .. }),
        "{child:?}"
    );

    let asked = turn.call(
        TURN,
        "ask_human",
        input(serde_json::json!({"reason": "Send it?"}))?,
    )?;
    assert_eq!(waiting_on(&asked)?, ["answer/3"]);

    // The paused run lists what the human must answer beside the act.
    let waiting = paused.resume(serde_json::json!({}))?;
    assert_eq!(requests(&waiting)?, [TURN, "answer/3"]);

    // The human's answer settles the pending call; the turn stays open.
    let settled = paused.resume(serde_json::json!({"approvals": {"answer/3": true}}))?;
    assert_eq!(requests(&settled)?, [TURN]);
    assert_eq!(
        statuses(&turn.status(TURN)?),
        ["completed", "refused", "completed", "completed"]
    );

    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "digested"}))?)?;
    assert_eq!(
        paused.sealed_step_children(&sealed)?,
        3,
        "the turn's receipt names its completed calls"
    );
    Ok(())
}

#[test]
fn a_turn_inside_a_child_skill_is_worked_at_its_own_address() -> TestResult {
    let temp = tempdir()?;
    let child_turn = "delegate#agent.answer.output";
    let paused = Paused::start(temp.path(), write_parent_skill(temp.path())?, child_turn)?;
    let turn = paused.open()?;
    assert_eq!(
        tool_names(&turn.status(child_turn)?),
        ["data_digest", "skill__digest-child", "ask_human"],
        "the child's turn offers the child's limits"
    );

    let digest = turn.call(
        child_turn,
        "data_digest",
        input(serde_json::json!({"value": "nested", "encoding": "utf8_text"}))?,
    )?;
    assert!(
        matches!(digest.outcome, TurnCallOutcome::Completed { .. }),
        "{digest:?}"
    );
    assert_eq!(statuses(&turn.status(child_turn)?), ["completed"]);

    let sealed = turn.complete(child_turn, input(serde_json::json!({"answer": "nested"}))?)?;
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
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let turn = paused.open()?;
    turn.call(
        TURN,
        "ask_human",
        input(serde_json::json!({"reason": "Go ahead?"}))?,
    )?;

    let refused = turn
        .complete(TURN, input(serde_json::json!({"answer": "early"}))?)
        .err()
        .ok_or("an answer while a call waits is refused")?;
    assert!(
        refused
            .to_string()
            .contains("call 0 of its turn has not settled"),
        "{refused}"
    );

    paused.resume(serde_json::json!({"approvals": {"answer/0": false}}))?;
    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "after"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 1);
    Ok(())
}

#[test]
fn a_turn_lists_its_tools_with_their_contracts() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let status = paused.open()?.status(TURN)?;
    let envelope = &status.invocation.envelope;
    assert!(envelope.instructions.as_str().contains("Turn demo"));
    assert!(
        envelope
            .output_schema
            .as_ref()
            .and_then(|schema| schema.as_object())
            .and_then(|schema| schema.get("properties"))
            .and_then(JsonValue::as_object)
            .is_some_and(|properties| properties.contains_key("answer")),
        "{envelope:?}"
    );
    let schema = |name: &str| -> TestResult<serde_json::Value> {
        Ok(serde_json::to_value(
            &status
                .tools
                .iter()
                .find(|tool| tool.name.as_str() == name)
                .ok_or(format!("missing tool {name}"))?
                .input_schema,
        )?)
    };
    assert!(schema("data_digest")?["properties"]["value"].is_object());
    assert!(schema("skill__digest-child")?["properties"]["value"].is_object());
    assert!(schema("ask_human")?["properties"]["reason"].is_object());
    Ok(())
}

#[test]
fn a_turn_the_run_has_left_cannot_be_worked() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let turn = paused.open()?;
    paused.resume(serde_json::json!({
        "answers": {TURN: {"answer": "done"}}
    }))?;

    let error = turn
        .call(
            TURN,
            "data_digest",
            input(serde_json::json!({"value": "late", "encoding": "utf8_text"}))?,
        )
        .err()
        .ok_or("a sealed run's turn takes no calls")?;
    assert!(error.to_string().contains("has no open turn"), "{error}");
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
    let paused = Paused::start(temp.path(), skill_dir, TURN)?;
    let step = |status: TurnStatus| status.invocation.envelope.step_id.map(|id| id.to_string());
    let first = paused.open()?;
    assert_eq!(step(first.status(TURN)?), Some("first".to_owned()));
    let next = first.complete(TURN, input(serde_json::json!({"answer": "one"}))?)?;
    assert_eq!(requests(&next)?, [TURN], "the second step asks its own act");
    let second = paused.open()?;
    assert_eq!(step(second.status(TURN)?), Some("second".to_owned()));
    let sealed = second.complete(TURN, input(serde_json::json!({"answer": "two"}))?)?;
    assert_eq!(text(object(&sealed.output)?, "status"), Some("sealed"));
    Ok(())
}

#[test]
fn a_call_into_a_skill_whose_agent_shares_the_task_opens_a_turn_of_its_own() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_answer_skills(temp.path())?, TURN)?;
    let turn = paused.open()?;
    assert_eq!(
        tool_names(&turn.status(TURN)?),
        ["skill__answer-child"],
        "a turn may ask the human only when its skill declares asks"
    );
    let child = turn.call(TURN, "skill__answer-child", JsonObject::new())?;
    let child_turn = "answer/0#agent.answer.output";
    assert_eq!(waiting_on(&child)?, [child_turn]);
    assert_eq!(
        turn.waiting()?
            .iter()
            .map(|request| request_id(request).to_owned())
            .collect::<Vec<_>>(),
        [TURN, child_turn]
    );

    // The same session works the child's turn, by its address.
    assert_eq!(
        turn.status(child_turn)?.invocation.envelope.skill.as_str(),
        "answer"
    );
    turn.complete(
        child_turn,
        input(serde_json::json!({"answer": "from the child"}))?,
    )?;
    assert_eq!(statuses(&turn.status(TURN)?), ["completed"]);

    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "parent"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 1);
    Ok(())
}

#[test]
fn sibling_child_turns_are_worked_apart() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_answer_skills(temp.path())?, TURN)?;
    let turn = paused.open()?;
    turn.call(TURN, "skill__answer-child", JsonObject::new())?;
    turn.call(TURN, "skill__answer-child", JsonObject::new())?;
    let (first, second) = (
        "answer/0#agent.answer.output",
        "answer/1#agent.answer.output",
    );
    assert_eq!(
        turn.waiting()?
            .iter()
            .map(|request| request_id(request).to_owned())
            .collect::<Vec<_>>(),
        [TURN, first, second]
    );

    // The later sibling ends first; the earlier one is untouched.
    turn.complete(second, input(serde_json::json!({"answer": "two"}))?)?;
    assert_eq!(statuses(&turn.status(TURN)?), ["pending", "completed"]);
    turn.complete(first, input(serde_json::json!({"answer": "one"}))?)?;
    assert_eq!(statuses(&turn.status(TURN)?), ["completed", "completed"]);

    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "both"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 2);
    Ok(())
}

#[test]
fn a_nested_ask_is_answered_by_its_address() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_answer_skills(temp.path())?, TURN)?;
    let turn = paused.open()?;
    turn.call(TURN, "skill__answer-child", JsonObject::new())?;
    let child_turn = "answer/0#agent.answer.output";
    let asked = turn.call(
        child_turn,
        "ask_human",
        input(serde_json::json!({"reason": "Go on?"}))?,
    )?;
    let ask = "answer/0#answer/0";
    assert_eq!(waiting_on(&asked)?, [ask]);

    // The run lists the child's ask beside both turns, and a resume answers
    // it where it waits.
    let waiting = paused.resume(serde_json::json!({}))?;
    assert_eq!(requests(&waiting)?, [TURN, child_turn, ask]);
    let settled = paused.resume(serde_json::json!({"approvals": {ask: true}}))?;
    assert_eq!(requests(&settled)?, [TURN, child_turn]);
    assert_eq!(statuses(&turn.status(child_turn)?), ["completed"]);

    turn.complete(
        child_turn,
        input(serde_json::json!({"answer": "approved"}))?,
    )?;
    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "done"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 1);
    Ok(())
}

// Only a build with the managed agent can be refused for using it.
#[cfg(feature = "agent")]
#[test]
fn a_run_keeps_the_driver_it_started_with() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_turn_skill(temp.path())?;
    // No model is configured, so the managed agent leaves the turn open.
    let managed = Paused::start_with(
        temp.path(),
        skill_dir.clone(),
        TURN,
        ManagedAgentPolicy::inline(2)?,
    )?;
    let error = managed
        .open()
        .err()
        .ok_or("a host cannot work a managed run's turns")?;
    assert!(
        error.to_string().contains("driven by the managed agent"),
        "{error}"
    );
    assert_eq!(requests(&managed.resume(serde_json::json!({}))?)?, [TURN]);

    let attached = Paused::start(temp.path(), skill_dir, TURN)?;
    let error = attached
        .resume_with(serde_json::json!({}), ManagedAgentPolicy::inline(2)?)
        .err()
        .ok_or("the managed agent cannot work an attached run's turns")?;
    assert!(
        error
            .to_string()
            .contains("driven by the host it is attached to"),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_resume_pinned_to_another_package_is_refused() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
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

/// A one-turn skill whose harness case works the turn: it digests a value,
/// expects the call to end `status`, then answers.
fn write_scripted_turn_skill(root: &Path, status: &str) -> TestResult<PathBuf> {
    let skill_dir = root.join("scripted-turn");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: scripted-turn\ndescription: Answers with what it digested.\n---\n# Scripted turn\n\nDigest the question, then answer.\n",
        &format!(
            r#"skill: scripted-turn
tools: [data.digest]
harness:
  cases:
    - name: digests-then-answers
      inputs:
        question: what digests?
      caller:
        turns:
          {TURN}:
            calls:
              - tool: data_digest
                input:
                  value: what digests?
                  encoding: utf8_text
                expect:
                  subset:
                    index: 0
                    outcome:
                      status: {status}
            output:
              answer: digested
      expect:
        status: sealed
        output:
          subset:
            answer: digested
runners:
  answer:
    default: true
    inputs:
      question: {{ type: string, required: true }}
    outputs:
      answer: string
"#
        ),
    )?;
    Ok(skill_dir)
}

fn run_case(root: &Path, skill_dir: PathBuf) -> TestResult<runx_runtime::PackageHarnessReport> {
    Ok(LocalOrchestrator::default()
        .with_environment(env(root))
        .run_package_harness(&PackageHarnessRequest {
            skill_path: skill_dir,
            receipt_dir: Some(root.join("receipts")),
        })?)
}

/// A parent and child whose turns share an id: a script keyed by the
/// child's address is the child's own, and the parent's works only the parent.
#[test]
fn a_script_by_address_works_the_turn_it_names() -> TestResult {
    let temp = tempdir()?;
    write_answer_skills(temp.path())?;
    let skill_dir = temp.path().join("scripted-parent");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: scripted-parent\ndescription: Asks its child.\n---\n# Scripted parent\n\nAsk the child, then answer.\n",
        &format!(
            r#"skill: scripted-parent
skills: [../answer-child]
harness:
  cases:
    - name: the-child-has-its-own-script
      inputs:
        question: what digests?
      caller:
        turns:
          {TURN}:
            calls:
              - tool: skill__answer-child
                expect:
                  subset:
                    outcome:
                      status: completed
                      output:
                        answer: from the child
            output:
              answer: from the parent
          "answer/0#{TURN}":
            output:
              answer: from the child
      expect:
        status: sealed
        output:
          subset:
            answer: from the parent
runners:
  answer:
    default: true
    inputs:
      question: {{ type: string, required: true }}
    outputs:
      answer: string
"#
        ),
    )?;
    let report = run_case(temp.path(), skill_dir)?;
    assert_eq!(report.status, "passed", "{:?}", report.assertion_errors);
    Ok(())
}

#[test]
fn a_harness_case_works_a_turn_call_by_call() -> TestResult {
    let temp = tempdir()?;
    let report = run_case(
        temp.path(),
        write_scripted_turn_skill(temp.path(), "completed")?,
    )?;
    assert_eq!(report.status, "passed", "{:?}", report.assertion_errors);
    assert_eq!(report.case_count, 1);
    Ok(())
}

#[test]
fn a_scripted_call_that_returns_something_else_fails_its_case() -> TestResult {
    let temp = tempdir()?;
    let report = run_case(
        temp.path(),
        write_scripted_turn_skill(temp.path(), "refused")?,
    )?;
    assert_eq!(report.status, "failed");
    assert!(
        report
            .assertion_errors
            .iter()
            .any(|error| error.contains("caller.turns.agent.answer.output.calls[0].expect")),
        "{:?}",
        report.assertion_errors
    );
    Ok(())
}

/// An actor's authority whose one delegation bounds how far it spreads.
fn bounded(fanout: Option<u32>, depth: Option<u32>) -> runx_runtime::RunAuthority {
    use runx_contracts::{
        AuthorityBounds, AuthorityResourceFamily, AuthorityVerb, Reference, ReferenceType,
    };
    let actor = Reference::with_uri(ReferenceType::Principal, "runx:principal:assistant");
    runx_runtime::RunAuthority {
        actor_ref: actor.clone(),
        mandate_ref: Some(Reference::runx(ReferenceType::Mandate, "test-mandate")),
        delegations: vec![runx_core::policy::AttenuationRequest {
            principal_ref: actor,
            resource_ref: Reference::with_uri(
                ReferenceType::ConnectorInstance,
                "runx:connector_instance:any",
            ),
            resource_family: AuthorityResourceFamily::Connector,
            verbs: vec![AuthorityVerb::Read],
            capabilities: Vec::new(),
            bounds: AuthorityBounds {
                max_fanout: fanout,
                max_child_depth: depth,
                ..AuthorityBounds::default()
            },
            expires_at: None,
        }],
    }
}

/// Start `skill_dir` under `authority` and return what the run first says.
fn start_bounded(
    root: &Path,
    skill_dir: PathBuf,
    authority: runx_runtime::RunAuthority,
) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().run_skill(&SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: Some(authority),
        skill_path: skill_dir,
        receipt_dir: Some(root.join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::from([(
            "question".to_owned(),
            JsonValue::String("what digests?".to_owned()),
        )]),
        env: env(root),
        cwd: root.to_path_buf(),
        managed_agent: ManagedAgentPolicy::HostDriven,
    })?)
}

fn opened(root: &Path, started: &RunResult) -> TestResult<TurnSession> {
    Ok(LocalOrchestrator::default().open_turns(&TurnRequest {
        run_id: text(object(&started.output)?, "run_id")
            .ok_or("missing run_id")?
            .to_owned(),
        receipt_dir: Some(root.join("receipts")),
        env: env(root),
        cwd: root.to_path_buf(),
    })?)
}

#[test]
fn a_delegation_bounds_the_child_runs_a_turn_starts() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_turn_skill(temp.path())?;
    let started = start_bounded(temp.path(), skill_dir, bounded(Some(1), None))?;
    let turn = opened(temp.path(), &started)?;
    let call = |value: &str| -> TestResult<TurnCallResult> {
        Ok(turn.call(
            TURN,
            "skill__digest-child",
            input(serde_json::json!({ "value": value }))?,
        )?)
    };
    assert!(matches!(
        call("one")?.outcome,
        TurnCallOutcome::Completed { .. }
    ));
    let second = call("two")?;
    assert!(
        matches!(&second.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains("at most 1 child runs")),
        "{second:?}"
    );
    Ok(())
}

#[test]
fn a_skill_call_that_starts_no_child_spends_no_fanout() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_turn_skill(temp.path())?;
    let started = start_bounded(temp.path(), skill_dir, bounded(Some(1), None))?;
    let turn = opened(temp.path(), &started)?;
    let missing = turn.call(TURN, "skill__digest-child", JsonObject::new())?;
    assert!(
        !matches!(missing.outcome, TurnCallOutcome::Completed { .. }),
        "{missing:?}"
    );
    let first = turn.call(
        TURN,
        "skill__digest-child",
        input(serde_json::json!({ "value": "one" }))?,
    )?;
    assert!(
        matches!(first.outcome, TurnCallOutcome::Completed { .. }),
        "{first:?}"
    );
    Ok(())
}

#[test]
fn a_delegation_bounds_how_deep_child_runs_nest() -> TestResult {
    let temp = tempdir()?;
    let turn_skill = write_turn_skill(temp.path())?;
    let started = start_bounded(temp.path(), turn_skill, bounded(None, Some(0)))?;
    let refused = opened(temp.path(), &started)?.call(
        TURN,
        "skill__digest-child",
        input(serde_json::json!({ "value": "deep" }))?,
    )?;
    assert!(
        matches!(&refused.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains("nest at most 0 deep")),
        "{refused:?}"
    );

    // A graph's skill step starts a child run too.
    let other = tempdir()?;
    let parent = write_parent_skill(other.path())?;
    let sealed = start_bounded(other.path(), parent, bounded(None, Some(0)))?;
    let closure = object(
        object(&sealed.output)?
            .get("closure")
            .ok_or("missing closure")?,
    )?;
    assert_eq!(text(closure, "disposition"), Some("blocked"));
    assert_eq!(text(closure, "reason_code"), Some("authority_denied"));
    assert!(
        text(closure, "summary").is_some_and(|summary| summary.contains("nest at most 0 deep")),
        "{closure:?}"
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
