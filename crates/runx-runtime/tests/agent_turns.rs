use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{
    Effects, JsonObject, JsonValue, Reach, ResolutionRequest, RunCallOutcome, RunEventFact,
    TurnCallOutcome, TurnCallResult, TurnStatus,
};
use runx_runtime::{
    LocalOrchestrator, ManagedAgentPolicy, PackageTestRequest, ResumeRequest, RunResult,
    SkillRunRequest, TurnRequest, TurnSession,
};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The act of a one-turn method named `answer`, at the root.
const TURN: &str = "answer";

/// A skill whose one method is one agent turn: it may digest data, run a
/// child skill that does the same, and ask the human.
fn write_turn_skill(root: &Path) -> TestResult<PathBuf> {
    crate::support::write_test_skill_package(
        &root.join("digest-child"),
        "---\nname: digest-child\ndescription: Digests a value.\n---\n# Digest child\n\nDigest the value.\n",
        r#"minimum_runx: 0.12.0
methods:
  digest:
    inputs:
      value: {type: string}
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
        "---\nname: turn-demo\ndescription: Answers with what it digested.\n---\n# Turn demo\n\n## Agent task: answer\n\nDigest what you need, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          tools: [data.digest]
          skills: [../digest-child]
          ask: true
        inputs: $input
        outputs:
          answer: {type: string}
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
        r#"minimum_runx: 0.12.0
methods:
  ask:
    inputs:
      question: {type: string}
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
        "---\nname: answer-child\ndescription: Answers.\n---\n# Answer child\n\n## Agent task: answer\n\nAnswer.\n",
        "minimum_runx: 0.12.0\nmethods:\n  answer:\n    steps:\n      - id: answer\n        agent: answer\n        calls:\n          ask: true\n        inputs: $input\n        outputs:\n          answer: {type: string}\n",
    )?;
    let skill_dir = root.join("answer-parent");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: answer-parent\ndescription: Answers through a child.\n---\n# Answer parent\n\n## Agent task: answer\n\nAsk the child, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          skills: [../answer-child]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    Ok(skill_dir)
}

/// A parent whose act calls a child skill whose own act digests values and
/// runs commands: both reach the world, so each is claimed and executed
/// apart from the session's driver.
fn write_nested_tool_skills(root: &Path) -> TestResult<PathBuf> {
    crate::support::write_test_skill_package(
        &root.join("tool-child"),
        "---\nname: tool-child\ndescription: Works with tools, then answers.\n---\n# Tool child\n\n## Agent task: answer\n\nWork, then answer.\n",
        "minimum_runx: 0.12.0\nmethods:\n  answer:\n    steps:\n      - id: answer\n        agent: answer\n        calls:\n          tools: [data.digest, command.execute]\n        inputs: $input\n        outputs:\n          answer: {type: string}\n",
    )?;
    let skill_dir = root.join("tool-parent");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: tool-parent\ndescription: Answers through a child.\n---\n# Tool parent\n\n## Agent task: answer\n\nAsk the child, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          skills: [../tool-child]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    Ok(skill_dir)
}

/// A one-turn skill whose act reads the house style, a context skill the
/// package holds.
fn write_context_turn_skill(root: &Path) -> TestResult<PathBuf> {
    let skill_dir = root.join("styled");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: styled\ndescription: Answers in the house style.\n---\n# Styled\n\n## Agent task: answer\n\nAnswer in the house style.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        inputs:
          question: $input.question
        outputs:
          answer: {type: string}
        context:
          skills: [./context/house-style]
"#,
    )?;
    let style = skill_dir.join("context/house-style");
    std::fs::create_dir_all(&style)?;
    std::fs::write(
        style.join("SKILL.md"),
        "---\nname: house-style\ndescription: How the house writes.\n---\n# House style\n\n## Agent task: operate\n\nWrite in short sentences.\n",
    )?;
    std::fs::write(
        style.join("run.yaml"),
        "minimum_runx: 0.12.0\ncatalog:\n  visibility: internal\nmethods:\n  operate:\n    steps:\n      - id: operate\n        agent: operate\n        outputs:\n          result: {type: object}\n",
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
        let started = LocalOrchestrator::default().run_skill_with_binding(
            &SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: None,
                act: None,
                skill_path: skill_dir,
                receipt_dir: Some(receipts.clone()),
                run_id: None,
                answers: None,
                inputs: BTreeMap::from([(
                    "question".to_owned(),
                    JsonValue::String("what digests?".to_owned()),
                )]),
                env: env(root),
                cwd: root.to_path_buf(),
                managed_agent,
            },
            None,
            None,
            None,
        )?;
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
        Ok(LocalOrchestrator::default().resume(&ResumeRequest {
            run_id: self.run_id.clone(),
            answers: crate::support::bind_resume(
                &env(&self.root),
                &self.root,
                &self.run_id,
                answers,
            )?,
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
        let workflow = crate::support::read_test_signed_receipt(&self.receipts, receipt_id)?;
        let step = crate::support::read_test_first_step_receipt(&self.receipts, &workflow)?;
        Ok(step.lineage.children.len())
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
            TurnCallOutcome::Declined { .. } => "declined",
            TurnCallOutcome::Pending { .. } => "pending",
            TurnCallOutcome::Cancelled => "cancelled",
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
        | ResolutionRequest::Turn { id, .. }
        | ResolutionRequest::Retry { id, .. }
        | ResolutionRequest::Purchase { id, .. }
        | ResolutionRequest::Wait { id, .. } => id.as_str(),
    }
}

fn tool_names(status: &TurnStatus) -> Vec<&str> {
    status.tools.iter().map(|tool| tool.name.as_str()).collect()
}

/// A turn shows what it may call once, by the names its calls take. The act's
/// declared allowance spells the catalog tool `data.digest`, which a call
/// names `data_digest`, so a test that called what the allowance showed
/// was refused its first call. The status shows the allowance only as its
/// tools, and each name it shows is one a call takes.
#[test]
fn a_turn_shows_what_it_may_call_by_the_names_its_calls_take() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let turn = paused.open()?;
    let status = turn.status(TURN)?;
    assert_eq!(
        serde_json::to_value(&status)?["invocation"].get("allowance"),
        None,
        "the declared allowance is not shown beside the tools"
    );
    assert_eq!(
        tool_names(&status),
        ["data_digest", "skill__digest-child", "ask_human"]
    );
    let digest = turn.call(
        TURN,
        "data_digest",
        None,
        input(serde_json::json!({"value": "hello", "encoding": "utf8_text"}))?,
    )?;
    assert!(
        matches!(digest.outcome, TurnCallOutcome::Completed { .. }),
        "{digest:?}"
    );
    Ok(())
}

/// A bundled skill a load binds is the runtime's own copy, named
/// `runx/<name>@<version>`: materialized into the runx home, which no path
/// inside the workspace names, and never into the project. Its method then
/// runs as the load bound it.
#[test]
fn a_bundled_skill_loads_by_its_name_wherever_it_is_materialized() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().join("workspace");
    let skill_dir = root.join("session");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: session\ndescription: Works with what it loads.\n---\n# Session\n\n## Agent task: answer\n\nLoad what you need, then answer.\n",
        "minimum_runx: 0.12.0\nmethods:\n  answer:\n    steps:\n      - id: answer\n        agent: answer\n        calls:\n          load: [{catalog: {trust: first_party}}]\n        inputs: $input\n        outputs:\n          answer: {type: string}\n",
    )?;
    let mut env = env(&root);
    env.insert(
        "RUNX_PROJECT_DIR".to_owned(),
        temp.path().join("project").to_string_lossy().into_owned(),
    );
    let receipts = temp.path().join("receipts");
    let started = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(receipts.clone()),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env: env.clone(),
            cwd: root.clone(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        None,
        None,
        None,
    )?;
    assert_eq!(requests(&started)?, [TURN]);
    let turn = LocalOrchestrator::default().open_turns(&TurnRequest {
        run_id: text(object(&started.output)?, "run_id")
            .ok_or("missing run_id")?
            .to_owned(),
        receipt_dir: Some(receipts),
        env,
        cwd: root.clone(),
    })?;

    let load = turn.call(
        TURN,
        "load",
        None,
        input(serde_json::json!({"skill": "runx/receipt-query"}))?,
    )?;
    let TurnCallOutcome::Completed { output, .. } = &load.outcome else {
        return Err(format!("the bundled skill loads: {load:?}").into());
    };
    let loaded: runx_contracts::TurnLoad = JsonValue::Object(output.clone()).deserialize_into()?;
    let runx_contracts::LoadedPackage::Skill { skill: package, .. } = &loaded.package else {
        return Err(format!("a skill was loaded: {loaded:?}").into());
    };
    let [tool] = loaded.tools.as_slice() else {
        return Err(format!("its one method is bound: {loaded:?}").into());
    };
    let runx_contracts::CallTarget::Skill { skill, .. } = &tool.target else {
        return Err(format!("a bound method: {tool:?}").into());
    };
    assert!(skill.as_str().starts_with("runx/receipt-query@"), "{skill}");
    assert_eq!(skill, package);
    assert!(
        matches!(
            &loaded.package,
            runx_contracts::LoadedPackage::Skill { trust_tier: Some(runx_contracts::TrustTier::FirstParty), publisher: Some(publisher), .. }
                if publisher.as_str() == "runx"
        ),
        "{loaded:?}"
    );
    // The home here fell back to the workspace, and bundled skills still
    // materialize there, never into the project.
    assert!(root.join(".runx/bundled/receipt-query").is_dir());
    assert!(!temp.path().join("project/added-skills").exists());

    let read = turn.call(TURN, tool.name.as_str(), None, JsonObject::new())?;
    assert!(
        matches!(read.outcome, TurnCallOutcome::Completed { .. }),
        "{read:?}"
    );
    Ok(())
}

