use runx_parser::{
    ParseError, ValidationError, parse_method_manifest_yaml, parse_skill_markdown,
    parse_tool_manifest_json, parse_tool_manifest_yaml, parse_workflow_yaml, validate_skill,
    validate_tool_manifest, validate_workflow,
};

#[test]
fn parse_rejections_cover_every_error_kind() -> Result<(), String> {
    let invalid_yaml = parse_rejection(parse_workflow_yaml("name: [unterminated\n"))?;
    assert!(
        matches!(invalid_yaml, ParseError::InvalidYaml { .. }),
        "{invalid_yaml:?}"
    );
    let invalid_json = parse_rejection(parse_tool_manifest_json("{"))?;
    assert!(
        matches!(invalid_json, ParseError::InvalidJson { .. }),
        "{invalid_json:?}"
    );
    let invalid_document = parse_rejection(parse_skill_markdown("# missing frontmatter\n"))?;
    assert!(
        matches!(invalid_document, ParseError::InvalidDocument { .. }),
        "{invalid_document:?}"
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
        "schema_digest",
        "source_digest",
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
    let missing_step_id = parse_workflow_yaml(
        r#"
name: bad
steps:
  - skill: ../../skills/echo
"#,
    )
    .map_err(|error| error.to_string())?;
    let missing_field = validation_rejection(validate_workflow(missing_step_id))?;
    assert!(
        matches!(missing_field, ValidationError::MissingField { .. }),
        "{missing_field:?}"
    );

    let unknown_field = parse_workflow_yaml(
        r#"
name: bad
schedule: daily
steps:
  - id: echo
    skill: ../../skills/echo
"#,
    )
    .map_err(|error| error.to_string())?;
    let invalid_field = validation_rejection(validate_workflow(unknown_field))?;
    assert!(
        matches!(invalid_field, ValidationError::InvalidField { .. }),
        "{invalid_field:?}"
    );
    Ok(())
}

#[test]
fn workflow_agent_task_accepts_context_skills() -> Result<(), String> {
    let graph = validate_workflow(
        parse_workflow_yaml(
            r#"
name: context-skills
steps:
  - id: apply_taste
    agent: apply taste
    context:
      skills:
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
fn method_inputs_expand_one_exact_definition_and_validate_nested_examples() -> Result<(), String> {
    // The converter copies a definition taken under another name; one name
    // for both is the owner's call, so the shared input is named for the
    // input that takes it.
    let raw = parse_method_manifest_yaml(
        r#"
minimum_runx: 0.12.0
shared_inputs:
  resources:
    type: object
    description: Bounded issue selector.
    required: [kind, filters]
    additionalProperties: false
    properties:
      kind: {type: string, enum: [issues]}
      filters:
        type: object
        required: [limit]
        additionalProperties: false
        properties:
          limit: {type: integer, minimum: 1, maximum: 25}
methods:
  inspect:
    inputs:
      resources: {}
    examples:
      - resources: {kind: issues, filters: {limit: 25}}
    steps:
      - id: inspect
        agent: inspect
        inputs:
          resources: $input.resources
"#,
    )
    .map_err(|error| error.to_string())?;
    let manifest = runx_parser::validate_method_manifest("typed-inputs", raw)
        .map_err(|error| error.to_string())?;
    let method = manifest
        .methods
        .get("inspect")
        .ok_or_else(|| "expanded method is missing".to_owned())?;

    assert_eq!(
        method.inputs["resources"],
        manifest.shared_inputs["resources"]
    );
    assert_eq!(method.examples.len(), 1);
    Ok(())
}

/// A method `inspect` taking the shared input `resources` as `declaration`,
/// with one example whose limit is `example_limit`.
fn shared_selector(declaration: &str, example_limit: u32) -> String {
    format!(
        r#"
minimum_runx: 0.12.0
shared_inputs:
  resources:
    type: object
    required: [filters]
    properties:
      filters:
        type: object
        required: [limit]
        properties:
          limit: {{type: integer, maximum: 25}}
methods:
  inspect:
    inputs:
      resources: {declaration}
    examples:
      - resources: {{filters: {{limit: {example_limit}}}}}
    steps:
      - id: inspect
        agent: inspect
        inputs:
          resources: $input.resources
"#
    )
}

#[test]
fn method_inputs_override_a_definition_by_key_and_validate_nested_examples() -> Result<(), String> {
    // A key beside a shared input overrides that key of it.
    let raw = parse_method_manifest_yaml(&shared_selector("{optional: true}", 25))
        .map_err(|error| error.to_string())?;
    let manifest = runx_parser::validate_method_manifest("typed-inputs", raw)
        .map_err(|error| error.to_string())?;
    assert!(manifest.shared_inputs["resources"].required);
    assert!(!manifest.methods["inspect"].inputs["resources"].required);

    let raw = parse_method_manifest_yaml(&shared_selector("{}", 26))
        .map_err(|error| error.to_string())?;
    let error = runx_parser::validate_method_manifest("typed-inputs", raw)
        .err()
        .ok_or_else(|| "an invalid nested example unexpectedly validated".to_owned())?;
    assert!(
        error
            .to_string()
            .contains("methods.inspect.examples[0]/resources/filters/limit"),
        "{error}"
    );
    Ok(())
}

#[test]
fn workflow_step_rejects_malformed_artifact_contract() -> Result<(), String> {
    // An output with a packet is an artifact, so a packet names an output the
    // step declares, and the result it becomes is one of its outputs.
    for (artifacts, expected) in [
        (
            "    artifacts:\n      packets:\n        result: runx.test.result.v1\n",
            "steps.0.artifacts does not apply to this tool step",
        ),
        (
            "    result_as: result\n",
            "steps.0.result_as names result, which the step's outputs don't declare",
        ),
        (
            "    outputs:\n      result: {type: object, packet: not a packet}\n",
            "steps.0.outputs is invalid: output declaration is invalid: packet \"not a packet\" is not a packet id",
        ),
    ] {
        let raw = parse_workflow_yaml(&format!(
            "name: malformed-step-artifacts\nsteps:\n  - id: package\n    tool: {{ module: package.mjs }}\n{artifacts}"
        ))
        .map_err(|error| error.to_string())?;
        let error = validate_workflow(raw).err().ok_or_else(|| {
            "expected malformed workflow-step artifacts to be rejected".to_owned()
        })?;
        assert!(error.to_string().contains(expected), "{error}");
    }
    Ok(())
}

#[test]
fn workflow_rejects_context_skills_on_non_agent_run_steps() -> Result<(), String> {
    let error = validate_workflow(
        parse_workflow_yaml(
            r#"
name: bad-context-skills
steps:
  - id: shell
    tool: { command: echo }
    context:
      skills:
        - ../taste-profile
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .err()
    .ok_or_else(|| "expected context skills validation rejection".to_owned())?;

    assert!(
        error
            .to_string()
            .contains("context does not apply to this tool step"),
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
    let error = parse_method_manifest_yaml(
        r#"
minimum_runx: 0.12.0
email:send:
  value: x
methods:
  default:
    steps:
      - id: default
        tool: { command: echo }
"#,
    )
    .err()
    .ok_or_else(|| "expected embedded-colon key rejection".to_owned())?;

    assert!(matches!(error, ParseError::InvalidYaml { .. }), "{error:?}");
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

    assert!(matches!(error, ParseError::InvalidYaml { .. }), "{error:?}");
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

fn parse_rejection<T>(result: Result<T, ParseError>) -> Result<ParseError, String> {
    result
        .err()
        .ok_or_else(|| "expected parse rejection".to_owned())
}

fn validation_rejection<T>(result: Result<T, ValidationError>) -> Result<ValidationError, String> {
    result
        .err()
        .ok_or_else(|| "expected validation rejection".to_owned())
}

#[test]
fn an_empty_list_is_refused_as_listing_nothing() -> Result<(), String> {
    for (step, field) in [
        (
            "tool: data.digest\n        guard: []",
            "steps.0.guard lists nothing",
        ),
        (
            "agent: review\n        calls: {tools: []}",
            "steps.0.calls.tools lists nothing",
        ),
    ] {
        let raw = parse_method_manifest_yaml(&format!(
            "minimum_runx: 0.12.0\nmethods:\n  main:\n    steps:\n      - id: first\n        {step}\n"
        ))
        .map_err(|error| error.to_string())?;
        let error = runx_parser::validate_method_manifest("lists", raw)
            .err()
            .ok_or_else(|| format!("an empty list validated: {step}"))?;
        assert!(error.to_string().contains(field), "{step}: {error}");
    }
    Ok(())
}
