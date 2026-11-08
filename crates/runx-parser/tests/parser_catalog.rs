use runx_parser::{CatalogVisibility, parse_method_manifest_yaml, validate_method_manifest};

fn parse_manifest(yaml: &str) -> Result<runx_parser::SkillMethodManifest, String> {
    let raw = parse_method_manifest_yaml(yaml).map_err(|error| error.to_string())?;
    validate_method_manifest("demo", raw).map_err(|error| error.to_string())
}

#[test]
fn catalog_metadata_parses_to_typed_enums() -> Result<(), String> {
    let manifest = parse_manifest(
        r#"
minimum_runx: 0.12.0
catalog:
  visibility: internal
methods:
  default:
    steps:
      - id: default
        tool: {command: node}
"#,
    )?;

    let catalog = manifest
        .catalog
        .ok_or_else(|| "expected catalog metadata".to_owned())?;
    assert_eq!(catalog.visibility, CatalogVisibility::Internal);
    // A typed visibility serializes back to its snake_case wire string.
    assert_eq!(catalog.visibility.as_str(), "internal");
    Ok(())
}

#[test]
fn catalog_visibility_defaults_to_public_when_absent() -> Result<(), String> {
    // The converter drops a catalog that held only removed keys, so the
    // catalog here files a tag and leaves its visibility to the default.
    let manifest = parse_manifest(
        r#"
minimum_runx: 0.12.0
catalog:
  tags: [demo]
methods:
  default:
    steps:
      - id: default
        tool: {command: node}
"#,
    )?;

    let catalog = manifest
        .catalog
        .ok_or_else(|| "expected catalog metadata".to_owned())?;
    assert_eq!(catalog.visibility, CatalogVisibility::Public);
    Ok(())
}

#[test]
fn unknown_catalog_values_fail_closed() -> Result<(), String> {
    for (catalog, expected) in [
        (
            "visibility: secret",
            "catalog.visibility must be public or internal",
        ),
        ("kind: not-a-kind", "catalog.kind is not supported"),
    ] {
        let raw = parse_method_manifest_yaml(&format!(
            "minimum_runx: 0.12.0\ncatalog:\n  {catalog}\nmethods:\n  default:\n    steps:\n      - id: default\n        tool: {{command: node}}\n"
        ))
        .map_err(|error| error.to_string())?;
        let error = validate_method_manifest("demo", raw)
            .err()
            .ok_or_else(|| format!("catalog {catalog} must fail closed at validation time"))?;
        assert!(error.to_string().contains(expected), "{error}");
    }
    Ok(())
}