#[test]
fn an_agent_turn_records_its_calls_and_seals_them_with_its_answer() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let turn = paused.open()?;

    let digest = turn.call(
        TURN,
        "data_digest",
        None,
        input(serde_json::json!({"value": "hello", "encoding": "utf8_text"}))?,
    )?;
    assert_eq!(digest.index, 0);
    assert!(
        matches!(&digest.outcome, TurnCallOutcome::Completed { output, .. } if output.contains_key("digest_result")),
        "{digest:?}"
    );

    let refused = turn.call(TURN, "ask_human", None, JsonObject::new())?;
    assert!(
        matches!(&refused.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains("reason")),
        "{refused:?}"
    );

    let child = turn.call(
        TURN,
        "skill__digest-child",
        None,
        input(serde_json::json!({"value": "child"}))?,
    )?;
    assert!(
        matches!(child.outcome, TurnCallOutcome::Completed { .. }),
        "{child:?}"
    );

    let asked = turn.call(
        TURN,
        "ask_human",
        None,
        input(serde_json::json!({"reason": "Send it?"}))?,
    )?;
    assert_eq!(waiting_on(&asked)?, ["answer/3"]);
    // A person's answer needs the run's driver lock: the host closes its
    // session to deliver it, and opens it again after.
    drop(turn);

    // The paused run lists what the human must answer beside the act.
    let waiting = paused.resume(serde_json::json!({}))?;
    assert_eq!(requests(&waiting)?, [TURN, "answer/3"]);

    // The human's answer settles the pending call; the turn stays open.
    let settled = paused.resume(serde_json::json!({"approvals": {"answer/3": true}}))?;
    assert_eq!(requests(&settled)?, [TURN]);
    let turn = paused.open()?;
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
fn a_turns_context_is_an_index_each_entry_of_which_reads_by_its_ref() -> TestResult {
    let temp = tempdir()?;
    let skill_dir = write_context_turn_skill(temp.path())?;
    let paused = Paused::start(temp.path(), skill_dir, TURN)?;
    let turn = paused.open()?;
    let status = turn.status(TURN)?;
    let [line] = status.invocation.envelope.current_context.as_slice() else {
        return Err(format!("one context entry: {status:?}").into());
    };
    assert_eq!(line.locator.as_str(), "current:0");
    assert_eq!(
        line.summary.as_ref().map(|summary| summary.as_str()),
        Some("Context skill house-style: How the house writes.")
    );
    assert!(
        !serde_json::to_string(&status)?.contains("Write in short sentences."),
        "a turn's status never carries an entry's data"
    );

    let page = turn.read(TURN, "current:0", 0, None)?;
    assert!(page.text.contains("Write in short sentences."), "{page:?}");
    assert_eq!(page.next_offset, None);
    assert_eq!(
        turn.read(TURN, "input:question", 0, None)?.text,
        "what digests?"
    );
    let unheld = turn
        .read(TURN, "current:1", 0, None)
        .err()
        .ok_or("a ref the turn does not hold is refused")?;
    assert!(
        unheld.to_string().contains("holds no current:1"),
        "{unheld}"
    );
    Ok(())
}

#[test]
fn a_turn_inside_a_child_skill_is_worked_at_its_own_address() -> TestResult {
    let temp = tempdir()?;
    let child_turn = "delegate#answer";
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
        None,
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
        None,
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

    drop(turn);
    paused.resume(serde_json::json!({"approvals": {"answer/0": false}}))?;
    let turn = paused.open()?;
    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "after"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 1);
    Ok(())
}

/// A run that waits seals at the time it closes, not the time it started.
#[test]
fn a_run_answered_later_seals_at_the_time_it_closes() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    // The answer comes at a later millisecond than the start, so a seal
    // stamped at the start would sort before it.
    let started = runx_runtime::RunClock::System.now();
    while runx_runtime::RunClock::System.now() <= started {
        std::hint::spin_loop();
    }
    let answered_at = runx_runtime::RunClock::System.now();
    let sealed = paused
        .open()?
        .complete(TURN, input(serde_json::json!({"answer": "later"}))?)?;
    let output = object(&sealed.output)?;
    let receipt_id = text(output, "receipt_id").ok_or("missing receipt_id")?;
    let workflow = crate::support::read_test_signed_receipt(&paused.receipts, receipt_id)?;
    let step = crate::support::read_test_first_step_receipt(&paused.receipts, &workflow)?;
    for receipt in [&workflow, &step] {
        assert!(
            receipt.created_at.as_str() >= answered_at.as_str(),
            "sealed {} before the answer at {answered_at}",
            receipt.created_at.as_str()
        );
    }
    Ok(())
}

/// A method with no steps is one agent turn, handed the run's inputs.
#[test]
fn a_one_turn_method_hands_its_agent_the_run_inputs() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let session = paused.open()?;
    let status = session.status(TURN)?;
    assert_eq!(
        status.invocation.envelope.inputs.get("question"),
        Some(&JsonValue::String("what digests?".to_owned())),
        "{:?}",
        status.invocation.envelope.inputs
    );
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
    turn.complete(TURN, input(serde_json::json!({"answer": "done"}))?)?;

    let error = turn
        .call(
            TURN,
            "data_digest",
            None,
            input(serde_json::json!({"value": "late", "encoding": "utf8_text"}))?,
        )
        .err()
        .ok_or("a sealed run's turn takes no calls")?;
    assert!(error.to_string().contains("has no open turn"), "{error}");
    drop(turn);
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
        "---\nname: twice\ndescription: Answers twice.\n---\n# Twice\n\n## Agent task: answer\n\nAnswer each step.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: first
        agent: answer
        outputs:
          answer: {type: string}
      - id: second
        agent: answer
        outputs:
          answer: {type: string}
"#,
    )?;
    let paused = Paused::start(temp.path(), skill_dir, "first")?;
    let step = |status: TurnStatus| status.invocation.envelope.step_id.map(|id| id.to_string());
    let first = paused.open()?;
    assert_eq!(step(first.status("first")?), Some("first".to_owned()));
    let next = first.complete("first", input(serde_json::json!({"answer": "one"}))?)?;
    assert_eq!(
        requests(&next)?,
        ["second"],
        "the second step asks its own act"
    );
    drop(first);
    let second = paused.open()?;
    assert_eq!(step(second.status("second")?), Some("second".to_owned()));
    let sealed = second.complete("second", input(serde_json::json!({"answer": "two"}))?)?;
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
    let child = turn.call(TURN, "skill__answer-child", None, JsonObject::new())?;
    let child_turn = "answer/0#answer";
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
    turn.call(TURN, "skill__answer-child", None, JsonObject::new())?;
    turn.call(TURN, "skill__answer-child", None, JsonObject::new())?;
    let (first, second) = ("answer/0#answer", "answer/1#answer");
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
    turn.call(TURN, "skill__answer-child", None, JsonObject::new())?;
    let child_turn = "answer/0#answer";
    let asked = turn.call(
        child_turn,
        "ask_human",
        None,
        input(serde_json::json!({"reason": "Go on?"}))?,
    )?;
    let ask = "answer/0#answer/0";
    assert_eq!(waiting_on(&asked)?, [ask]);
    drop(turn);

    // The run lists the child's ask beside both turns, and a resume answers
    // it where it waits.
    let waiting = paused.resume(serde_json::json!({}))?;
    assert_eq!(requests(&waiting)?, [TURN, child_turn, ask]);
    let settled = paused.resume(serde_json::json!({"approvals": {ask: true}}))?;
    assert_eq!(requests(&settled)?, [TURN, child_turn]);
    let turn = paused.open()?;
    assert_eq!(statuses(&turn.status(child_turn)?), ["completed"]);

    turn.complete(
        child_turn,
        input(serde_json::json!({"answer": "approved"}))?,
    )?;
    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "done"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 1);
    Ok(())
}

#[test]
fn a_nested_turns_call_runs_where_it_waits() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_nested_tool_skills(temp.path())?, TURN)?;
    let turn = paused.open()?;
    let entered = turn.call(TURN, "skill__tool-child", None, JsonObject::new())?;
    let child_turn = "answer/0#answer";
    assert_eq!(waiting_on(&entered)?, [child_turn]);

    // The child's call reaches the world: admitted under the driver, then
    // executed apart from it at the child's turn, and recorded there.
    let digested = turn.call(
        child_turn,
        "data_digest",
        Some("nested"),
        input(serde_json::json!({"value": "nested", "encoding": "utf8_text"}))?,
    )?;
    assert!(
        matches!(digested.outcome, TurnCallOutcome::Completed { .. }),
        "{digested:?}"
    );
    assert_eq!(statuses(&turn.status(child_turn)?), ["completed"]);

    // The child's answer settles the parent's call.
    turn.complete(
        child_turn,
        input(serde_json::json!({"answer": "digested"}))?,
    )?;
    assert_eq!(statuses(&turn.status(TURN)?), ["completed"]);
    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "done"}))?)?;
    assert_eq!(paused.sealed_step_children(&sealed)?, 1);
    Ok(())
}

