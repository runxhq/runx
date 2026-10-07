use runx_parser::{
    InputMode, SourceKind, ValidateSkillOptions, parse_runner_manifest_yaml, parse_skill_markdown,
    validate_runner_manifest, validate_skill, validate_skill_with_options,
};

fn parse_strict(markdown: &str) -> Result<runx_parser::ValidatedSkill, String> {
    let raw = parse_skill_markdown(markdown).map_err(|error| error.to_string())?;
    validate_skill(raw).map_err(|error| error.to_string())
}

#[test]
fn cli_tool_source_parses_to_typed_kind_and_input_mode() -> Result<(), String> {
    let skill = parse_strict(
        r#"---
name: cli-skill
source:
  type: cli-tool
  command: node
  input_mode: stdin
---
# CLI
"#,
    )?;
    assert_eq!(skill.source.source_type, SourceKind::CliTool);
    assert_eq!(skill.source.input_mode, Some(InputMode::Stdin));
    // The typed kind serializes back to the original wire string.
    assert_eq!(skill.source.source_type.as_str(), "cli-tool");
    Ok(())
}

#[test]
fn javascript_source_parses_a_portable_module_without_process_plumbing() -> Result<(), String> {
    let skill = parse_strict(
        r#"---
name: javascript-skill
source:
  type: javascript
  module: domain/workflow.mjs
  export: execute
---
# JavaScript
"#,
    )?;
    assert_eq!(skill.source.source_type, SourceKind::JavaScript);
    assert_eq!(skill.source.module.as_deref(), Some("domain/workflow.mjs"));
    assert_eq!(skill.source.javascript_export.as_deref(), Some("execute"));
    assert_eq!(skill.source.command, None);
    assert_eq!(skill.source.source_type.as_str(), "javascript");
    Ok(())
}

#[test]
fn javascript_source_declares_exact_required_and_optional_environment() -> Result<(), String> {
    let skill = parse_strict(
        r#"---
name: javascript-environment
source:
  type: javascript
  module: domain/workflow.mjs
  environment:
    required: [NITROSEND_ACCOUNT_ID, REGION]
    optional: [TRACE_LABEL]
---
# JavaScript environment
"#,
    )?;

    assert_eq!(
        skill.source.environment.required,
        ["NITROSEND_ACCOUNT_ID", "REGION"]
    );
    assert_eq!(skill.source.environment.optional, ["TRACE_LABEL"]);
    Ok(())
}

#[test]
fn environment_declaration_rejects_duplicates_invalid_names_and_runtime_owned_names()
-> Result<(), String> {
    for environment in [
        "required: [REGION]\n    optional: [REGION]",
        "required: [not-portable!]",
        "required: [RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64]",
    ] {
        let raw = parse_skill_markdown(&format!(
            "---\nname: bad-environment\nsource:\n  type: javascript\n  module: domain.mjs\n  environment:\n    {environment}\n---\n# Bad environment\n"
        ))
        .map_err(|error| error.to_string())?;
        assert!(
            validate_skill(raw).is_err(),
            "invalid environment unexpectedly passed: {environment}"
        );
    }
    Ok(())
}

#[test]
fn javascript_source_parses_runtime_owned_artifact_pages() -> Result<(), String> {
    let skill = parse_strict(
        r#"---
name: paged-javascript
inputs:
  archive_file:
    type: string
    required: true
  archive_base:
    type: string
    required: true
source:
  type: javascript
  module: domain.mjs
  pages:
    path_from: archive_file
    path_scope_from: archive_base
    media_type: application/javascript
    framing: json_array
    page_bytes: 524288
---
# Paged JavaScript
"#,
    )?;
    let pages = skill
        .source
        .pages
        .ok_or_else(|| "artifact pages were not parsed".to_owned())?;
    assert_eq!(pages.path_from, "archive_file");
    assert_eq!(pages.path_scope_from.as_deref(), Some("archive_base"));
    assert_eq!(pages.page_bytes, 524_288);
    Ok(())
}

#[test]
fn javascript_source_rejects_ambiguous_or_reserved_page_inputs() -> Result<(), String> {
    for pages in [
        "path_from: runx_page\n      media_type: application/json\n      framing: json_array",
        "path_from: archive\n      path_scope_from: archive\n      media_type: application/json\n      framing: json_array",
        "path_from: archive\n      media_type: application/json\n      framing: json_array\n      page_bytes: 4194305",
    ] {
        let raw = parse_skill_markdown(&format!(
            "---\nname: bad-pages\nsource:\n  type: javascript\n  module: domain.mjs\n  pages:\n      {pages}\n---\n# Bad pages\n"
        ))
        .map_err(|error| error.to_string())?;
        assert!(
            validate_skill(raw).is_err(),
            "invalid pages unexpectedly passed"
        );
    }
    Ok(())
}

