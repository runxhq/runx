use runx_contracts::{
    AllowedOperation, CatalogRule, CatalogTrust, SkillAllowance, TrustTier, TurnAllowance,
};
use runx_parser::{
    SkillMethodManifest, Source, StepKind, parse_method_manifest_yaml, validate_method_manifest,
};

fn manifest(skill: &str, yaml: &str) -> Result<SkillMethodManifest, String> {
    validate_method_manifest(
        skill,
        parse_method_manifest_yaml(yaml).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn refusal(skill: &str, yaml: &str) -> Result<String, String> {
    manifest(skill, yaml)
        .err()
        .ok_or_else(|| "expected the manifest to be refused".to_owned())
}

fn allowance(manifest: &SkillMethodManifest, method: &str) -> Result<TurnAllowance, String> {
    let step = manifest
        .methods
        .get(method)
        .and_then(|method| method.workflow.steps.first())
        .ok_or_else(|| format!("method {method} has no step"))?;
    match &step.kind {
        StepKind::Agent(agent) => Ok(agent.allowance.clone()),
        kind => Err(format!("expected an agent step, got {kind:?}")),
    }
}

/// One method, `work`, of one agent turn that may load under `rules`.
fn loading(rules: &str) -> String {
    format!(
        "minimum_runx: 0.12.0\nmethods:\n  work:\n    steps:\n      - id: work\n        agent: work\n        calls:\n          load: [{rules}]\n        inputs: $input\n        outputs: {{summary: {{type: string}}}}\n"
    )
}

#[test]
fn every_agent_turn_receives_its_methods_explicit_operations() -> Result<(), String> {
    let manifest = manifest(
        "triage",
        r#"
minimum_runx: 0.12.0
methods:
  review:
    steps:
      - id: read
        agent: read
        calls:
          tools: [fs.read, git.status]
          operations: [github/issue.read, github/issue.comment]
          skills: [./label]
          ask: true
      - id: reply
        agent: reply
        calls:
          tools: [fs.read, git.status]
          operations: [github/issue.read, github/issue.comment]
          skills: [./label]
          ask: true
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
    for step in &manifest.methods["review"].workflow.steps {
        let StepKind::Agent(agent) = &step.kind else {
            return Err("expected agent steps".to_owned());
        };
        assert_eq!(agent.allowance, expected);
        assert_eq!(
            agent.connectors, manifest.connectors,
            "each turn admits its operations under the skill's requirements"
        );
    }
    Ok(())
}

#[test]
fn a_method_of_one_agent_turn_is_one_agent_step() -> Result<(), String> {
    // What today's parser made of a method with no steps, the converter now
    // writes: one agent step named for the method, given the method's input
    // and answering in its outputs.
    let source = r#"
minimum_runx: 0.12.0
methods:
  reply:
    inputs:
      question: {type: string}
    steps:
      - id: reply
        agent: reply
        calls:
          tools: [fs.read]
        inputs: $input
        outputs:
          answer: {type: string}
"#;
    let manifest = manifest("answer", source)?;

    let method = &manifest.methods["reply"];
    let [step] = method.workflow.steps.as_slice() else {
        return Err("expected one step".to_owned());
    };
    assert_eq!(step.id, "reply");
    let StepKind::Agent(agent) = &step.kind else {
        return Err("expected an agent step".to_owned());
    };
    assert_eq!(agent.task, "reply");
    assert_eq!(step.bindings.len(), 1);
    assert!(step.bindings[0].at.is_empty());
    assert_eq!(step.bindings[0].source, Source::Input { path: Vec::new() });
    assert!(
        step.outputs
            .as_ref()
            .is_some_and(|outputs| outputs.contains_key("answer"))
    );
    assert_eq!(method.workflow.result_from, ["reply"]);
    assert_eq!(allowance(&manifest, "reply")?.tools, ["fs.read"]);

    // The method without steps is refused, by name.
    let stepless = refusal(
        "answer",
        "minimum_runx: 0.12.0\nmethods:\n  reply:\n    inputs:\n      question: {type: string}\n",
    )?;
    assert!(
        stepless.contains("methods.reply has no steps; a method is always its steps"),
        "{stepless}"
    );
    Ok(())
}

#[test]
fn a_skills_entry_is_a_ref_or_a_catalog_rule() -> Result<(), String> {
    let manifest = manifest(
        "session",
        r#"
minimum_runx: 0.12.0
methods:
  work:
    steps:
      - id: work
        agent: work
        calls:
          skills: [./label]
          load: [{catalog: {trust: verified}}, {project: true}]
        inputs: $input
        outputs: {summary: {type: string}}
"#,
    )?;
    let allowance = allowance(&manifest, "work")?;
    assert_eq!(
        allowance.skills,
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
        let error = refusal("session", &loading(rule))?;
        assert!(error.contains("load[0]"), "{rule}: {error}");
        assert!(error.contains(expected), "{rule}: {error}");
    }
    let repeated = refusal("session", &loading("{ project: true }, { project: true }"))?;
    assert!(repeated.contains("load[1] repeats"), "{repeated}");
    Ok(())
}

#[test]
fn each_turn_takes_the_calls_its_step_writes() -> Result<(), String> {
    let manifest = manifest(
        "session",
        r#"
minimum_runx: 0.12.0
methods:
  read:
    steps:
      - id: read
        agent: read
        calls:
          tools: [fs.read]
          load: [{project: true}]
        inputs: $input
        outputs: {summary: {type: string}}
  write:
    steps:
      - id: write
        agent: write
        calls:
          tools: [fs.read, fs.edit]
          load: [{catalog: {trust: verified}}, {project: true}]
          ask: true
        inputs: $input
        outputs: {summary: {type: string}}
"#,
    )?;
    let read = allowance(&manifest, "read")?;
    assert_eq!(read.tools, ["fs.read"]);
    assert_eq!(read.rules().count(), 1);
    assert!(!read.asks);
    let write = allowance(&manifest, "write")?;
    assert_eq!(write.tools, ["fs.read", "fs.edit"]);
    assert_eq!(write.rules().count(), 2);
    assert!(write.asks);
    Ok(())
}

#[test]
fn methods_opt_into_connector_calls_without_changing_workflow_operations() -> Result<(), String> {
    let manifest = manifest(
        "inspect",
        r#"
minimum_runx: 0.12.0
methods:
  supplied:
    steps:
    - id: supplied
      agent: supplied
      inputs: $input
      outputs: {summary: {type: string}}
  read:
    steps:
    - id: read
      agent: read
      calls:
        operations: [fixture/forge/issue.read]
      inputs: $input
      outputs: {summary: {type: string}}
  omitted:
    steps:
    - id: omitted
      agent: omitted
      inputs: $input
      outputs: {summary: {type: string}}
  workflow:
    steps:
    - id: read
      operation: fixture/forge/issue.read
      inputs: {repository: example/project, number: 1}
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
        manifest.methods["workflow"].workflow.steps[0].kind,
        StepKind::Operation(_)
    ));
    // A requirement lists exactly the operations the steps name, once each.
    assert_eq!(
        manifest.connectors["fixture/forge"]
            .operations
            .iter()
            .map(|operation| operation.as_str())
            .collect::<Vec<_>>(),
        ["issue.read"]
    );
    Ok(())
}

#[test]
fn a_turns_operations_each_name_a_connector_and_an_operation() -> Result<(), String> {
    for (operations, expected) in [
        ("[issue.read]", "names <connector>/<operation>"),
        ("[/issue.read]", "names <connector>/<operation>"),
        ("[github/]", "names <connector>/<operation>"),
        ("[github/issue.read, github/issue.read]", "repeats"),
        ("null", "must be an array"),
        ("[42]", "must be a string"),
    ] {
        let error = refusal(
            "inspect",
            &format!(
                "minimum_runx: 0.12.0\nmethods:\n  read:\n    steps:\n      - id: read\n        agent: read\n        calls:\n          operations: {operations}\n        inputs: $input\n        outputs: {{summary: {{type: string}}}}\n"
            ),
        )?;
        assert!(error.contains("calls.operations"), "{error}");
        assert!(error.contains(expected), "{operations}: {error}");
    }
    let error = refusal(
        "inspect",
        "minimum_runx: 0.12.0\noperations: []\nmethods:\n  read:\n    steps:\n      - id: read\n        agent: read\n",
    )?;
    assert!(error.contains("not supported"), "{error}");
    Ok(())
}

#[test]
fn a_tool_limit_must_be_an_admissible_catalog_ref() -> Result<(), String> {
    let error = refusal(
        "demo",
        r#"
minimum_runx: 0.12.0
methods:
  default:
    steps:
      - id: default
        agent: default
        calls:
          tools: [/tmp/tool/manifest.json]
        inputs: $input
        outputs: {answer: {type: string}}
"#,
    )?;
    assert!(error.contains("tools[0]"), "{error}");
    Ok(())
}

#[test]
fn a_limit_is_listed_once() -> Result<(), String> {
    let error = refusal(
        "demo",
        r#"
minimum_runx: 0.12.0
methods:
  default:
    steps:
      - id: default
        agent: default
        calls:
          tools: [fs.read, fs.read]
        inputs: $input
        outputs: {answer: {type: string}}
"#,
    )?;
    assert!(error.contains("repeats"), "{error}");
    Ok(())
}

#[test]
fn an_agent_step_declares_no_tools_or_scopes_outside_its_calls() -> Result<(), String> {
    for field in ["allowed_tools: [fs.read]", "scopes: [fs.read]"] {
        let error = refusal(
            "demo",
            &format!(
                r#"
minimum_runx: 0.12.0
methods:
  default:
    steps:
      - id: review
        agent: review
        {field}
"#
            ),
        )?;
        assert!(
            error.contains("does not apply to this agent step"),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn an_agent_step_that_writes_no_calls_allows_nothing() -> Result<(), String> {
    let manifest = manifest(
        "bare",
        r#"
minimum_runx: 0.12.0
methods:
  review:
    steps:
      - id: review
        agent: review
"#,
    )?;
    assert!(allowance(&manifest, "review")?.is_empty());
    Ok(())
}

#[test]
fn workflow_rejects_path_like_tool_refs() -> Result<(), String> {
    let error = refusal(
        "bad-tool-ref",
        r#"
minimum_runx: 0.12.0
methods:
  read:
    steps:
      - id: read
        tool: ../tools/read/manifest.json
"#,
    )?;

    assert!(
        error.contains("not an admissible catalog tool name"),
        "{error}"
    );
    Ok(())
}

#[test]
fn step_and_gate_ids_are_identifiers_a_run_can_address() -> Result<(), String> {
    let step = refusal(
        "triage",
        r#"
minimum_runx: 0.12.0
methods:
  review:
    steps:
      - id: read/0
        agent: read
        outputs:
          summary: {type: string}
"#,
    )?;
    assert!(step.contains("must match [a-z][a-z0-9_-]*"), "{step}");
    let gate = refusal(
        "triage",
        r#"
minimum_runx: 0.12.0
methods:
  review:
    steps:
      - id: confirm
        ask: {gate: "send#now", reason: Send it?}
"#,
    )?;
    assert!(gate.contains("must match [a-z0-9][a-z0-9_.-]*"), "{gate}");
    Ok(())
}