#[test]
#[cfg(unix)]
fn a_cancelled_nested_call_is_recorded_and_its_turn_goes_on() -> TestResult {
    use std::io::BufRead as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_nested_tool_skills(temp.path())?, TURN)?;
    let turn = paused.open()?;
    turn.call(TURN, "skill__tool-child", None, JsonObject::new())?;
    let child_turn = "answer/0#answer";

    // The host cancels a command while it runs: the command is terminated,
    // and the call records that it stopped, then returns. The command says
    // it's running on a named pipe, then waits on a gate nothing opens, so
    // only the cancellation ends it. The test holds both ends of the first
    // pipe, so neither side's open waits, and lets its own end go once the
    // call returns, so the watch ends on every path.
    let pipes = tempdir()?;
    let (running, gate) = (pipes.path().join("running"), pipes.path().join("gate"));
    for pipe in [&running, &gate] {
        let made = std::process::Command::new("mkfifo").arg(pipe).status()?;
        assert!(made.success(), "mkfifo {}", pipe.display());
    }
    let nonblocking = rustix::fs::OFlags::NONBLOCK;
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nonblocking.bits() as i32)
        .open(&running)?;
    rustix::fs::fcntl_setfl(&reader, rustix::fs::fcntl_getfl(&reader)? - nonblocking)?;
    let ours = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(nonblocking.bits() as i32)
        .open(&running)?;
    let command = input(serde_json::json!({"command": "sh", "args": [
        "-c",
        format!("echo running > '{}'; read go < '{}'", running.display(), gate.display()),
    ]}))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let stopped = std::thread::scope(|scope| {
        let watch = scope.spawn(|| {
            let mut line = String::new();
            let _read = std::io::BufReader::new(&reader).read_line(&mut line);
            if !line.is_empty() {
                cancel.store(true, Ordering::Release);
            }
        });
        let call = scope.spawn(|| {
            runx_runtime::interrupt::while_working(&cancel, || {
                turn.call(child_turn, "command_execute", Some("sleep"), command)
            })
        });
        let stopped = call.join();
        drop(ours);
        let _watched = watch.join();
        stopped
    })
    .map_err(|_| "the cancelled call panicked")??;
    let TurnCallOutcome::Completed { output, .. } = &stopped.outcome else {
        return Err(format!("the stopped call is recorded: {stopped:?}").into());
    };
    let decision = output
        .get("command_execution")
        .and_then(|execution| execution.as_object())
        .and_then(|execution| execution.get("data"))
        .and_then(|data| data.as_object())
        .and_then(|data| data.get("decision"))
        .and_then(JsonValue::as_str);
    assert_eq!(decision, Some("stopped"), "{output:?}");

    // The stop was the call's: the turn's next call runs.
    let next = turn.call(
        child_turn,
        "data_digest",
        Some("after"),
        input(serde_json::json!({"value": "after", "encoding": "utf8_text"}))?,
    )?;
    assert!(
        matches!(next.outcome, TurnCallOutcome::Completed { .. }),
        "{next:?}"
    );
    assert_eq!(
        statuses(&turn.status(child_turn)?),
        ["completed", "completed"]
    );
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
    let error = LocalOrchestrator::default()
        .resume(&ResumeRequest {
            run_id: paused.run_id.clone(),
            answers: runx_contracts::ResumeDocument::default(),
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

/// A one-turn skill whose test case works the turn: it digests a value,
/// expects the call to end `status`, then answers.
fn write_scripted_turn_skill(root: &Path, status: &str) -> TestResult<PathBuf> {
    let skill_dir = root.join("scripted-turn");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: scripted-turn\ndescription: Answers with what it digested.\n---\n# Scripted turn\n\n## Agent task: answer\n\nDigest the question, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          tools: [data.digest]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    std::fs::create_dir_all(skill_dir.join("tests"))?;
    std::fs::write(
        skill_dir.join("tests/digests-then-answers.yaml"),
        format!(
            r#"name: digests-then-answers
inputs:
  question: what digests?
host:
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
"#
        ),
    )?;
    Ok(skill_dir)
}

fn run_case(root: &Path, skill_dir: PathBuf) -> TestResult<runx_runtime::PackageTestReport> {
    Ok(LocalOrchestrator::default()
        .with_environment(env(root))
        .run_package_test(&PackageTestRequest {
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
        "---\nname: scripted-parent\ndescription: Asks its child.\n---\n# Scripted parent\n\n## Agent task: answer\n\nAsk the child, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          skills: [../answer-child]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    std::fs::create_dir_all(skill_dir.join("tests"))?;
    std::fs::write(
        skill_dir.join("tests/the-child-has-its-own-script.yaml"),
        format!(
            r#"name: the-child-has-its-own-script
inputs:
  question: what digests?
host:
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
"#
        ),
    )?;
    let report = run_case(temp.path(), skill_dir)?;
    assert_eq!(report.status, "passed", "{:?}", report.assertion_errors);
    Ok(())
}

#[test]
fn a_test_case_works_a_turn_call_by_call() -> TestResult {
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
            .any(|error| error.contains("host.turns.answer.calls[0].expect")),
        "{:?}",
        report.assertion_errors
    );
    Ok(())
}

/// An actor's authority that bounds how far its child runs spread.
fn bounded(fanout: Option<u32>, depth: Option<u32>) -> runx_runtime::RunAuthority {
    use runx_contracts::{Reference, ReferenceType};
    runx_runtime::RunAuthority {
        grants: std::collections::BTreeMap::new(),
        actor_ref: Some(Reference::with_uri(
            ReferenceType::Principal,
            "runx:principal:assistant",
        )),
        mandate_ref: Some(Reference::runx(ReferenceType::Mandate, "test-mandate")),
        delegations: Vec::new(),
        max_fanout: fanout,
        max_child_depth: depth,
        standing_approvals: Vec::new(),
        max_spend_micro_usd: None,
        ..runx_runtime::RunAuthority::default()
    }
}

/// Start `skill_dir` under `authority` and return what the run first says.
fn start_bounded(
    root: &Path,
    skill_dir: PathBuf,
    authority: runx_runtime::RunAuthority,
) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: Some(authority),
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::from([(
                "question".to_owned(),
                JsonValue::String("what digests?".to_owned()),
            )]),
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        None,
        None,
        None,
    )?)
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
            None,
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
    let missing = turn.call(TURN, "skill__digest-child", None, JsonObject::new())?;
    assert!(
        !matches!(missing.outcome, TurnCallOutcome::Completed { .. }),
        "{missing:?}"
    );
    let first = turn.call(
        TURN,
        "skill__digest-child",
        None,
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
        None,
        input(serde_json::json!({ "value": "deep" }))?,
    )?;
    assert!(
        matches!(&refused.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains("nest at most 0 deep")),
        "{refused:?}"
    );

    // A workflow's skill step starts a child run too.
    let other = tempdir()?;
    let parent = write_parent_skill(other.path())?;
    let sealed = start_bounded(other.path(), parent, bounded(None, Some(0)))?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "outcome"), Some("blocked"));
    assert_eq!(text(output, "seal_reason"), Some("authority_denied"));
    // Why authority refused rides the run's output, outside the signed receipt.
    let refusal = object(output.get("refusal").ok_or("missing refusal")?)?;
    assert!(
        text(refusal, "message").is_some_and(|message| message.contains("nest at most 0 deep")),
        "{refusal:?}"
    );
    Ok(())
}

/// A one-turn skill whose agent may read, list and write files.
fn write_file_turn_skill(root: &Path) -> TestResult<PathBuf> {
    let skill_dir = root.join("files-demo");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: files-demo\ndescription: Reads the project, then answers.\n---\n# Files demo\n\n## Agent task: answer\n\nRead what you need, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          tools: [fs.read, fs.list, fs.write]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    Ok(skill_dir)
}

fn completed(call: &TurnCallResult) -> bool {
    matches!(call.outcome, TurnCallOutcome::Completed { .. })
}

fn write_paged_read_skill(root: &Path) -> TestResult<PathBuf> {
    let dir = root.join("paged-child");
    let manifest = r#"minimum_runx: 0.12.0
methods:
  read:
    inputs:
      path: {type: string}
    steps:
    - id: page
      inputs: {source: $input.path}
      tool:
        module: page.mjs
        export: observe
        paged_input: {path: source, media_type: application/json, framing: json_array, page_bytes: 65536}
      outputs: {record_count: {type: number}}
"#;
    crate::support::write_test_skill_package(
        &dir,
        "---\nname: paged-child\ndescription: Counts runtime-framed synthetic records.\n---\n# Page proof\n\nCount only the records the native reader admits.\n",
        manifest,
    )?;
    std::fs::write(
        dir.join("page.mjs"),
        "export function observe({runx_page: page}) { const count=(page.state?.count??0)+page.records.length; return {runx_page:{state:{count},done:page.eof},...(page.eof?{record_count:count}:{})}; }\n",
    )?;
    Ok(dir)
}

fn run_paged_read(root: &Path, skill: PathBuf, path: &str, reach: Reach) -> TestResult<RunResult> {
    run_page_fixture(root, skill, input(serde_json::json!({"path":path}))?, reach)
}

fn run_page_fixture(
    root: &Path,
    skill: PathBuf,
    inputs: JsonObject,
    reach: Reach,
) -> TestResult<RunResult> {
    let mut authority = bounded(None, None);
    authority.reach = reach;
    Ok(LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: Some(authority),
            act: None,
            skill_path: skill,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs,
            env: env(root),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        None,
        None,
        None,
    )?)
}