#[test]
fn javascript_source_rejects_process_fields_and_path_escape() -> Result<(), String> {
    for source in [
        "type: javascript\n  module: ../escape.mjs",
        "type: javascript\n  module: domain.mjs\n  command: node",
        "type: javascript\n  module: domain.mjs\n  args: [extra]",
        "type: javascript\n  module: domain.mjs\n  timeout_seconds: 31",
        "type: javascript\n  module: domain.mjs\n  export: not-valid",
        "type: cli-tool\n  command: node\n  module: domain.mjs",
    ] {
        let raw = parse_skill_markdown(&format!(
            "---\nname: bad-javascript\nsource:\n  {source}\n---\n# JavaScript\n"
        ))
        .map_err(|error| error.to_string())?;
        assert!(
            validate_skill(raw).is_err(),
            "invalid javascript source unexpectedly passed: {source}"
        );
    }
    Ok(())
}

#[test]
fn javascript_source_accepts_a_bounded_wall_limit() -> Result<(), String> {
    let raw = parse_skill_markdown(
        "---\nname: bounded-javascript\nsource:\n  type: javascript\n  module: domain.mjs\n  timeout_seconds: 30\n---\n# Bounded JavaScript\n",
    )
    .map_err(|error| error.to_string())?;
    let skill = validate_skill(raw).map_err(|error| error.to_string())?;

    assert_eq!(skill.source.timeout_seconds, Some(30));
    Ok(())
}

#[test]
fn retired_sandbox_field_is_rejected() -> Result<(), String> {
    let manifest = format!(
        r#"skill: retired-field-javascript
runners:
  transform:
    type: javascript
    module: domain.mjs
    runx:
      {}:
        profile: readonly
        network: false
"#,
        "sandbox"
    );
    let raw = parse_runner_manifest_yaml(&manifest).map_err(|error| error.to_string())?;
    let error = validate_runner_manifest(raw)
        .err()
        .ok_or_else(|| "retired sandbox field unexpectedly passed".to_owned())?;
    assert!(error.to_string().contains("unknown field"));
    Ok(())
}

#[test]
fn misplaced_permission_fields_are_not_silently_ignored_under_runx() -> Result<(), String> {
    let raw = parse_runner_manifest_yaml(
        r#"skill: misplaced-permissions
runners:
  transform:
    type: javascript
    module: domain.mjs
    runx:
      scopes:
        - "opaque capability with spaces"
"#,
    )
    .map_err(|error| error.to_string())?;
    let error = validate_runner_manifest(raw)
        .err()
        .ok_or_else(|| "misplaced runx.scopes unexpectedly passed".to_owned())?;
    assert!(error.to_string().contains("unknown field 'scopes'"));
    Ok(())
}

#[test]
fn default_source_is_agent_kind() -> Result<(), String> {
    // A skill with no explicit source defaults to the `agent` source; the typed
    // `SourceKind` must carry an `Agent` variant for that (the built-in default).
    let raw = parse_skill_markdown(
        r#"---
name: portable-agent
inputs:
  prompt:
    type: string
    required: true
---
# Portable agent
"#,
    )
    .map_err(|error| error.to_string())?;
    let skill = validate_skill_with_options(raw, ValidateSkillOptions::lenient())
        .map_err(|error| error.to_string())?;
    assert_eq!(skill.source.source_type, SourceKind::Agent);
    Ok(())
}

#[test]
fn retired_http_source_points_to_native_http_tools() -> Result<(), String> {
    let raw = parse_skill_markdown(
        r#"---
name: retired-http-source
source:
  type: http
  url: https://api.example.test/v1/pets
---
# HTTP
"#,
    )
    .map_err(|error| error.to_string())?;
    let error = validate_skill(raw)
        .err()
        .ok_or_else(|| "retired http source unexpectedly validated".to_owned())?;
    assert!(error.to_string().contains(
        "source.type http was removed; call a connector operation from a graph operation step"
    ));
    Ok(())
}

#[test]
fn retired_catalog_source_points_to_graph_tool_steps() -> Result<(), String> {
    let raw = parse_skill_markdown(
        r#"---
name: retired-catalog-source
source:
  type: catalog
  catalog_ref: git.status
---
# Catalog
"#,
    )
    .map_err(|error| error.to_string())?;
    let error = validate_skill(raw)
        .err()
        .ok_or_else(|| "retired catalog source unexpectedly validated".to_owned())?;
    assert!(
        error.to_string().contains(
            "source.type catalog was removed; invoke catalog tools from a graph tool step"
        )
    );
    Ok(())
}

#[test]
fn unknown_source_type_fails_closed() -> Result<(), String> {
    let raw = parse_skill_markdown(
        r#"---
name: bogus
source:
  type: not-a-real-source
---
# Bogus
"#,
    )
    .map_err(|error| error.to_string())?;
    assert!(
        validate_skill(raw).is_err(),
        "an unknown source.type must fail closed at parse time"
    );
    Ok(())
}
