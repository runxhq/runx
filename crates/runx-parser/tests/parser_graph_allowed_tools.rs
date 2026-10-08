use runx_parser::{parse_graph_yaml, parse_runner_manifest_yaml};
use runx_parser::{validate_graph, validate_runner_manifest};

#[test]
fn graph_accepts_catalog_style_allowed_tools() -> Result<(), String> {
    let graph = validate_graph(
        parse_graph_yaml(
            r#"
name: allowed-tools
steps:
  - id: review
    agent: { role: builder, task: review }
    allowed_tools:
      - fs.read
      - git.current_branch
      - cli.capture_help
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    assert_eq!(
        graph.steps[0].allowed_tools,
        Some(vec![
            "fs.read".to_owned(),
            "git.current_branch".to_owned(),
            "cli.capture_help".to_owned(),
        ])
    );
    Ok(())
}

#[test]
fn graph_rejects_path_like_allowed_tools() -> Result<(), String> {
    let error = validate_graph(
        parse_graph_yaml(
            r#"
name: bad-allowed-tools
steps:
  - id: review
    agent: { role: builder, task: review }
    allowed_tools:
      - ../tools/read/manifest.json
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .err()
    .ok_or_else(|| "expected graph allowed_tools rejection".to_owned())?;

    assert!(
        error
            .to_string()
            .contains("not an admissible agent tool ref"),
        "{error}"
    );
    Ok(())
}

#[test]
fn graph_rejects_path_like_tool_refs() -> Result<(), String> {
    let error = validate_graph(
        parse_graph_yaml(
            r#"
name: bad-tool-ref
steps:
  - id: read
    tool: ../tools/read/manifest.json
"#,
        )
        .map_err(|error| error.to_string())?,
    )
    .err()
    .ok_or_else(|| "expected graph tool ref rejection".to_owned())?;

    assert!(
        error
            .to_string()
            .contains("not an admissible catalog tool ref"),
        "{error}"
    );
    Ok(())
}

#[test]
fn runner_rejects_path_like_allowed_tools() -> Result<(), String> {
    let raw = parse_runner_manifest_yaml(
        r#"
skill: demo
runners:
  default:
    steps:
      - id: default
        agent: { role: reviewer, task: review }
        allowed_tools:
          - /tmp/tool/manifest.json
"#,
    )
    .map_err(|error| error.to_string())?;

    let error = validate_runner_manifest(raw)
        .err()
        .ok_or_else(|| "expected runner allowed_tools rejection".to_owned())?;
    assert!(
        error
            .to_string()
            .contains("not an admissible agent tool ref"),
        "{error}"
    );
    Ok(())
}
