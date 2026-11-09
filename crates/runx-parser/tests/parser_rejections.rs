use runx_parser::{
    ParseError, ValidationError, parse_method_manifest_yaml, parse_skill_markdown,
    parse_tool_manifest_json, parse_tool_manifest_yaml, validate_method_manifest, validate_skill,
    validate_tool_manifest,
};

#[test]
fn parse_rejections_cover_every_error_kind() -> Result<(), String> {
    let invalid_yaml = parse_rejection(parse_method_manifest_yaml(
        "minimum_runx: 0.12.0\nmethods: [unterminated\n",
    ))?;
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
            "schema: runx.tool.manifest.v1\nname: invalid-input\nprocess:\n  command: /bin/true\ninputs:\n  value:\n    {input}\n"
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
fn tool_manifests_refuse_the_keys_0_12_removed() -> Result<(), String> {
    for field in ["source", "scopes", "idempotency", "artifacts"] {
        let raw = parse_tool_manifest_yaml(&format!(
            "schema: runx.tool.manifest.v1\nname: removed\nprocess:\n  command: /bin/true\n{field}: {{}}\n"
        ))
        .map_err(|error| error.to_string())?;
        let error = validate_tool_manifest(raw)
            .err()
            .ok_or_else(|| format!("removed key {field} unexpectedly validated"))?;
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
            "schema: runx.tool.manifest.v1\nname: canonical-only\nprocess:\n  command: /bin/true\n{field}: {{}}\n"
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
    let raw = parse_tool_manifest_yaml("name: missing-schema\nprocess:\n  command: /bin/true\n")
        .map_err(|error| error.to_string())?;
    let error = validate_tool_manifest(raw)
        .err()
        .ok_or_else(|| "schema-less tool manifest unexpectedly validated".to_owned())?;

    assert!(error.to_string().contains("schema is required"), "{error}");
    Ok(())
}

#[test]
fn validation_rejections_cover_every_error_kind() -> Result<(), String> {
    let no_minimum_runx = parse_method_manifest_yaml(
        r#"
methods:
  echo:
    steps:
      - id: echo
        skill: ../../skills/echo
"#,
    )
    .map_err(|error| error.to_string())?;
    let missing_field = validation_rejection(validate_method_manifest("bad", no_minimum_runx))?;
    assert!(
        matches!(missing_field, ValidationError::MissingField { .. }),
        "{missing_field:?}"
    );

    let unknown_field = parse_method_manifest_yaml(
        r#"
minimum_runx: 0.12.0
methods:
  echo:
    schedule: daily
    steps:
      - id: echo
        skill: ../../skills/echo
"#,
    )
    .map_err(|error| error.to_string())?;
    let invalid_field = validation_rejection(validate_method_manifest("bad", unknown_field))?;
    assert!(
        matches!(invalid_field, ValidationError::InvalidField { .. }),
        "{invalid_field:?}"
    );
    Ok(())
}

/// The workflow of the one method `run_yaml` declares.
fn workflow(run_yaml: &str) -> Result<runx_parser::Workflow, String> {
    let raw = parse_method_manifest_yaml(run_yaml).map_err(|error| error.to_string())?;
    let manifest = validate_method_manifest("workflow", raw).map_err(|error| error.to_string())?;
    manifest
        .default_method()
        .map(|method| method.workflow.clone())
        .ok_or_else(|| "the run.yaml declares one method".to_owned())
}

#[test]
fn workflow_agent_task_accepts_context_skills() -> Result<(), String> {
    let workflow = workflow(
        r#"
minimum_runx: 0.12.0
methods:
  apply:
    steps:
      - id: apply_taste
        agent: apply taste
        context:
          skills:
            - registry:runx/taste-profile@1.0.0
"#,
    )?;

    assert_eq!(
        workflow.steps[0].context_skills,
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
            "        artifacts:\n          packets:\n            result: runx.test.result.v1\n",
            "steps.0.artifacts does not apply to this tool step",
        ),
        (
            "        result_as: result\n",
            "steps.0.result_as names result, which steps.0.outputs doesn't declare",
        ),
        (
            "        outputs:\n          result: {type: object, packet: not a packet}\n",
            "steps.0.outputs is invalid: output declaration is invalid: packet \"not a packet\" is not a packet id",
        ),
    ] {
        let error = workflow(&format!(
            "minimum_runx: 0.12.0\nmethods:\n  package:\n    steps:\n      - id: package\n        tool: {{ module: package.mjs }}\n{artifacts}"
        ))
        .err()
        .ok_or_else(|| "expected malformed workflow-step artifacts to be rejected".to_owned())?;
        assert!(error.to_string().contains(expected), "{error}");
    }
    Ok(())
}

#[test]
fn workflow_rejects_context_skills_on_non_agent_run_steps() -> Result<(), String> {
    let error = workflow(
        r#"
minimum_runx: 0.12.0
methods:
  shell:
    steps:
      - id: shell
        tool: { command: echo }
        context:
          skills:
            - ../taste-profile
"#,
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

/// A method that takes a repository either by URL or by owner and name.
fn constrained_method(constraints: &str, examples: &str) -> String {
    format!(
        "minimum_runx: 0.12.0\nmethods:\n  main:\n    inputs:\n      url: {{type: string, optional: true}}\n      owner: {{type: string, optional: true}}\n      name: {{type: string, optional: true}}\n{constraints}{examples}    steps:\n      - id: first\n        tool: data.digest\n"
    )
}

const EITHER_URL_OR_OWNER_AND_NAME: &str = "    input_constraints:\n      oneOf:\n        - required: [url]\n        - required: [owner, name]\n      dependentRequired:\n        owner: [name]\n";

#[test]
fn input_constraints_join_the_methods_input_schema_and_its_digest() -> Result<(), String> {
    let validate = |source: String| {
        parse_method_manifest_yaml(&source)
            .map_err(|error| error.to_string())
            .and_then(|raw| {
                runx_parser::validate_method_manifest("constrained", raw)
                    .map_err(|error| error.to_string())
            })
    };
    let constrained = validate(constrained_method(EITHER_URL_OR_OWNER_AND_NAME, ""))?;
    let open = validate(constrained_method("", ""))?;
    let method = &constrained.methods["main"];
    let schema = method.input_schema();
    assert_eq!(
        schema
            .get("oneOf")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(2),
        "{schema:?}"
    );
    assert!(schema.contains_key("dependentRequired"), "{schema:?}");
    assert_eq!(
        schema.get("additionalProperties"),
        Some(&runx_contracts::JsonValue::Bool(false))
    );
    let digest = |manifest: &runx_parser::SkillMethodManifest| {
        manifest.methods["main"]
            .schemas()
            .map(|schemas| schemas.input_schema_digest)
            .map_err(|error| error.to_string())
    };
    assert_ne!(digest(&constrained)?, digest(&open)?);

    let error = validate(constrained_method(
        EITHER_URL_OR_OWNER_AND_NAME,
        "    examples:\n      - {owner: acme}\n",
    ))
    .err()
    .ok_or("an example that breaks the constraints validated")?;
    assert!(
        error.contains("methods.main.examples[0]") && error.contains("input contract"),
        "{error}"
    );
    Ok(())
}

#[test]
fn input_constraints_relate_declared_inputs_and_nothing_else() -> Result<(), String> {
    for (constraints, expected) in [
        (
            "    input_constraints: {required: [url]}\n",
            "input_constraints.required belongs on the inputs themselves",
        ),
        (
            "    input_constraints: {properties: {url: {minLength: 1}}}\n",
            "input_constraints.properties belongs on the inputs themselves",
        ),
        (
            "    input_constraints: {minProperties: 1}\n",
            "input_constraints.minProperties is not a constraint across inputs",
        ),
        (
            "    input_constraints:\n      anyOf:\n        - required: [repo]\n",
            "names input repo, which method main does not declare",
        ),
        (
            "    input_constraints: {dependentRequired: {owner: [branch]}}\n",
            "names input branch, which method main does not declare",
        ),
        (
            "    input_constraints: {anyOf: url}\n",
            "input_constraints is not valid JSON Schema",
        ),
        (
            "    input_constraints: {}\n",
            "input_constraints lists nothing",
        ),
    ] {
        let raw = parse_method_manifest_yaml(&constrained_method(constraints, ""))
            .map_err(|error| error.to_string())?;
        let error = runx_parser::validate_method_manifest("constrained", raw)
            .err()
            .ok_or_else(|| format!("constraints validated: {constraints}"))?;
        assert!(
            error.to_string().contains(expected),
            "{constraints}: {error}"
        );
    }
    Ok(())
}

/// A method whose one step emits a transcript and a debug trace.
fn method_with_outputs(outputs: &str) -> String {
    format!(
        "minimum_runx: 0.12.0\nmethods:\n  main:\n{outputs}    steps:\n      - id: transcribe\n        tool: data.digest\n        outputs:\n          transcript: {{type: string}}\n          debug: {{type: object, optional: true}}\n          audio: {{packet: acme/outputs.audio.v1, optional: true}}\n"
    )
}

#[test]
fn a_methods_outputs_are_its_output_schema() -> Result<(), String> {
    let validate = |source: String| {
        parse_method_manifest_yaml(&source)
            .map_err(|error| error.to_string())
            .and_then(|raw| {
                runx_parser::validate_method_manifest("outputs", raw)
                    .map_err(|error| error.to_string())
            })
    };
    let declared = validate(method_with_outputs(
        "    outputs:\n      transcript: {type: string}\n",
    ))?;
    let schema = declared.methods["main"].output_schema();
    let properties = schema
        .get("properties")
        .and_then(|value| value.as_object())
        .ok_or("no properties")?;
    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        ["transcript"],
        "{schema:?}"
    );
    let open = validate(method_with_outputs(""))?;
    let open_schema = open.methods["main"].output_schema();
    assert!(
        open_schema
            .get("properties")
            .and_then(|value| value.as_object())
            .is_some_and(|properties| properties.contains_key("debug")),
        "{open_schema:?}"
    );

    for (outputs, expected) in [
        (
            "    outputs:\n      summary: {type: string}\n",
            "outputs.summary is emitted by none of the steps in result_from (transcribe)",
        ),
        (
            "    outputs:\n      audio: {packet: acme/outputs.video.v1}\n",
            "outputs.audio is packet acme/outputs.video.v1, and a step in result_from emits it as",
        ),
        ("    outputs: {}\n", "outputs lists nothing"),
        (
            "    outputs:\n      transcript: {type: wrong}\n",
            "outputs is invalid",
        ),
    ] {
        let error = validate(method_with_outputs(outputs))
            .err()
            .ok_or_else(|| format!("outputs validated: {outputs}"))?;
        assert!(error.contains(expected), "{outputs}: {error}");
    }
    Ok(())
}
