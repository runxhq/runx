use std::collections::BTreeSet;

use runx_parser::{
    ParseErrorKind, ValidationErrorKind, assert_yaml_scalar_subset, parse_graph_yaml,
    parse_runner_manifest_yaml, parse_skill_markdown, parse_tool_manifest_json,
    parse_tool_manifest_yaml, validate_graph, validate_skill, validate_tool_manifest,
};

#[test]
fn parse_rejections_cover_every_error_kind() -> Result<(), String> {
    let mut kinds = BTreeSet::new();

    kinds.insert(parse_error_kind(parse_graph_yaml("name: [unterminated\n"))?);
    kinds.insert(parse_error_kind(parse_tool_manifest_json("{"))?);
    kinds.insert(parse_error_kind(parse_skill_markdown(
        "# missing frontmatter\n",
    ))?);
    kinds.insert(parse_error_kind(assert_yaml_scalar_subset(
        "fixture", "yes",
    ))?);

    assert_eq!(
        kinds,
        BTreeSet::from([
            ParseErrorKind::InvalidYaml,
            ParseErrorKind::InvalidJson,
            ParseErrorKind::InvalidDocument,
            ParseErrorKind::UnsupportedScalar,
        ]),
    );
    Ok(())
}

#[test]
fn tool_manifests_reject_unknown_input_types_and_invalid_defaults() -> Result<(), String> {
    for (input, expected) in [
        ("type: mystery", "must be one of"),
        (
            "type: integer\n    default: not-an-integer",
            "default must match",
        ),
    ] {
        let raw = parse_tool_manifest_yaml(&format!(
            "schema: runx.tool.manifest.v1\nname: invalid-input\nsource:\n  command: /bin/true\ninputs:\n  value:\n    {input}\n"
        ))
        .map_err(|error| error.to_string())?;
        let error = validate_tool_manifest(raw)
            .err()
            .ok_or_else(|| format!("invalid input contract unexpectedly validated: {input}"))?;
        assert!(error.to_string().contains(expected), "{error}");
    }
    Ok(())
}

#[test]
fn tool_manifests_reject_blank_scopes_without_rewriting_opaque_values() -> Result<(), String> {
    let raw = parse_tool_manifest_yaml(
        "schema: runx.tool.manifest.v1\nname: invalid-scope\nsource:\n  command: /bin/true\nscopes:\n  - '   '\n",
    )
    .map_err(|error| error.to_string())?;
    let error = validate_tool_manifest(raw)
        .err()
        .ok_or_else(|| "blank tool scope unexpectedly validated".to_owned())?;
    assert!(
        error
            .to_string()
            .contains("scopes must contain only non-empty scope strings"),
        "{error}"
    );

    let raw = parse_tool_manifest_yaml(
        "schema: runx.tool.manifest.v1\nname: opaque-scope\nsource:\n  command: /bin/true\nscopes:\n  - 'https://provider.example/auth/custom.scope?mode=read,write'\n  - 'opaque capability with spaces'\n  - 'opaque capability with spaces'\n",
    )
    .map_err(|error| error.to_string())?;
    let tool = validate_tool_manifest(raw).map_err(|error| error.to_string())?;
    assert_eq!(
        tool.scopes,
        [
            "https://provider.example/auth/custom.scope?mode=read,write",
            "opaque capability with spaces",
            "opaque capability with spaces",
        ]
    );
    Ok(())
}