#[test]
fn admitted_artifact_pages_run_through_real_store_open_and_child_workflows() -> TestResult {
    let temp = tempdir()?;
    let child = temp.path().join("admitted-child");
    crate::support::write_test_skill_package(
        &child,
        "---\nname: admitted-child\ndescription: Count a previously admitted synthetic snapshot.\n---\n# Admitted records\n\nRead the registered snapshot without reopening a file.\n",
        r#"minimum_runx: 0.12.0
methods:
  count:
    inputs:
      source: {type: string}
    steps:
    - id: page
      inputs: {source_uri: $input.source}
      tool:
        module: page.mjs
        paged_input: {artifact: source_uri, framing: json_array, page_bytes: 65536}
      outputs: {record_count: {type: number}}
"#,
    )?;
    std::fs::write(
        child.join("page.mjs"),
        "export default ({runx_page:page}) => {const count=(page.state?.count??0)+page.records.length;return {runx_page:{state:{count},done:page.eof},...(page.eof?{record_count:count}:{})};};\n",
    )?;
    let parent = temp.path().join("admitted-parent");
    crate::support::write_test_skill_package(
        &parent,
        "---\nname: admitted-parent\ndescription: Retain and process one selected record array natively.\n---\n# Retained records\n\nStore, open and pass the native snapshot to the existing child.\n",
        r#"minimum_runx: 0.12.0
methods:
  count:
    inputs:
      records: {type: array}
    result_from: [count]
    steps:
    - id: retain
      tool: artifact.store
      inputs: {value: $input.records}
    - id: open
      tool: artifact.open
      inputs: {artifact: $steps.retain.artifact}
    - id: count
      skill: ../admitted-child
      inputs: {source: $steps.open.artifact.ref}
"#,
    )?;
    let records = (0..5_000)
        .map(|id| serde_json::json!({"id":id,"text":"日本語 😀"}))
        .collect::<Vec<_>>();
    let values = input(serde_json::json!({"records":records}))?;
    let allowed = run_page_fixture(temp.path(), parent.clone(), values.clone(), Reach::Inputs)?;
    assert!(allowed.succeeded(), "{allowed:?}");
    assert_eq!(
        serde_json::to_value(&allowed.output)?["result"]["record_count"],
        5_000
    );
    let denied = run_page_fixture(temp.path(), parent, values, Reach::Workspace)?;
    assert!(!denied.succeeded(), "{denied:?}");
    assert!(
        format!("{:?}", denied.output).contains("protected operator state"),
        "{denied:?}"
    );
    Ok(())
}

#[test]
fn paged_file_reads_keep_the_real_workflow_callers_reach() -> TestResult {
    let temp = tempdir()?;
    std::fs::write(temp.path().join("ordinary.json"), "[{}]")?;
    std::fs::write(temp.path().join(".env.page-proof"), "[{}]")?;
    std::fs::create_dir_all(temp.path().join(".runx"))?;
    std::fs::write(temp.path().join(".runx/page-proof.json"), "[{}]")?;
    let skill = write_paged_read_skill(temp.path())?;
    assert!(
        run_paged_read(
            temp.path(),
            skill.clone(),
            "ordinary.json",
            Reach::Workspace
        )?
        .succeeded()
    );
    for path in [".env.page-proof", ".runx/page-proof.json"] {
        let denied = run_paged_read(temp.path(), skill.clone(), path, Reach::Workspace)?;
        assert!(
            !denied.succeeded(),
            "protected page source was admitted: {denied:?}"
        );
        assert!(
            format!("{:?}", denied.output).contains("secret"),
            "{denied:?}"
        );
        assert!(run_paged_read(temp.path(), skill.clone(), path, Reach::Inputs)?.succeeded());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(".env.page-proof", temp.path().join("alias.json"))?;
        let denied = run_paged_read(temp.path(), skill, "alias.json", Reach::Workspace)?;
        assert!(!denied.succeeded(), "{denied:?}");
        assert!(
            format!("{:?}", denied.output).contains("secret"),
            "{denied:?}"
        );
    }
    Ok(())
}

#[test]
fn paged_file_reads_keep_reach_workspace_when_an_agent_calls_a_child_skill() -> TestResult {
    let temp = tempdir()?;
    std::fs::write(temp.path().join("ordinary.json"), "[{}]")?;
    std::fs::write(temp.path().join(".env.page-proof"), "[{}]")?;
    write_paged_read_skill(temp.path())?;
    let parent = temp.path().join("paged-parent");
    crate::support::write_test_skill_package(
        &parent,
        "---\nname: paged-parent\ndescription: Reuses a declared native page reader.\n---\n# Page parent\n\n## Agent task: answer\n\nUse only the admitted child reader.\n",
        "minimum_runx: 0.12.0\nmethods:\n  answer:\n    inputs:\n      question: {type: string}\n    steps:\n      - id: answer\n        agent: answer\n        calls:\n          skills: [../paged-child]\n        inputs: $input\n        outputs: {answer: {type: string}}\n",
    )?;
    let mut authority = bounded(None, None);
    authority.reach = Reach::Workspace;
    let started = start_bounded(temp.path(), parent, authority)?;
    let turn = opened(temp.path(), &started)?;
    let denied = turn.call(
        TURN,
        "skill__paged-child",
        None,
        input(serde_json::json!({"path": ".env.page-proof"}))?,
    )?;
    assert!(!completed(&denied), "{denied:?}");
    assert!(format!("{denied:?}").contains("secret"), "{denied:?}");
    let allowed = turn.call(
        TURN,
        "skill__paged-child",
        None,
        input(serde_json::json!({"path": "ordinary.json"}))?,
    )?;
    assert!(completed(&allowed), "{allowed:?}");
    Ok(())
}

#[test]
fn a_method_excludes_provider_calls_before_connector_authentication() -> TestResult {
    let temp = tempdir()?;
    crate::support::plugin_with_fixture_connectors(temp.path(), &["fixture/forge"])?;
    let skill_dir = temp.path().join("source-review");
    // The package calls the forge, from another method; this method's
    // turn names none of its operations.
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: source-review\ndescription: Interpret supplied evidence.\n---\n# Source review\n\n## Agent task: answer\n\nUse the supplied evidence without a provider read.\n",
        r#"minimum_runx: 0.12.0
default_method: answer
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        inputs: $input
        outputs:
          answer: {type: string}
  provider:
    steps:
      - id: read
        operation: fixture/forge/issue.read
      - id: push
        operation: fixture/forge/branch.push
        idempotency_key: $run.id
"#,
    )?;
    let paused = Paused::start(temp.path(), skill_dir, TURN)?;
    let turn = paused.open()?;
    assert!(turn.status(TURN)?.tools.is_empty());
    for tool in ["fixture_forge__issue_read", "fixture_forge__branch_push"] {
        let error = turn
            .call(
                TURN,
                tool,
                None,
                input(
                    serde_json::json!({"input": {"repository": "example/project", "number": 1}}),
                )?,
            )
            .err()
            .ok_or("an excluded provider call was admitted")?;
        assert!(error.to_string().contains("offers no tool"), "{error}");
    }
    assert!(turn.status(TURN)?.tools.is_empty());
    Ok(())
}

#[test]
fn reach_workspace_keeps_a_turns_file_calls_inside_the_workspace_and_off_secrets() -> TestResult {
    let temp = tempdir()?;
    let elsewhere = tempdir()?;
    std::fs::write(temp.path().join("README.md"), "# Project\n")?;
    std::fs::write(temp.path().join(".env"), "TOKEN=1\n")?;
    std::fs::write(elsewhere.path().join("notes.md"), "elsewhere\n")?;
    let skill_dir = write_file_turn_skill(temp.path())?;
    let mut authority = bounded(None, None);
    authority.reach = Reach::Workspace;
    let started = start_bounded(temp.path(), skill_dir.clone(), authority)?;
    let turn = opened(temp.path(), &started)?;
    let read = |inputs: serde_json::Value| -> TestResult<TurnCallResult> {
        Ok(turn.call(TURN, "fs_read", None, input(inputs)?)?)
    };

    assert!(completed(&read(
        serde_json::json!({ "path": "README.md" })
    )?));
    let secret = read(serde_json::json!({ "path": ".env" }))?;
    assert!(!completed(&secret), "{secret:?}");
    let outside = read(serde_json::json!({
        "repo_root": elsewhere.path().to_string_lossy(),
        "path": "notes.md",
    }))?;
    assert!(!completed(&outside), "{outside:?}");
    let listed = turn.call(TURN, "fs_list", None, input(serde_json::json!({}))?)?;
    assert!(completed(&listed));
    assert!(!format!("{listed:?}").contains(".env"));
    let written = turn.call(
        TURN,
        "fs_write",
        None,
        input(serde_json::json!({ "path": ".env", "contents": "TOKEN=2" }))?,
    )?;
    assert!(!completed(&written), "{written:?}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".env"))?,
        "TOKEN=1\n"
    );

    // A run of the same skill that reaches its inputs reads what they name.
    let inputs = start_bounded(temp.path(), skill_dir, bounded(None, None))?;
    let inputs = opened(temp.path(), &inputs)?;
    let read = inputs.call(
        TURN,
        "fs_read",
        None,
        input(serde_json::json!({ "path": ".env" }))?,
    )?;
    assert!(completed(&read), "{read:?}");
    Ok(())
}

/// A one-turn skill whose agent may change files and run commands.
fn write_develop_skill(root: &Path) -> TestResult<PathBuf> {
    let skill_dir = root.join("develop-demo");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: develop-demo\ndescription: Changes the project, then answers.\n---\n# Develop demo\n\n## Agent task: answer\n\nChange what you need, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          tools: [fs.write, command.execute]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    Ok(skill_dir)
}

/// What a host gives its agent: `effects: ask` and `reach: workspace`, with
/// the person's standing approvals.
fn asking(standing: serde_json::Value) -> TestResult<runx_runtime::RunAuthority> {
    let mut authority = bounded(None, None);
    authority.effects = Effects::Ask;
    authority.reach = Reach::Workspace;
    authority.standing_approvals = serde_json::from_value(standing)?;
    Ok(authority)
}

/// The run's open floor approval at `address`: its gate.
fn floor_gate(call: &TurnCallResult, address: &str) -> TestResult<runx_contracts::ApprovalGate> {
    let TurnCallOutcome::Pending { waiting } = &call.outcome else {
        return Err(format!("the call waits on the person: {call:?}").into());
    };
    match waiting.as_slice() {
        [ResolutionRequest::Approval { id, gate }] if id.as_str() == address => {
            Ok(gate.as_ref().clone())
        }
        _ => Err(format!("one floor approval at {address}: {waiting:?}").into()),
    }
}

