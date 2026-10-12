use runx_parser::{
    CatalogApproval, CatalogAudience, CatalogCompletion, CatalogExecution, CatalogKind,
    CatalogRole, CatalogVisibility, parse_runner_manifest_yaml, validate_runner_manifest,
};

fn parse_manifest(yaml: &str) -> Result<runx_parser::SkillRunnerManifest, String> {
    let raw = parse_runner_manifest_yaml(yaml).map_err(|error| error.to_string())?;
    validate_runner_manifest(raw).map_err(|error| error.to_string())
}

#[test]
fn catalog_metadata_parses_to_typed_enums() -> Result<(), String> {
    let manifest = parse_manifest(
        r#"
skill: demo
catalog:
  kind: graph
  audience: builder
  visibility: internal
  role: graph-stage
  execution: plan
  completion: plan
  approval: required
  part_of:
    - runx/demo
runners:
  default:
    steps:
      - id: default
        tool: { command: node }
"#,
    )?;

    let catalog = manifest
        .catalog
        .ok_or_else(|| "expected catalog metadata".to_owned())?;
    assert_eq!(catalog.kind, CatalogKind::Graph);
    assert_eq!(catalog.audience, CatalogAudience::Builder);
    assert_eq!(catalog.visibility, CatalogVisibility::Internal);
    assert_eq!(catalog.role, CatalogRole::GraphStage);
    assert_eq!(catalog.execution, Some(CatalogExecution::Plan));
    assert_eq!(catalog.completion, Some(CatalogCompletion::Plan));
    assert_eq!(catalog.approval, Some(CatalogApproval::Required));
    assert_eq!(catalog.part_of, vec!["runx/demo"]);
    // Typed kinds serialize back to their original snake_case wire strings.
    assert_eq!(catalog.kind.as_str(), "graph");
    assert_eq!(catalog.audience.as_str(), "builder");
    assert_eq!(catalog.visibility.as_str(), "internal");
    assert_eq!(catalog.role.as_str(), "graph-stage");
    Ok(())
}

#[test]
fn catalog_visibility_defaults_to_public_when_absent() -> Result<(), String> {
    let manifest = parse_manifest(
        r#"
catalog:
  kind: skill
  audience: public
  role: context
skill: demo
runners:
  default:
    steps:
      - id: default
        tool: { command: node }
"#,
    )?;

    let catalog = manifest
        .catalog
        .ok_or_else(|| "expected catalog metadata".to_owned())?;
    assert_eq!(catalog.kind, CatalogKind::Skill);
    assert_eq!(catalog.visibility, CatalogVisibility::Public);
    assert_eq!(catalog.role, CatalogRole::Context);
    Ok(())
}

#[test]
fn catalog_audience_accepts_system_sync_profiles() -> Result<(), String> {
    let manifest = parse_manifest(
        r#"
catalog:
  kind: skill
  audience: system
  visibility: internal
  role: canonical
skill: demo
runners:
  default:
    steps:
      - id: default
        agent: { role: agent, task: default }
"#,
    )?;

    let catalog = manifest
        .catalog
        .ok_or_else(|| "expected catalog metadata".to_owned())?;
    assert_eq!(catalog.audience, CatalogAudience::System);
    assert_eq!(catalog.audience.as_str(), "system");
    Ok(())
}

#[test]
fn unknown_catalog_kind_fails_closed() -> Result<(), String> {
    let raw = parse_runner_manifest_yaml(
        r#"
catalog:
  kind: not-a-kind
  audience: public
skill: demo
runners:
  default:
    steps:
      - id: default
        tool: { command: node }
"#,
    )
    .map_err(|error| error.to_string())?;
    assert!(
        validate_runner_manifest(raw).is_err(),
        "an unknown catalog.kind must fail closed at validation time"
    );
    Ok(())
}
