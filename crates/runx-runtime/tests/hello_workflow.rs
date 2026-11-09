// The full hello-workflow run (names, statuses, stdout, receipt digests and ids)
// is pinned against the golden fixture in parity/hello_workflow.rs, and its
// continuation from a run record in the method's own tests.

#[test]
fn unknown_step_kind_fails_closed_before_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    let error = match crate::support::run_yaml_workflow(
        "unknown-step-kind",
        r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: custom
        custom-effect: {}
        inputs: {}
"#,
    ) {
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