fn resume_document(
    root: &Path,
    run_id: &str,
    document: runx_contracts::ResumeDocument,
) -> TestResult<RunResult> {
    Ok(LocalOrchestrator::default().resume(&ResumeRequest {
        run_id: run_id.to_owned(),
        answers: document,
        receipt_dir: Some(root.join("receipts")),
        env: env(root),
        cwd: root.to_path_buf(),
        managed_agent: ManagedAgentPolicy::HostDriven,
        package_digest: None,
        execution_closure_digest: None,
    })?)
}

#[test]
fn the_floor_holds_an_agents_edit_until_the_person_approves_its_exact_diff() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    std::fs::write(root.join("README.md"), "old\n")?;
    let started = start_bounded(
        root,
        write_develop_skill(root)?,
        asking(serde_json::json!([]))?,
    )?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let turn = opened(root, &started)?;
    let write = |turn: &TurnSession, inputs: serde_json::Value| -> TestResult<TurnCallResult> {
        Ok(turn.call(TURN, "fs_write", None, input(inputs)?)?)
    };

    let held = write(
        &turn,
        serde_json::json!({
            "path": "README.md",
            "contents": "new\n",
            "expected_digest": runx_contracts::sha256_prefixed(b"old\n"),
        }),
    )?;
    let gate = floor_gate(&held, "answer/0/tool")?;
    assert_eq!(gate.gate_type.as_deref(), Some("tool"));
    let payload = gate
        .payload
        .clone()
        .ok_or("the gate carries what executes")?;
    let file = serde_json::to_value(&payload)?["files"][0].clone();
    assert_eq!(file["path"], "README.md");
    let diff = file["diff"].as_str().unwrap_or_default();
    assert!(diff.contains("-old") && diff.contains("+new"), "{diff}");
    assert_eq!(
        gate.summary
            .as_ref()
            .and_then(|summary| summary.get("payload_digest")),
        Some(&JsonValue::String(runx_contracts::canonical_digest(
            &payload
        )?))
    );
    assert_eq!(std::fs::read_to_string(root.join("README.md"))?, "old\n");
    // It waits on the person, never "may have run".
    let paused = Paused {
        root: root.to_path_buf(),
        receipts: root.join("receipts"),
        run_id: run_id.clone(),
    };
    assert!(
        matches!(
            recorded_calls(&paused)?.as_slice(),
            [(_, RunCallOutcome::Pending { .. })]
        ),
        "the floor decided before the claim"
    );

    drop(turn);
    paused.resume(serde_json::json!({"approvals": {"answer/0/tool": true}}))?;
    assert_eq!(std::fs::read_to_string(root.join("README.md"))?, "new\n");
    let turn = paused.open()?;
    assert_eq!(statuses(&turn.status(TURN)?), ["completed"]);

    let declined = write(
        &turn,
        serde_json::json!({"path": "other.md", "contents": "no\n", "create": true}),
    )?;
    floor_gate(&declined, "answer/1/tool")?;
    drop(turn);
    paused.resume(serde_json::json!({"approvals": {"answer/1/tool": false}}))?;
    // A person's no reads as theirs, never as runx's refusal (changed 10
    // November: it read as refused).
    let turn = paused.open()?;
    assert_eq!(statuses(&turn.status(TURN)?), ["completed", "declined"]);
    assert!(!root.join("other.md").exists());
    Ok(())
}

#[test]
fn the_floor_holds_a_process_start_on_its_exact_argv_and_reads_run_free() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let skill_dir = root.join("serve-demo");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: serve-demo\ndescription: Serves the project.\n---\n# Serve demo\n\n## Agent task: answer\n\nServe it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  answer:\n    inputs:\n      question: {type: string}\n    steps:\n      - id: answer\n        agent: answer\n        calls:\n          tools: [process.start, process.list]\n        inputs: $input\n        outputs:\n          answer: {type: string}\n",
    )?;
    let started = start_bounded(root, skill_dir, asking(serde_json::json!([]))?)?;
    let turn = opened(root, &started)?;

    let held = turn.call(
        TURN,
        "process_start",
        None,
        input(serde_json::json!({"command": "sh", "args": ["-c", "sleep 1"], "max_seconds": 30}))?,
    )?;
    let gate = floor_gate(&held, "answer/0/tool")?;
    let payload = serde_json::to_value(gate.payload.ok_or("the gate carries what runs")?)?;
    assert_eq!(payload["kind"], "process");
    assert_eq!(payload["argv"], serde_json::json!(["sh", "-c", "sleep 1"]));
    assert_eq!(payload["max_seconds"], 30);

    let listed = turn.call(TURN, "process_list", None, JsonObject::new())?;
    assert!(
        matches!(&listed.outcome, TurnCallOutcome::Completed { output, .. } if output.contains_key("process_list")),
        "{listed:?}"
    );
    Ok(())
}

#[test]
fn a_standing_approval_admits_what_it_covers_and_the_record_names_it() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let standing = serde_json::json!([
        {"edits": {"paths": ["docs"]}},
        {"commands": {"argv_prefix": ["echo"]}}
    ]);
    let started = start_bounded(root, write_develop_skill(root)?, asking(standing)?)?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let paused = Paused {
        root: root.to_path_buf(),
        receipts: root.join("receipts"),
        run_id: run_id.clone(),
    };
    let turn = opened(root, &started)?;
    let write = |turn: &TurnSession, path: &str| -> TestResult<TurnCallResult> {
        Ok(turn.call(
            TURN,
            "fs_write",
            None,
            input(serde_json::json!({"path": path, "contents": "made\n"}))?,
        )?)
    };
    let run = |command: &str| -> TestResult<TurnCallResult> {
        Ok(turn.call(
            TURN,
            "command_execute",
            None,
            input(serde_json::json!({"command": command, "args": ["hi"]}))?,
        )?)
    };

    assert!(
        completed(&write(&turn, "docs/a.md")?),
        "edits under docs run at once"
    );
    assert!(
        completed(&run("echo")?),
        "a command under its prefix runs at once"
    );
    let recorded = recorded_calls(&paused)?;
    let read = runx_runtime::run_record::read_run(&env(root), root, &run_id, None)?;
    let standing_of = |index: u32| {
        read.events
            .iter()
            .rev()
            .find_map(|record| match &record.event {
                RunEventFact::Call {
                    index: at,
                    standing,
                    ..
                } if *at == index => Some(standing.clone()),
                _ => None,
            })
    };
    assert!(
        standing_of(0)
            .flatten()
            .is_some_and(|entry| entry.edits.is_some())
    );
    assert!(
        standing_of(1)
            .flatten()
            .is_some_and(|entry| entry.commands.is_some())
    );
    assert_eq!(recorded.len(), 2);

    // What the set does not cover waits; the person's new set covers it, and
    // every later step runs under it.
    floor_gate(&write(&turn, "src/b.rs")?, "answer/2/tool")?;
    drop(turn);
    resume_document(
        root,
        &run_id,
        runx_contracts::ResumeDocument {
            standing_approvals: Some(serde_json::from_value(
                serde_json::json!([{"local": true}]),
            )?),
            ..runx_contracts::ResumeDocument::default()
        },
    )?;
    assert_eq!(std::fs::read_to_string(root.join("src/b.rs"))?, "made\n");
    let turn = paused.open()?;
    assert!(completed(&write(&turn, "anywhere.md")?));
    let read = runx_runtime::run_record::read_run(&env(root), root, &run_id, None)?;
    assert!(read.events.iter().any(|record| matches!(
        &record.event,
        RunEventFact::Standing { approvals } if approvals.iter().any(|entry| entry.local)
    )));
    Ok(())
}

#[test]
fn an_agent_asks_the_person_questions_and_reads_their_answers() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let turn = paused.open()?;
    let asked = turn.call(
        TURN,
        "ask_human",
        None,
        input(serde_json::json!({"questions": [
            {"id": "base", "prompt": "Which base?", "options": [{"label": "main"}, {"label": "next", "detail": "the next release"}]},
            {"id": "note", "prompt": "Anything else?", "required": false}
        ]}))?,
    )?;
    let TurnCallOutcome::Pending { waiting } = &asked.outcome else {
        return Err(format!("the ask waits on the person: {asked:?}").into());
    };
    let [ResolutionRequest::Input { id, questions }] = waiting.as_slice() else {
        return Err(format!("one input request: {waiting:?}").into());
    };
    assert_eq!(id.as_str(), "answer/0");
    assert_eq!(questions[0].question_type.as_str(), "choice");
    assert!(
        questions[1].free_text,
        "a question without options takes text"
    );
    drop(turn);

    let refused = paused
        .resume(
            serde_json::json!({"approvals": {"answer/0": {"value": {"base": {"option": "dev"}}}}}),
        )
        .err()
        .ok_or("an answer outside the options is refused")?;
    assert!(refused.to_string().contains("not an option"), "{refused}");
    paused.resume(serde_json::json!({"approvals": {"answer/0": {"value": {
        "base": {"option": "next"},
        "note": {"text": "ship it"}
    }}}}))?;
    let turn = paused.open()?;
    let [call] = turn
        .status(TURN)?
        .calls
        .try_into()
        .map_err(|_| "one call")?;
    let TurnCallOutcome::Completed { output, .. } = &call.outcome else {
        return Err(format!("the ask completed: {call:?}").into());
    };
    assert_eq!(
        serde_json::to_value(output)?["answers"]["base"]["option"],
        "next"
    );
    Ok(())
}

#[test]
fn the_floor_holds_a_skills_workflow_step_before_it_is_claimed() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    let skill_dir = root.join("write-step");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: write-step\ndescription: Writes its notes.\n---\n# Write step\n\nWrite the notes.\n",
        r#"minimum_runx: 0.12.0
methods:
  write:
    inputs:
      question: {type: string}
    steps:
      - id: write
        tool: fs.write
        inputs:
          path: notes.md
          contents: hello
