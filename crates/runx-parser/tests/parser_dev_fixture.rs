use runx_parser::{
    DevExpectedStatus, DevFixtureError, DevFixtureGit, DevFixtureLane, DevFixtureTargetKind,
    HarnessCaller, parse_dev_fixture,
};

fn expected_error<T, E>(
    result: Result<T, E>,
    message: &str,
) -> Result<E, Box<dyn std::error::Error>> {
    match result {
        Err(error) => Ok(error),
        Ok(_) => Err(std::io::Error::other(message).into()),
    }
}

#[test]
fn parses_typed_dev_fixture_contract() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = parse_dev_fixture(
        r#"
name: repo-read
lane: repo-integration
target:
  kind: skill
  ref: workspace-read
workspace:
  files:
    README.md: hello
  git:
    initial_branch: trunk
    dirty_files:
      changed.txt: changed
inputs:
  path: README.md
env:
  MODE: fixture
caller:
  approvals:
    release.publish: true
expect:
  status: success
  output:
    matches_packet: runx.file.v1
    subset:
      path: README.md
"#,
    )?;

    assert_eq!(fixture.name, "repo-read");
    assert_eq!(fixture.lane, DevFixtureLane::RepoIntegration);
    assert_eq!(fixture.target.kind, DevFixtureTargetKind::Skill);
    assert_eq!(fixture.target.reference, "workspace-read");
    assert_eq!(fixture.expect.status, DevExpectedStatus::Success);
    assert_eq!(
        fixture
            .expect
            .output
            .as_ref()
            .and_then(|output| output.matches_packet.as_deref()),
        Some("runx.file.v1")
    );
    let workspace = fixture
        .workspace
        .ok_or_else(|| std::io::Error::other("the workspace was not parsed"))?;
    assert!(workspace.files.contains_key("README.md"));
    assert!(matches!(workspace.git, Some(DevFixtureGit::Config(_))));
    assert_eq!(fixture.caller.approvals.get("release.publish"), Some(&true));
    Ok(())
}

#[test]
fn defaults_lane_inputs_environment_caller_and_expectation()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = parse_dev_fixture(
        r#"
name: echo
target:
  kind: tool
  ref: acme.echo
"#,
    )?;

    assert_eq!(fixture.lane, DevFixtureLane::Deterministic);
    assert!(fixture.inputs.is_empty());
    assert!(fixture.env.is_empty());
    assert_eq!(fixture.caller, HarnessCaller::default());
    assert_eq!(fixture.expect.status, DevExpectedStatus::Success);
    assert!(fixture.expect.output.is_none());
    Ok(())
}

#[test]
fn rejects_unsafe_workspace_paths_at_the_parser_boundary() -> Result<(), Box<dyn std::error::Error>>
{
    let error = expected_error(
        parse_dev_fixture(
            r#"
name: escape
target:
  kind: tool
  ref: acme.echo
workspace:
  files:
    ../outside.txt: nope
"#,
        ),
        "workspace path traversal was accepted",
    )?;

    assert!(matches!(
        error,
        DevFixtureError::Invalid { field, .. } if field == "workspace.files.../outside.txt"
    ));
    Ok(())
}

#[test]
fn rejects_unknown_contract_fields_instead_of_silently_ignoring_them()
-> Result<(), Box<dyn std::error::Error>> {
    let error = expected_error(
        parse_dev_fixture(
            r#"
name: drift
target:
  kind: graph
  ref: graph.yaml
retry_forever: true
"#,
        ),
        "unknown fixture fields were accepted",
    )?;

    assert!(matches!(error, DevFixtureError::Parse(_)));
    Ok(())
}

#[test]
fn rejects_unknown_lane_selectors() -> Result<(), Box<dyn std::error::Error>> {
    let error = expected_error(
        "fast".parse::<DevFixtureLane>(),
        "unknown fixture lane was accepted",
    )?;

    assert!(matches!(
        error,
        DevFixtureError::Invalid { field, .. } if field == "lane"
    ));
    Ok(())
}
