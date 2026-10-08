#![cfg(feature = "process-tool")]

use runx_parser::{parse_graph_yaml, validate_graph};

// The full hello-graph run (names, statuses, stdout, receipt digests and ids)
// is pinned against the golden fixture in parity/hello_graph.rs, and its
// continuation from a run record in the runner's own tests.

#[test]
fn unknown_step_kind_fails_closed_before_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    let parsed = parse_graph_yaml(
        r#"
name: unknown-step-kind
steps:
  - id: custom
    custom-effect: {}
    inputs: {}
"#,
    )?;
    let error = match validate_graph(parsed) {
        Err(error) => error,
        Ok(_) => {
            return Err(std::io::Error::other(
                "an unknown step kind passed typed graph validation",
            )
            .into());
        }
    };
    let message = error.to_string();
    assert!(
        message.contains("must declare exactly one of agent, tool, operation, ask, or skill"),
        "unexpected error: {message}"
    );
    Ok(())
}