"#,
    )?;
    let started = start_bounded(root, skill_dir, asking(serde_json::json!([]))?)?;
    assert_eq!(requests(&started)?, ["write/tool"]);
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let read = runx_runtime::run_record::read_run(&env(root), root, &run_id, None)?;
    assert!(
        !read.events.iter().any(|record| matches!(
            record.event,
            RunEventFact::Suspended {
                hold: runx_contracts::RunHold::Claimed { .. },
                ..
            }
        )),
        "nothing waiting on the person was claimed"
    );
    assert!(!root.join("notes.md").exists());

    let paused = Paused {
        root: root.to_path_buf(),
        receipts: root.join("receipts"),
        run_id,
    };
    let sealed = paused.resume(serde_json::json!({"approvals": {"write/tool": true}}))?;
    assert!(sealed.succeeded(), "{:?}", sealed.output);
    assert_eq!(std::fs::read_to_string(root.join("notes.md"))?, "hello");
    Ok(())
}

/// The calls a run's record holds, each with its pinned id and the outcome
/// its latest event records, in index order.
fn recorded_calls(paused: &Paused) -> TestResult<Vec<(Option<String>, RunCallOutcome)>> {
    let read =
        runx_runtime::run_record::read_run(&env(&paused.root), &paused.root, &paused.run_id, None)?;
    let mut calls = BTreeMap::new();
    for record in read.events {
        if let RunEventFact::Call {
            index,
            call_id,
            outcome,
            ..
        } = record.event
        {
            calls.insert(index, (call_id.map(|id| id.as_str().to_owned()), *outcome));
        }
    }
    Ok(calls.into_values().collect())
}

#[test]
fn a_pinned_call_runs_once_and_its_outcome_is_on_the_runs_record() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_turn_skill(temp.path())?, TURN)?;
    let turn = paused.open()?;
    let hello = || input(serde_json::json!({"value": "hello", "encoding": "utf8_text"}));
    let first = turn.call(TURN, "data_digest", Some("toolu_1"), hello()?)?;
    assert_eq!(
        first.call_id.as_ref().map(|id| id.as_str()),
        Some("toolu_1")
    );

    // Made again with its id, the call returns as recorded and runs once.
    assert_eq!(
        turn.call(TURN, "data_digest", Some("toolu_1"), hello()?)?,
        first
    );
    assert_eq!(turn.status(TURN)?.calls.len(), 1);
    let other = turn
        .call(
            TURN,
            "data_digest",
            Some("toolu_1"),
            input(serde_json::json!({"value": "other", "encoding": "utf8_text"}))?,
        )
        .err()
        .ok_or("an id names one call")?;
    assert!(
        other.to_string().contains("another target or input"),
        "{other}"
    );
    assert!(
        turn.call(TURN, "data_digest", Some("two words"), hello()?)
            .is_err()
    );
    turn.call(TURN, "ask_human", Some("toolu_2"), JsonObject::new())?;

    // The record names each call by its id, holds a completed call's output
    // as project artifacts, and says why a refused call was refused.
    let calls = recorded_calls(&paused)?;
    let [(Some(first_id), completed), (Some(second_id), refused)] = calls.as_slice() else {
        return Err(format!("two pinned calls: {calls:?}").into());
    };
    assert_eq!(
        (first_id.as_str(), second_id.as_str()),
        ("toolu_1", "toolu_2")
    );
    assert!(
        matches!(refused, RunCallOutcome::Refused { reason } if reason.as_str().contains("reason")),
        "{refused:?}"
    );
    let RunCallOutcome::Completed {
        succeeded: true,
        outputs,
        reason: None,
        ..
    } = completed
    else {
        return Err(format!("a completed call: {completed:?}").into());
    };
    let TurnCallOutcome::Completed { output, .. } = &first.outcome else {
        return Err("the first call completed".into());
    };
    // The artifacts rebuild the output the agent read: a packet's data under
    // its name, and the rest as it is.
    let mut rebuilt = serde_json::Map::new();
    for part in outputs {
        assert_eq!(part.artifact.media_type, "application/json");
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(
            part.artifact
                .path
                .as_deref()
                .ok_or("the output names its path")?,
        )?)?;
        match &part.output {
            Some(name) => {
                rebuilt.insert(
                    name.as_str().to_owned(),
                    serde_json::json!({ "data": value }),
                );
            }
            None => rebuilt.extend(value.as_object().cloned().unwrap_or_default()),
        }
    }
    assert!(!outputs.is_empty());
    assert_eq!(
        serde_json::Value::Object(rebuilt),
        serde_json::to_value(output)?
    );
    Ok(())
}

#[test]
fn an_act_that_fails_says_why_and_its_skill_call_settles_failed() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_answer_skills(temp.path())?, TURN)?;
    let turn = paused.open()?;
    turn.call(
        TURN,
        "skill__answer-child",
        Some("child"),
        JsonObject::new(),
    )?;
    let child_turn = "answer/0#answer";
    assert!(
        turn.fail(child_turn, " ", None).is_err(),
        "a failure says why"
    );
    turn.fail(child_turn, "the source is gone", None)?;

    let [call] = turn
        .status(TURN)?
        .calls
        .try_into()
        .map_err(|_| "one call")?;
    assert!(
        matches!(&call.outcome, TurnCallOutcome::Failed { reason, .. } if reason.as_str().contains("the source is gone")),
        "{call:?}"
    );
    let recorded = recorded_calls(&paused)?;
    assert!(
        // The child ran, and sealed its failure: the call completed without
        // succeeding, and its receipt holds the child's work.
        matches!(recorded.as_slice(), [(Some(id), RunCallOutcome::Completed { succeeded: false, reason: Some(reason), .. })] if id == "child" && reason.as_str().contains("the source is gone")),
        "{recorded:?}"
    );

    // The calling agent fails its own act; the run seals failed.
    let sealed = turn.fail(TURN, "nothing to answer with", None)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "outcome"), Some("failed"), "{output:?}");
    Ok(())
}

/// A one-turn skill whose turn requires a fresh context of its agent harness.
fn write_requiring_skill(root: &Path) -> TestResult<PathBuf> {
    let skill_dir = root.join("judge");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: judge\ndescription: Judges from its purpose alone.\n---\n# Judge\n\n## Agent task: answer\n\nJudge it from its purpose alone.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        requires: [fresh_context]
        context: {kinds: [people]}
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    Ok(skill_dir)
}

/// A turn's requirement reaches its host, and is never assumed met: a run
/// whose answer doesn't say its agent harness met it seals failed, saying
/// so; one that says so seals completed; and one whose agent harness
/// couldn't ends the act failed with the requirement named.
#[test]
fn a_turns_requirement_reaches_its_host_and_is_never_assumed_met() -> TestResult {
    let fresh = runx_contracts::TurnRequirement::FreshContext;
    let start = || -> TestResult<(tempfile::TempDir, Paused)> {
        let temp = tempdir()?;
        let paused = Paused::start(temp.path(), write_requiring_skill(temp.path())?, TURN)?;
        Ok((temp, paused))
    };

    let (_temp, paused) = start()?;
    let status = paused.open()?.status(TURN)?;
    assert_eq!(status.invocation.requires, [fresh]);
    assert_eq!(
        status
            .invocation
            .context_kinds
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>(),
        ["people"],
        "the host is asked for the kinds the step declares"
    );
    let unstated = paused
        .open()?
        .complete(TURN, input(serde_json::json!({"answer": "yes"}))?)?;
    let output = object(&unstated.output)?;
    assert_eq!(text(output, "outcome"), Some("failed"), "{output:?}");
    assert!(
        serde_json::to_string(output)?.contains(
            "requires fresh_context, and its answer doesn't say its agent harness met it"
        ),
        "{output:?}"
    );

    let (_temp, paused) = start()?;
    let sealed = paused.open()?.complete(
        TURN,
        input(serde_json::json!({
            "answer": "yes",
            "ending": "completed", "requirements": ["fresh_context"],
        }))?,
    )?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "status"), Some("sealed"), "{output:?}");
    assert_eq!(text(output, "outcome"), Some("completed"), "{output:?}");

    let (_temp, paused) = start()?;
    let refused = paused
        .open()?
        .fail(TURN, "this agent harness has no subagent", Some(fresh))?;
    let output = object(&refused.output)?;
    assert_eq!(text(output, "outcome"), Some("failed"), "{output:?}");
    assert!(
        serde_json::to_string(output)?.contains("unsupported_requirement fresh_context"),
        "{output:?}"
    );
    Ok(())
}

/// A one-turn skill that may digest data and hand a digest packet to
/// `digest-reader`, whose method takes it as a `runx.data.digest.v1` and
/// digests the digest it names.
fn write_packet_skills(root: &Path) -> TestResult<PathBuf> {
    let reader = root.join("digest-reader");
    crate::support::write_test_skill_package(
        &reader,
        "---\nname: digest-reader\ndescription: Digests a digest packet's digest.\n---\n# Digest reader\n\nDigest the digest.\n",
        r#"minimum_runx: 0.12.0
methods:
  read:
    inputs:
      packet: {packet: runx.data.digest.v1}
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: $input.packet.digest
          encoding: utf8_text
"#,
    )?;
    let skill_dir = root.join("packet-turn");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: packet-turn\ndescription: Hands packets on.\n---\n# Packet turn\n\n## Agent task: answer\n\nDigest, then hand the digest on.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          tools: [data.digest]
          skills: [../digest-reader]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    Ok(skill_dir)
}

