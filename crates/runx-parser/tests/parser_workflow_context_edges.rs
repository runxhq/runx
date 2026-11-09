use runx_parser::{parse_method_manifest_yaml, validate_method_manifest};

/// The workflow of the one method `run_yaml` declares.
fn workflow(run_yaml: &str) -> Result<runx_parser::Workflow, String> {
    let raw = parse_method_manifest_yaml(run_yaml).map_err(|error| error.to_string())?;
    let manifest =
        validate_method_manifest("step-reference", raw).map_err(|error| error.to_string())?;
    manifest
        .default_method()
        .map(|method| method.workflow.clone())
        .ok_or_else(|| "the run.yaml declares one method".to_owned())
}

fn workflow_with_reference(path: &str) -> String {
    format!(
        r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: first
        tool: {{ command: node, args: ["-e", "process.stdout.write('{{}}')"] }}
      - id: second
        skill: ../../skills/echo
        inputs:
          message: $steps.first.{path}
"#
    )
}

fn read_path(workflow: &runx_parser::Workflow) -> Option<String> {
    match &workflow.steps[1].bindings.first()?.source {
        runx_parser::Source::Step { step, path } if step == "first" => Some(path.join(".")),
        _ => None,
    }
}

#[test]
fn a_step_reads_a_field_of_an_earlier_step() -> Result<(), String> {
    let workflow = workflow(&workflow_with_reference("result.data.message"))?;

    assert_eq!(read_path(&workflow).as_deref(), Some("result.data.message"));
    assert_eq!(workflow.steps[1].reads(), ["first"]);
    Ok(())
}

#[test]
fn workflow_does_not_reserve_process_diagnostic_names() -> Result<(), String> {
    // Process diagnostics live outside the declared step contract. A producer
    // may therefore intentionally declare any of these ordinary field names.
    for name in ["stdout", "skill_claim", "status", "raw", "stderr"] {
        let workflow = workflow(&workflow_with_reference(name))?;
        assert_eq!(read_path(&workflow).as_deref(), Some(name));
    }
    Ok(())
}

#[test]
fn a_worker_step_preserves_exact_environment_requirements() -> Result<(), String> {
    let workflow = workflow(
        r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: compute
        tool:
          module: compute.mjs
          environment:
            required: [REGION, TENANT_LABEL]
            optional: [TRACE_LABEL]
"#,
    )?;

    let runx_parser::StepKind::Tool(runx_parser::ToolTarget::Worker(worker)) =
        &workflow.steps[0].kind
    else {
        return Err("the worker step was not preserved".to_owned());
    };
    assert_eq!(worker.environment.required, ["REGION", "TENANT_LABEL"]);
    assert_eq!(worker.environment.optional, ["TRACE_LABEL"]);
    Ok(())
}
