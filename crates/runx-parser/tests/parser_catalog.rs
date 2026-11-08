use runx_parser::{CatalogVisibility, parse_method_manifest_yaml, validate_method_manifest};

fn parse_manifest(yaml: &str) -> Result<runx_parser::SkillMethodManifest, String> {
    let raw = parse_method_manifest_yaml(yaml).map_err(|error| error.to_string())?;
    validate_method_manifest(raw).map_err(|error| error.to_string())
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
  role: workflow-stage
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
    assert_eq!(catalog.visibility, CatalogVisibility::Internal);
    // Typed kinds serialize back to their original snake_case wire strings.
    assert_eq!(catalog.visibility.as_str(), "internal");
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
    assert_eq!(catalog.visibility, CatalogVisibility::Public);
    Ok(())
}

#[test]
fn unknown_catalog_kind_fails_closed() -> Result<(), String> {
    let raw = parse_method_manifest_yaml(
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
        validate_method_manifest(raw).is_err(),
        "an unknown catalog.kind must fail closed at validation time"
    );
    Ok(())
}