#[test]
fn tool_manifests_reject_generated_parallel_contract_fields() -> Result<(), String> {
    for field in [
        "output",
        "runx",
        "runtime",
        "schema_hash",
        "source_hash",
        "toolkit_version",
    ] {
        let raw = parse_tool_manifest_yaml(&format!(
            "schema: runx.tool.manifest.v1\nname: canonical-only\nsource:\n  command: /bin/true\n{field}: {{}}\n"
        ))
        .map_err(|error| error.to_string())?;
        let error = validate_tool_manifest(raw)
            .err()
            .ok_or_else(|| format!("generated field {field} unexpectedly validated"))?;
        assert!(
            error
                .to_string()
                .contains(&format!("{field} is not supported")),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn tool_manifests_require_the_canonical_schema_marker() -> Result<(), String> {
    let raw = parse_tool_manifest_yaml("name: missing-schema\nsource:\n  command: /bin/true\n")
        .map_err(|error| error.to_string())?;
    let error = validate_tool_manifest(raw)
        .err()
        .ok_or_else(|| "schema-less tool manifest unexpectedly validated".to_owned())?;

    assert!(error.to_string().contains("schema is required"), "{error}");
    Ok(())
}

#[test]
fn validation_rejections_cover_every_error_kind() -> Result<(), String> {
    let mut kinds = BTreeSet::new();

    let missing_step_id = parse_graph_yaml(
        r#"
name: bad
steps:
  - skill: ../../skills/echo
"#,
    )
    .map_err(|error| error.to_string())?;
    kinds.insert(validation_error_kind(validate_graph(missing_step_id))?);

    let unknown_field = parse_graph_yaml(
        r#"
name: bad
schedule: daily
steps:
  - id: echo
    skill: ../../skills/echo
"#,
    )
    .map_err(|error| error.to_string())?;
    kinds.insert(validation_error_kind(validate_graph(unknown_field))?);

    assert_eq!(
        kinds,
        BTreeSet::from([
            ValidationErrorKind::MissingField,
            ValidationErrorKind::InvalidField,
        ]),
    );
    Ok(())
}

#[test]
fn graph_agent_task_accepts_context_skills() -> Result<(), String> {
    let graph = validate_graph(
        parse_graph_yaml(
            r#"
name: context-skills
steps:
  - id: apply_taste
    agent: { role: builder, task: apply taste }
    context_skills:
      - registry:runx/taste-profile@1.0.0
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    assert_eq!(
        graph.steps[0].context_skills,
        vec!["registry:runx/taste-profile@1.0.0"]
    );
    Ok(())
}

#[test]
fn runner_inputs_expand_one_exact_definition_and_validate_nested_examples() -> Result<(), String> {
    let raw = parse_runner_manifest_yaml(
        r#"
skill: typed-inputs
input_definitions:
  selector:
    type: object
    required: true
    description: Bounded issue selector.
    schema:
      required: [kind, filters]
      additionalProperties: false
      properties:
        kind: { type: string, enum: [issues] }
        filters:
          type: object
          required: [limit]
          additionalProperties: false
          properties:
            limit: { type: integer, minimum: 1, maximum: 25 }
runners:
  inspect:
    default: true
    inputs:
      resources: { definition: selector }
    examples:
      - resources: { kind: issues, filters: { limit: 25 } }
    steps:
      - id: inspect
        agent: { role: operator, task: inspect }
        inputs:
          resources: $input.resources
"#,
    )
    .map_err(|error| error.to_string())?;
    let manifest = runx_parser::validate_runner_manifest(raw).map_err(|error| error.to_string())?;
    let runner = manifest
        .runners
        .get("inspect")
        .ok_or_else(|| "expanded runner is missing".to_owned())?;

    assert_eq!(
        runner.inputs["resources"],
        manifest.input_definitions["selector"]
    );
    assert_eq!(runner.examples.len(), 1);
    Ok(())
}

#[test]
fn runner_inputs_reject_definition_overrides_and_invalid_nested_examples() -> Result<(), String> {
    for (declaration, expected) in [
        (
            "{ definition: selector, required: false }",
            "definition is not supported",
        ),
        (
            "{ definition: selector }",
            "runners.inspect.examples[0]/resources/filters/limit",
        ),
    ] {
        let example_limit = if declaration.contains("required") {
            25
        } else {
            26
        };
        let raw = parse_runner_manifest_yaml(&format!(
            r#"
skill: typed-inputs
input_definitions:
  selector:
    type: object
    required: true
    schema:
      required: [filters]
      properties:
        filters:
          type: object
          required: [limit]
          properties:
            limit: {{ type: integer, maximum: 25 }}
runners:
  inspect:
    inputs:
      resources: {declaration}
    examples:
      - resources: {{ filters: {{ limit: {example_limit} }} }}
    steps:
      - id: inspect
        agent: {{ role: operator, task: inspect }}
        inputs:
          resources: $input.resources
"#
        ))
        .map_err(|error| error.to_string())?;
        let error = runx_parser::validate_runner_manifest(raw)
            .err()
            .ok_or_else(|| {
                "invalid reusable input declaration unexpectedly validated".to_owned()
            })?;
        assert!(error.to_string().contains(expected), "{error}");
    }
    Ok(())
}

#[test]
fn graph_step_rejects_malformed_artifact_contract() -> Result<(), String> {
    let raw = parse_graph_yaml(
        r#"
name: malformed-step-artifacts
steps:
  - id: package
    tool: { module: package.mjs }
    artifacts:
      packets:
        result: runx.test.result.v1
"#,
    )
    .map_err(|error| error.to_string())?;
    let error = validate_graph(raw)
        .err()
        .ok_or_else(|| "expected malformed graph-step artifacts to be rejected".to_owned())?;

    assert!(
        error
            .to_string()
            .contains("artifacts.packets requires steps.0.artifacts.named_emits"),
        "{error}"
    );
    Ok(())
}

#[test]
fn graph_rejects_context_skills_on_non_agent_run_steps() -> Result<(), String> {
    let error = validate_graph(
        parse_graph_yaml(
            r#"
name: bad-context-skills
steps:
  - id: shell
    tool: { command: echo }
    context_skills:
      - ../taste-profile
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .err()
    .ok_or_else(|| "expected context_skills validation rejection".to_owned())?;

    assert!(
        error
            .to_string()
            .contains("context_skills does not apply to this tool step"),
        "{error}"
    );
    Ok(())
}

#[test]
fn strict_skill_validation_rejects_runx_frontmatter() -> Result<(), String> {
    let raw = parse_skill_markdown(
        r#"---
name: bad-runx
description: Catalog metadata belongs in run.yaml.
runx: invalid
---
Body
"#,
    )
    .map_err(|error| error.to_string())?;

    match validate_skill(raw) {
        Ok(_) => Err("expected strict frontmatter rejection".to_owned()),
        Err(error) => {
            assert_eq!(
                error.to_string(),
                "SKILL.md.runx is not supported; allowed fields: name, description, license, compatibility, metadata, allowed-tools."
            );
            Ok(())
        }
    }
}

#[test]
fn yaml_parity_rejects_embedded_colon_mapping_key() -> Result<(), String> {
    let error = parse_runner_manifest_yaml(
        r#"
skill: bad
email:send:
  value: x
runners:
  default:
    steps:
      - id: default
        tool: { command: echo }
"#,
    )
    .err()
    .ok_or_else(|| "expected embedded-colon key rejection".to_owned())?;

    assert_eq!(error.kind(), ParseErrorKind::InvalidYaml);
    assert!(
        error.to_string().contains("ambiguous YAML construct"),
        "{error}"
    );
    Ok(())
}

#[test]
fn yaml_parity_rejects_colon_space_in_plain_scalar() -> Result<(), String> {
    let error = parse_tool_manifest_yaml(
        r#"
name: bad-tool
description: needs quote (granted: repo.read)
source:
  command: echo
"#,
    )
    .err()
    .ok_or_else(|| "expected colon-space scalar rejection".to_owned())?;

    assert_eq!(error.kind(), ParseErrorKind::InvalidYaml);
    assert!(
        error.to_string().contains("ambiguous YAML construct"),
        "{error}"
    );
    Ok(())
}

#[test]
fn yaml_parity_allows_quoted_colon_space() -> Result<(), String> {
    let raw = parse_tool_manifest_yaml(
        r#"
name: ok-tool
description: "quoted value (granted: repo.read)"
source:
  command: echo
"#,
    )
    .map_err(|error| error.to_string())?;

    assert!(raw.document.contains_key("name"));
    Ok(())
}

fn parse_error_kind<T>(
    result: Result<T, runx_parser::ParseError>,
) -> Result<ParseErrorKind, String> {
    match result {
        Ok(_) => Err("expected parse rejection".to_owned()),
        Err(error) => Ok(error.kind()),
    }
}

fn validation_error_kind<T>(
    result: Result<T, runx_parser::ValidationError>,
) -> Result<ValidationErrorKind, String> {
    match result {
        Ok(_) => Err("expected validation rejection".to_owned()),
        Err(error) => Ok(error.kind()),
    }
}
