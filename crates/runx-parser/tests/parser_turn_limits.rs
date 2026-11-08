use runx_contracts::{
    AllowedOperation, CatalogRule, CatalogTrust, SkillAllowance, TrustTier, TurnAllowance,
};
use runx_parser::{
    SkillMethodManifest, StepKind, parse_method_manifest_yaml, parse_workflow_yaml,
    validate_method_manifest, validate_workflow,
};

fn manifest(yaml: &str) -> Result<SkillMethodManifest, String> {
    validate_method_manifest(parse_method_manifest_yaml(yaml).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn refusal(yaml: &str) -> Result<String, String> {
    manifest(yaml)
        .err()
        .ok_or_else(|| "expected the manifest to be refused".to_owned())
}

fn allowance(manifest: &SkillMethodManifest, runner: &str) -> Result<TurnAllowance, String> {
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
fn every_agent_turn_receives_its_methods_explicit_operations() -> Result<(), String> {
    let manifest = manifest(
        r#"
skill: triage
connectors:
  github:
    operations: [issue.read, issue.comment]
tools: [fs.read, git.status]
skills: [./label]
asks: true
runners:
  review:
    operations: [github/issue.read, github/issue.comment]
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
        skills: vec![SkillAllowance::Skill("./label".into())],
        asks: true,
    };
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
fn a_method_without_steps_is_one_agent_turn() -> Result<(), String> {
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
    assert_eq!(agent.task.as_str(), "reply");
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
fn a_skills_entry_is_a_ref_or_a_catalog_rule() -> Result<(), String> {
    let manifest = manifest(
        r#"
skill: session
skills:
  - ./label
  - { catalog: { trust: verified } }
  - { project: true }
runners:
  work:
    outputs: { summary: string }
"#,
    )?;
    assert_eq!(
        manifest.skills,
        [
            SkillAllowance::Skill("./label".into()),
            SkillAllowance::Rule(CatalogRule {
                catalog: Some(CatalogTrust {
                    trust: TrustTier::Verified,
                }),
                project: false,
            }),
            SkillAllowance::Rule(CatalogRule {
                catalog: None,
                project: true,
            }),
        ]
    );
    let allowance = allowance(&manifest, "work")?;
    assert_eq!(
        allowance
            .named_skills()
            .map(|r| r.as_str())
            .collect::<Vec<_>>(),
        ["./label"]
    );
    assert_eq!(allowance.rules().count(), 2);

    for (rule, expected) in [
        ("{ project: false }", "exactly one of catalog and project"),
        (
            "{ catalog: { trust: verified }, project: true }",
            "exactly one of catalog and project",
        ),
        ("{ catalog: { trust: anyone } }", "is not a catalog rule"),
        ("{ registry: true }", "is not a catalog rule"),
    ] {
        let error = refusal(&format!(
            "skill: session\nskills: [{rule}]\nrunners:\n  work:\n    outputs: {{ summary: string }}\n"
        ))?;
        assert!(error.contains("skills[0]"), "{rule}: {error}");
        assert!(error.contains(expected), "{rule}: {error}");
    }
    let repeated = refusal(
        "skill: session\nskills: [{ project: true }, { project: true }]\nrunners:\n  work:\n    outputs: { summary: string }\n",
    )?;
    assert!(repeated.contains("skills[1] repeats"), "{repeated}");
    Ok(())
}

#[test]
fn a_method_narrows_the_skills_limits() -> Result<(), String> {
    let manifest = manifest(
        r#"
skill: session
tools: [fs.read, fs.edit]
skills: [{ catalog: { trust: verified } }, { project: true }]
asks: true
runners:
  read:
    tools: [fs.read]
    skills: [{ project: true }]
    asks: false
    outputs: { summary: string }
  develop:
    outputs: { summary: string }
"#,
    )?;
    let read = allowance(&manifest, "read")?;
    assert_eq!(read.tools, ["fs.read"]);
    assert_eq!(read.rules().count(), 1);
    assert!(!read.asks);
    let develop = allowance(&manifest, "develop")?;
    assert_eq!(develop.tools, ["fs.read", "fs.edit"]);
    assert_eq!(develop.rules().count(), 2);
    assert!(develop.asks);
    Ok(())
}

#[test]
fn methods_opt_into_connector_calls_without_changing_workflow_operations() -> Result<(), String> {
    let manifest = manifest(
        r#"
skill: inspect
connectors:
  fixture/forge:
    operations: [issue.read, branch.push]
runners:
  supplied:
    operations: []
    outputs: { summary: string }
  read:
    operations: [fixture/forge/issue.read]
    outputs: { summary: string }
  omitted:
    outputs: { summary: string }
  graph:
    operations: []
    steps:
    - id: read
      operation: fixture/forge/issue.read
      input: {repository: example/project, number: 1}
"#,
    )?;
    assert!(allowance(&manifest, "supplied")?.operations.is_empty());
    assert_eq!(
        allowance(&manifest, "read")?.operations,
        [AllowedOperation {
            requirement: "fixture/forge".into(),
            operation: "issue.read".into(),
        }]
    );
    assert!(allowance(&manifest, "omitted")?.operations.is_empty());
    assert!(matches!(
        manifest.runners["graph"].graph.steps[0].kind,
        StepKind::Operation(_)
    ));
    assert_eq!(manifest.connectors["fixture/forge"].operations.len(), 2);
    Ok(())
}

#[test]
fn method_operations_cannot_widen_or_misstate_the_package_ceiling() -> Result<(), String> {
    for (operations, expected) in [
        ("[issue.read]", "must name requirement/operation"),
        ("[/issue.read]", "must name requirement/operation"),
        ("[github/]", "must name requirement/operation"),
        ("[github/issue.read, github/issue.read]", "repeats"),
        ("null", "is required"),
        ("[42]", "must be a string"),
    ] {
        let error = refusal(&format!(
            "skill: inspect\nconnectors:\n  github:\n    operations: [issue.read]\nrunners:\n  read:\n    operations: {operations}\n    outputs: {{ summary: string }}\n"
        ))?;
        assert!(error.contains("runners.read.operations"), "{error}");
        assert!(error.contains(expected), "{operations}: {error}");
    }
    let error = refusal(
        "skill: inspect\noperations: []\nrunners:\n  read:\n    outputs: { summary: string }\n",
    )?;
    assert!(error.contains("not supported"), "{error}");
    Ok(())
}

#[test]
fn a_composed_method_returns_its_steps_not_method_outputs() -> Result<(), String> {
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
fn a_workflow_outside_a_skill_allows_nothing() -> Result<(), String> {
    let graph = validate_workflow(
        parse_workflow_yaml(
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
fn workflow_rejects_path_like_tool_refs() -> Result<(), String> {
    let error = validate_workflow(
        parse_workflow_yaml(
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
    .ok_or_else(|| "expected graph tool name rejection".to_owned())?;

    assert!(
        error
            .to_string()
            .contains("not an admissible catalog tool name"),
        "{error}"
    );
    Ok(())
}

#[test]
fn step_and_gate_ids_are_identifiers_a_run_can_address() -> Result<(), String> {
    let step = refusal(
        r#"
skill: triage
runners:
  review:
    steps:
      - id: read/0
        agent: { role: reviewer, task: read }
        outputs:
          summary: string
"#,
    )?;
    assert!(step.contains("must match [a-z][a-z0-9_-]*"), "{step}");
    let gate = refusal(
        r#"
skill: triage
runners:
  review:
    steps:
      - id: confirm
        ask: { gate: "send#now", reason: Send it? }
"#,
    )?;
    assert!(gate.contains("must match [a-z0-9][a-z0-9_.-]*"), "{gate}");
    Ok(())
}
