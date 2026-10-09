use runx_contracts::{AllowedOperation, TurnAllowance};
use runx_parser::{
    SkillRunnerManifest, StepKind, parse_graph_yaml, parse_runner_manifest_yaml, validate_graph,
    validate_runner_manifest,
};

fn manifest(yaml: &str) -> Result<SkillRunnerManifest, String> {
    validate_runner_manifest(parse_runner_manifest_yaml(yaml).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn refusal(yaml: &str) -> Result<String, String> {
    manifest(yaml)
        .err()
        .ok_or_else(|| "expected the manifest to be refused".to_owned())
}

fn allowance(manifest: &SkillRunnerManifest, runner: &str) -> Result<TurnAllowance, String> {
    let step = manifest
        .runners
        .get(runner)
        .and_then(|runner| runner.graph.steps.first())
        .ok_or_else(|| format!("runner {runner} has no step"))?;
    match &step.kind {
        StepKind::Agent(agent) => Ok(agent.allowance.clone()),
        kind => Err(format!("expected an agent step, got {kind:?}")),
    }
}

#[test]
fn every_agent_turn_may_call_the_skills_declared_limits() -> Result<(), String> {
    let manifest = manifest(
        r#"
skill: triage
connectors:
  github:
    operations: [issue.read, issue.comment]
tools: [fs.read, git.status]
skills: [./label]
runners:
  review:
    steps:
      - id: read
        agent: { role: reviewer, task: read }
      - id: reply
        agent: { role: writer, task: reply }
"#,
    )?;

    let expected = TurnAllowance {
        operations: vec![
            AllowedOperation {
                requirement: "github".into(),
                operation: "issue.read".into(),
            },
            AllowedOperation {
                requirement: "github".into(),
                operation: "issue.comment".into(),
            },
        ],
        tools: vec!["fs.read".into(), "git.status".into()],
        skills: vec!["./label".into()],
    };
    assert_eq!(manifest.tools, ["fs.read", "git.status"]);
    assert_eq!(manifest.skills, ["./label"]);
    for step in &manifest.runners["review"].graph.steps {
        let StepKind::Agent(agent) = &step.kind else {
            return Err("expected agent steps".to_owned());
        };
        assert_eq!(agent.allowance, expected);
        assert_eq!(
            agent.connectors, manifest.connectors,
            "each turn admits its operations under the skill's declarations"
        );
    }
    Ok(())
}

#[test]
fn a_runner_without_steps_is_one_agent_turn() -> Result<(), String> {
    let manifest = manifest(
        r#"
skill: answer
tools: [fs.read]
runners:
  reply:
    default: true
    inputs:
      question: { type: string, required: true }
    outputs:
      answer: string
"#,
    )?;

    let runner = &manifest.runners["reply"];
    let [step] = runner.graph.steps.as_slice() else {
        return Err("expected one step".to_owned());
    };
    assert_eq!(step.id, "reply");
    let StepKind::Agent(agent) = &step.kind else {
        return Err("expected an agent step".to_owned());
    };
    assert_eq!(
        (agent.role.as_str(), agent.task.as_str()),
        ("answer", "reply")
    );
    assert!(
        step.outputs
            .as_ref()
            .is_some_and(|outputs| outputs.contains_key("answer"))
    );
    assert_eq!(runner.graph.result_from, ["reply"]);
    assert_eq!(allowance(&manifest, "reply")?.tools, ["fs.read"]);
    Ok(())
}

#[test]
fn a_composed_runner_returns_its_steps_not_runner_outputs() -> Result<(), String> {
    let error = refusal(
        r#"
skill: demo
runners:
  default:
    outputs: { answer: string }
    steps:
      - id: decide
        agent: { role: reviewer, task: decide }
"#,
    )?;
    assert!(
        error.contains("belong to a runner without steps"),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_tool_limit_must_be_an_admissible_catalog_ref() -> Result<(), String> {
    let error = refusal(
        r#"
skill: demo
tools: [/tmp/tool/manifest.json]
runners:
  default:
    outputs: { answer: string }
"#,
    )?;
    assert!(error.contains("tools[0]"), "{error}");
    Ok(())
}

#[test]
fn a_limit_is_listed_once() -> Result<(), String> {
    let error = refusal(
        r#"
skill: demo
tools: [fs.read, fs.read]
runners:
  default:
    outputs: { answer: string }
"#,
    )?;
    assert!(error.contains("repeats"), "{error}");
    Ok(())
}

#[test]
fn an_agent_step_declares_no_tools_or_scopes_of_its_own() -> Result<(), String> {
    for field in ["allowed_tools: [fs.read]", "scopes: [fs.read]"] {
        let error = refusal(&format!(
            r#"
skill: demo
runners:
  default:
    steps:
      - id: review
        agent: {{ role: reviewer, task: review }}
        {field}
"#
        ))?;
        assert!(
            error.contains("does not apply to this agent step"),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn a_graph_outside_a_skill_allows_nothing() -> Result<(), String> {
    let graph = validate_graph(
        parse_graph_yaml(
            r#"
name: bare
steps:
  - id: review
    agent: { role: builder, task: review }
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let StepKind::Agent(agent) = &graph.steps[0].kind else {
        return Err("expected an agent step".to_owned());
    };
    assert!(agent.allowance.is_empty());
    Ok(())
}

#[test]
fn graph_rejects_path_like_tool_refs() -> Result<(), String> {
    let error = validate_graph(
        parse_graph_yaml(
            r#"
name: bad-tool-ref
steps:
  - id: read
    tool: ../tools/read/manifest.json
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .err()
    .ok_or_else(|| "expected graph tool ref rejection".to_owned())?;

    assert!(
        error
            .to_string()
            .contains("not an admissible catalog tool ref"),
        "{error}"
    );
    Ok(())
}
