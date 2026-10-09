use runx_parser::{parse_graph_yaml, validate_graph};

fn graph_with_reference(path: &str) -> String {
    format!(
        r#"
name: step-reference
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

fn read_path(graph: &runx_parser::ExecutionGraph) -> Option<String> {
    match &graph.steps[1].bindings.first()?.source {
        runx_parser::Source::Step { step, path } if step == "first" => Some(path.join(".")),
        _ => None,
    }
}

#[test]
fn a_step_reads_a_field_of_an_earlier_step() -> Result<(), String> {
    let graph = validate_graph(
        parse_graph_yaml(&graph_with_reference("result.data.message"))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    assert_eq!(read_path(&graph).as_deref(), Some("result.data.message"));
    assert_eq!(graph.steps[1].reads(), ["first"]);
    Ok(())
}

#[test]
fn graph_does_not_reserve_process_diagnostic_names() -> Result<(), String> {
    // Process diagnostics live outside the declared step contract. A producer
    // may therefore intentionally declare any of these ordinary field names.
    for name in ["stdout", "skill_claim", "status", "raw", "stderr"] {
        let graph = validate_graph(
            parse_graph_yaml(&graph_with_reference(name)).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(read_path(&graph).as_deref(), Some(name));
    }
    Ok(())
}

#[test]
fn a_worker_step_preserves_exact_environment_requirements() -> Result<(), String> {
    let graph = validate_graph(
        parse_graph_yaml(
            r#"
name: worker-environment
steps:
  - id: compute
    tool:
      module: compute.mjs
      environment:
        required: [REGION, TENANT_LABEL]
        optional: [TRACE_LABEL]
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    let runx_parser::StepKind::Tool(runx_parser::ToolTarget::Worker(worker)) = &graph.steps[0].kind
    else {
        return Err("the worker step was not preserved".to_owned());
    };
    assert_eq!(worker.environment.required, ["REGION", "TENANT_LABEL"]);
    assert_eq!(worker.environment.optional, ["TRACE_LABEL"]);
    Ok(())
}