#[test]
fn a_remote_turns_call_whose_result_is_local_only_is_refused() -> TestResult {
    let temp = tempdir()?;
    std::fs::write(temp.path().join("kept.txt"), "kept on this machine")?;
    std::fs::write(temp.path().join("shared.txt"), "shared with the model")?;
    let parent = temp.path().join("local-reader");
    crate::support::write_test_skill_package(
        &parent,
        "---\nname: local-reader\ndescription: Reads admitted notes in a turn.\n---\n# Local reader\n\n## Agent task: answer\n\nRead the admitted notes.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: shared
        tool: artifact.admit
        inputs: {path: shared.txt, media_type: text/plain}
      - id: answer
        agent: answer
        calls:
          tools: [artifact.admit, artifact.read]
        inputs:
          question: $input.question
          shared: $steps.shared.artifact.ref
        outputs:
          answer: {type: string}
"#,
    )?;
    let paused = Paused::start(temp.path(), parent, TURN)?;
    let turn = paused.open()?;

    // This turn's host may run its model anywhere: no managed agent works it.
    // What it admits as local_only is refused to it, reference and all.
    let kept = turn.call(
        TURN,
        "artifact_admit",
        None,
        input(serde_json::json!({
            "path": "kept.txt",
            "media_type": "text/plain",
            "labels": {"sharing": "local_only"}
        }))?,
    )?;
    assert!(
        matches!(&kept.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains("local_only")),
        "{kept:?}"
    );
    // The record says the same however the turn is read again.
    let page = turn.read(TURN, &format!("call:{}", kept.index), 0, None)?;
    assert!(!page.text.contains("runx:local-artifact:"), "{}", page.text);
    let refused = turn
        .status(TURN)?
        .calls
        .into_iter()
        .find(|call| call.index == kept.index)
        .ok_or("the turn lists the call")?;
    assert!(
        matches!(refused.outcome, TurnCallOutcome::Refused { .. }),
        "{refused:?}"
    );
    // What the configured model may see is given.
    let reference = turn.read(TURN, "input:shared", 0, None)?.text;
    let shared = turn.call(
        TURN,
        "artifact_read",
        None,
        input(serde_json::json!({"ref": reference, "encoding": "utf8"}))?,
    )?;
    assert!(completed(&shared), "{shared:?}");
    assert!(
        format!("{shared:?}").contains("shared with the model"),
        "{shared:?}"
    );
    Ok(())
}

#[test]
fn a_project_artifact_packet_input_receives_the_descriptor_instead_of_its_content() -> TestResult {
    let temp = tempdir()?;
    let child = temp.path().join("artifact-child");
    crate::support::write_test_skill_package(
        &child,
        "---\nname: artifact-child\ndescription: Stores and opens immutable values.\n---\n# Artifact child\n\nStore and reopen selected values.\n",
        r#"minimum_runx: 0.12.0
methods:
  store:
    inputs:
      value: {}
    steps:
      - id: store
        tool: artifact.store
        inputs:
          value: $input.value
"#,
    )?;
    crate::support::write_test_skill_package(
        &temp.path().join("artifact-reader"),
        "---\nname: artifact-reader\ndescription: Reads a retained value by its descriptor.\n---\n# Artifact reader\n\nOpen an exact retained value without substituting its contents as the descriptor.\n",
        r#"minimum_runx: 0.12.0
methods:
  read:
    inputs:
      cursor: {packet: runx.artifact.v1}
    steps:
      - id: open
        tool: artifact.open
        inputs:
          artifact: $input.cursor
      - id: read
        tool: artifact.read
        inputs:
          ref: $steps.open.artifact.ref
          encoding: json_array
"#,
    )?;
    let parent = temp.path().join("artifact-turn");
    crate::support::write_test_skill_package(
        &parent,
        "---\nname: artifact-turn\ndescription: Hands retained artifact descriptors to child skills.\n---\n# Artifact turn\n\n## Agent task: answer\n\nContinue retained work through a declared child skill.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          skills: [../artifact-child, ../artifact-reader]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    let paused = Paused::start(temp.path(), parent, TURN)?;
    let turn = paused.open()?;
    let stored = turn.call(
        TURN,
        "skill__artifact-child",
        None,
        input(serde_json::json!({"value": [{"id": 1, "label": "日本語"}]}))?,
    )?;
    let TurnCallOutcome::Completed { output, .. } = stored.outcome else {
        return Err(format!("store did not complete: {stored:?}").into());
    };
    let descriptor = output
        .get("artifact")
        .and_then(JsonValue::as_object)
        .and_then(|value| value.get("data"))
        .ok_or("store returned its exact descriptor")?;
    let reopened = turn.call(
        TURN,
        "skill__artifact-reader",
        None,
        input(serde_json::json!({"cursor": descriptor}))?,
    )?;
    let TurnCallOutcome::Completed { output, .. } = reopened.outcome else {
        return Err(format!("open did not complete: {reopened:?}").into());
    };
    let page = &serde_json::to_value(output)?["artifact_page"]["data"];
    let records = page["records"].as_array().ok_or("framed records")?;
    assert_eq!(records.len(), 1);
    let record: serde_json::Value =
        serde_json::from_str(records[0].as_str().ok_or("record text")?)?;
    assert_eq!(record, serde_json::json!({"id": 1, "label": "日本語"}));
    assert_eq!(page["digest"], serde_json::to_value(descriptor)?["digest"]);
    turn.complete(TURN, input(serde_json::json!({"answer": "reopened"}))?)?;
    Ok(())
}

#[test]
fn a_packet_one_call_returned_feeds_the_next_by_reference() -> TestResult {
    let temp = tempdir()?;
    let paused = Paused::start(temp.path(), write_packet_skills(temp.path())?, TURN)?;
    let turn = paused.open()?;

    // The reader's packet input takes the packet or a reference to one.
    let status = turn.status(TURN)?;
    let reader = status
        .tools
        .iter()
        .find(|tool| tool.name.as_str() == "skill__digest-reader")
        .ok_or("the reader is offered")?;
    let branches = reader.input_schema["properties"]
        .as_object()
        .and_then(|properties| properties.get("packet"))
        .and_then(JsonValue::as_object)
        .and_then(|packet| packet.get("anyOf"))
        .and_then(JsonValue::as_array)
        .ok_or("the packet input has two forms")?;
    assert_eq!(branches.len(), 2);
    assert_eq!(
        text(object(&branches[0])?, "x-runx-packet-id"),
        Some("runx.data.digest.v1")
    );

    let digest = |value: &str| {
        turn.call(
            TURN,
            "data_digest",
            None,
            input(serde_json::json!({"value": value, "encoding": "utf8_text"}))?,
        )
        .map_err(Box::<dyn std::error::Error>::from)
    };
    let hello = digest("hello")?;
    let TurnCallOutcome::Completed {
        output, artifacts, ..
    } = &hello.outcome
    else {
        return Err(format!("the digest completed: {hello:?}").into());
    };
    let reference = artifacts
        .get("digest_result")
        .ok_or("the packet output is an artifact")?;
    let named = serde_json::to_value(output)?["digest_result"]["data"]["digest"]
        .as_str()
        .ok_or("the digest names a digest")?
        .to_owned();

    // The reader takes the packet by its reference, as the call recorded it.
    let read = turn.call(
        TURN,
        "skill__digest-reader",
        None,
        input(serde_json::json!({"packet": reference}))?,
    )?;
    let TurnCallOutcome::Completed {
        output: read_output,
        receipt_id,
        ..
    } = &read.outcome
    else {
        return Err(format!("the reader completed: {read:?}").into());
    };
    let TurnCallOutcome::Completed {
        output: expected, ..
    } = digest(&named)?.outcome
    else {
        return Err("the digest of the digest completed".into());
    };
    assert_eq!(
        serde_json::to_value(read_output)?["digest_result"]["data"],
        serde_json::to_value(&expected)?["digest_result"]["data"],
        "the reader digested the packet the reference names"
    );

    // A packet an earlier run left in the project feeds an input too, as a
    // host reuses one across sessions; one of another type, a reference whose
    // digest is not its content's, or one that leaves the project's artifact
    // store, is refused.
    let artifacts = runx_runtime::RunxPaths::resolve(&env(&paused.root), &paused.root).artifacts();
    let data = output
        .get("digest_result")
        .and_then(JsonValue::as_object)
        .and_then(|envelope| envelope.get("data"))
        .ok_or("the digest is a packet")?;
    let earlier = runx_runtime::persist_json_artifact(
        &artifacts,
        "rx_earlier",
        "packets",
        "digest",
        data,
        runx_contracts::ArtifactLabels::CONFIGURED_MODEL,
    )?;
    let reused = turn.call(
        TURN,
        "skill__digest-reader",
        None,
        input(serde_json::json!({"packet": earlier}))?,
    )?;
    assert!(
        matches!(reused.outcome, TurnCallOutcome::Completed { .. }),
        "{reused:?}"
    );
    let mut forged = serde_json::to_value(reference)?;
    let forged_digest = format!("sha256:{}", "0".repeat(64));
    forged["digest"] = serde_json::json!(forged_digest);
    forged["ref"] = serde_json::json!(format!("runx:project-artifact:{forged_digest}"));
    let outside = temp.path().join("outside.json");
    std::fs::write(&outside, br#"{"algorithm":"sha256"}"#)?;
    let mut escaped = forged.clone();
    escaped["path"] = serde_json::json!(outside);
    let mistyped = serde_json::to_value(runx_runtime::persist_json_artifact(
        &artifacts,
        "rx_earlier",
        "packets",
        "comparison",
        &serde_json::from_value(serde_json::json!({"equal": true}))?,
        runx_contracts::ArtifactLabels::CONFIGURED_MODEL,
    )?)?;
    for (bad, why) in [
        (mistyped, "declared schema"),
        (forged, "digest differs"),
        (escaped, "escapes"),
    ] {
        let refused = turn.call(
            TURN,
            "skill__digest-reader",
            None,
            input(serde_json::json!({"packet": bad}))?,
        )?;
        assert!(
            matches!(&refused.outcome, TurnCallOutcome::Refused { reason } if reason.as_str().contains(why)),
            "{refused:?}"
        );
    }

    // The reader's receipt names the artifact that fed its input, as
    // evidence of what it was given.
    turn.complete(TURN, input(serde_json::json!({"answer": "read"}))?)?;
    let receipt = crate::support::read_test_signed_receipt(&paused.receipts, receipt_id.as_str())?;
    let outcome = receipt
        .seal
        .criteria
        .iter()
        .find(|binding| binding.criterion_id == runx_contracts::Criterion::StepOutcome)
        .ok_or("the receipt seals its outcome")?;
    let fed = outcome
        .evidence_refs
        .iter()
        .find(|evidence| evidence.reference_type == runx_contracts::ReferenceType::Artifact)
        .ok_or("the receipt names the input's artifact")?;
    assert_eq!(fed.uri.as_str(), reference.reference.as_str());
    assert_eq!(
        fed.label.as_ref().map(|label| label.as_str()),
        Some("packet")
    );
    assert!(
        !outcome.verification_refs.contains(fed),
        "an input's artifact verifies nothing"
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

/// A child skill whose own step takes a malformed input fails the turn's
/// call with the child's reason; the turn stays open for its agent to decide.
#[test]
fn a_childs_malformed_step_input_fails_the_turns_call_not_the_run() -> TestResult {
    let temp = tempdir()?;
    crate::support::write_test_skill_package(
        &temp.path().join("bad-child"),
        "---\nname: bad-child\ndescription: Calls a tool without its input.\n---\n# Bad child\n\nTry it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  write:\n    steps:\n      - id: write\n        tool: fs.write\n        inputs: {path: child.txt, content: written}\n",
    )?;
    let skill_dir = temp.path().join("turn-bad");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: turn-bad\ndescription: Calls a child that fails.\n---\n# Turn bad\n\n## Agent task: answer\n\nCall it, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        effects: none
        calls:
          skills: [../bad-child]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    let paused = Paused::start(temp.path(), skill_dir, TURN)?;
    let turn = paused.open()?;
    let called = turn.call(TURN, "skill__bad-child", None, JsonObject::new())?;
    assert!(
        matches!(&called.outcome, TurnCallOutcome::Failed { reason, .. } if reason.as_str().contains("contents")),
        "{called:?}"
    );
    // The turn is still the agent's to answer.
    let sealed = turn.complete(TURN, input(serde_json::json!({"answer": "it failed"}))?)?;
    assert_eq!(text(object(&sealed.output)?, "status"), Some("sealed"));
    Ok(())
}

/// A read-only turn can't write through a script, at any depth: the turn's
/// `effects: none` reaches the script it calls and the child that script
/// calls, so the script's own write and its child's are refused, and
/// nothing is written.
#[test]
fn a_read_only_turn_cant_write_through_a_script_at_any_depth() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    crate::support::write_test_skill_package(
        &root.join("writer-child"),
        "---\nname: writer-child\ndescription: Writes a file.\n---\n# Writer child\n\nWrite it.\n",
        "minimum_runx: 0.12.0\nmethods:\n  write:\n    steps:\n      - id: write\n        tool: fs.write\n        inputs: {path: child.txt, contents: written}\n",
    )?;
    let writer = root.join("writer-script");
    crate::support::write_test_skill_package(
        &writer,
        "---\nname: writer-script\ndescription: Writes, and has its child write.\n---\n# Writer script\n\nWrite.\n",
        "minimum_runx: 0.12.0\nmethods:\n  write:\n    steps:\n      - id: write\n        script: {module: write.mjs}\n        calls:\n          tools: [fs.write]\n          skills: [../writer-child]\n        max_calls: 2\n        outputs:\n          own: {type: string}\n          child: {type: string}\n",
    )?;
    std::fs::write(
        writer.join("write.mjs"),
        r#"export default async function (ctx) {
  const reason = (call) => call.then(() => "ran", (rejected) => rejected.reason);
  return {
    own: await reason(ctx.tool("fs.write", { path: "own.txt", contents: "written" })),
    child: await reason(ctx.skill("../writer-child", {})),
  };
}
"#,
    )?;
    let skill_dir = root.join("read-only-turn");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: read-only-turn\ndescription: Reads through a script.\n---\n# Read only turn\n\nCall it, then answer.\n\n## Agent task: answer\n\nCall it, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        effects: none
        calls:
          skills: [../writer-script]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    let paused = Paused::start(root, skill_dir, TURN)?;
    let turn = paused.open()?;
    let called = turn.call(TURN, "skill__writer-script", None, JsonObject::new())?;
    let TurnCallOutcome::Completed { output, .. } = &called.outcome else {
        return Err(format!("the script ran and reported each write: {called:?}").into());
    };
    let output = serde_json::to_value(output)?;
    assert_eq!(output["own"], "refused", "{output}");
    assert_ne!(output["child"], "ran", "{output}");
    for written in [
        "own.txt",
        "child.txt",
        "writer-child/child.txt",
        "writer-script/own.txt",
    ] {
        assert!(!root.join(written).exists(), "{written} was written");
    }
    Ok(())
}

/// A turn's run draws its receipts from one budget, however its turn is
/// worked. The turn's first call runs a child that seals a step and then
/// waits on its ask; its tree's receipts are the run's already. A new call
/// that would take the tree past the host's budget is refused before it
/// runs, both in the session that made the first call and in a later one,
/// which counts the waiting child's recorded tree as the turn's record
/// loads. The run then seals with its tree at the budget exactly.
#[test]
fn a_call_past_the_budget_counts_a_waiting_childs_tree_and_is_refused() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path();
    crate::support::write_test_skill_package(
        &root.join("asking-child"),
        "---\nname: asking-child\ndescription: Digests a value, then asks.\n---\n# Asking child\n\nDigest, then ask.\n",
        r#"minimum_runx: 0.12.0
methods:
  confirm:
    inputs:
      value: {type: string}
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: $input.value
          encoding: utf8_text
      - id: approve
        ask: {gate: confirm, reason: Confirm the digest.}
"#,
    )?;
    let skill_dir = root.join("budget-turn");
    crate::support::write_test_skill_package(
        &skill_dir,
        "---\nname: budget-turn\ndescription: Digests through a child.\n---\n# Budget turn\n\n## Agent task: answer\n\nDigest, then answer.\n",
        r#"minimum_runx: 0.12.0
methods:
  answer:
    inputs:
      question: {type: string}
    steps:
      - id: answer
        agent: answer
        calls:
          tools: [data.digest]
          skills: [../asking-child]
        inputs: $input
        outputs:
          answer: {type: string}
"#,
    )?;
    // The root, its agent step, and the child call: its own receipt, its
    // child run's, and that run's two steps.
    let budget = 6;
    let mut budget_env = env(root);
    budget_env.insert("RUNX_MAX_RECEIPTS".to_owned(), budget.to_string());
    let receipts = root.join("receipts");
    let started = LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(receipts.clone()),
            run_id: None,
            answers: None,
            inputs: BTreeMap::from([(
                "question".to_owned(),
                JsonValue::String("what digests?".to_owned()),
            )]),
            env: budget_env.clone(),
            cwd: root.to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        None,
        None,
        None,
    )?;
    let run_id = text(object(&started.output)?, "run_id")
        .ok_or("missing run_id")?
        .to_owned();
    let open = || -> TestResult<TurnSession> {
        Ok(LocalOrchestrator::default().open_turns(&TurnRequest {
            run_id: run_id.clone(),
            receipt_dir: Some(receipts.clone()),
            env: budget_env.clone(),
            cwd: root.to_path_buf(),
        })?)
    };
    let digest = || input(serde_json::json!({"value": "more", "encoding": "utf8_text"}));
    let refused_at_the_budget = |call: &TurnCallResult| {
        matches!(&call.outcome, TurnCallOutcome::Refused { reason }
            if reason.as_str().contains(&format!("past its bound of {budget} receipts")))
    };

    let session = open()?;
    let asked = session.call(
        TURN,
        "skill__asking-child",
        None,
        input(serde_json::json!({"value": "child"}))?,
    )?;
    let ask = waiting_on(&asked)?;
    let refused = session.call(TURN, "data_digest", None, digest()?)?;
    assert!(refused_at_the_budget(&refused), "{refused:?}");
    drop(session);

    let session = open()?;
    let refused = session.call(TURN, "data_digest", None, digest()?)?;
    assert!(refused_at_the_budget(&refused), "{refused:?}");
    assert_eq!(
        statuses(&session.status(TURN)?),
        ["pending", "refused", "refused"]
    );
    drop(session);

    let approvals = ask
        .iter()
        .map(|id| (id.clone(), serde_json::Value::Bool(true)))
        .collect::<serde_json::Map<_, _>>();
    LocalOrchestrator::default().resume(&ResumeRequest {
        run_id: run_id.clone(),
        answers: crate::support::bind_resume(
            &budget_env,
            root,
            &run_id,
            serde_json::json!({ "approvals": approvals }),
        )?,
        receipt_dir: Some(receipts.clone()),
        env: budget_env.clone(),
        cwd: root.to_path_buf(),
        managed_agent: ManagedAgentPolicy::HostDriven,
        package_digest: None,
        execution_closure_digest: None,
    })?;
    let sealed = open()?.complete(TURN, input(serde_json::json!({"answer": "digested"}))?)?;
    let output = object(&sealed.output)?;
    assert_eq!(text(output, "outcome"), Some("completed"), "{output:?}");
    let receipt_id = text(output, "receipt_id").ok_or("missing receipt_id")?;
    let signature = crate::support::test_signature_config()?;
    let trees = runx_runtime::receipts::store::LocalReceiptStore::new(&receipts)
        .verify_trees(Some(receipt_id), signature.signature_policy())?;
    assert!(trees.valid(), "{trees:?}");
    assert_eq!(trees.trees[0].receipt_count, budget);
    Ok(())
}
