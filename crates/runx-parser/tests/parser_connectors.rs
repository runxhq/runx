//! The execution profile's `connectors:` block and its operation steps.

use runx_parser::{parse_method_manifest_yaml, validate_method_manifest};

fn profile(connectors: &str, step: &str) -> String {
    format!(
        r#"minimum_runx: 0.12.0
methods:
  default:
    steps:
      - id: deliver
{step}
{connectors}
"#
    )
}

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn validate(source: &str) -> Result<runx_parser::SkillMethodManifest, String> {
    let raw = parse_method_manifest_yaml(source).map_err(|error| error.to_string())?;
    validate_method_manifest("notify", raw).map_err(|error| error.to_string())
}

#[test]
fn concrete_requirements_validate_with_their_operation_steps() -> Result<(), String> {
    // A requirement is what the steps name: with no connectors block at all.
    let manifest = validate(&profile(
        "",
        "        operation: slack/channel.post\n        inputs: {}",
    ))?;
    let requirement = manifest
        .connectors
        .get("slack")
        .ok_or("missing requirement")?;
    assert!(requirement.lists("channel.post"));
    assert!(!requirement.lists("channel.post.read"));
    assert!(requirement.choose.is_none());
    Ok(())
}

#[test]
fn a_run_time_choice_needs_a_source_and_a_closed_list() -> Result<(), String> {
    let chosen =
        "connectors:\n  delivery:\n    choose: $input.connector.provider\n    from: [slack]";
    let manifest = validate(&profile(
        chosen,
        "        operation: delivery/message.send\n        inputs: {}",
    ))?;
    let requirement = manifest
        .connectors
        .get("delivery")
        .ok_or("missing requirement")?;
    assert_eq!(requirement.from.len(), 1);
    let open = "connectors:\n  delivery:\n    choose: $input.connector.provider";
    let error = validate(&profile(open, "        operation: delivery/message.send"))
        .err()
        .ok_or("a run-time choice without a list must fail")?;
    assert!(
        error.contains("connectors.delivery.from lists the connectors the run may choose"),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_connectors_entry_names_a_connector_a_step_calls() -> Result<(), String> {
    // The block says only what steps can't, of a connector a step calls: a
    // declaration no step uses is refused, as an operation no requirement
    // listed was.
    let pinned = format!("connectors:\n  slack:\n    manifest_digest: {DIGEST}");
    let manifest = validate(&profile(&pinned, "        operation: slack/channel.post"))?;
    assert!(manifest.connectors["slack"].manifest_digest.is_some());
    let undeclared = validate(&profile(&pinned, "        operation: github/issue.read"))
        .err()
        .ok_or("a declaration no step uses must fail")?;
    assert!(
        undeclared.contains("connectors.slack names a connector no step calls"),
        "{undeclared}"
    );
    let unlisted = validate(&profile(
        "connectors:\n  slack:\n    operations: [channel.post]",
        "        operation: slack/channel.post",
    ))
    .err()
    .ok_or("a listed operation must fail: steps name them")?;
    assert!(unlisted.contains("operations"), "{unlisted}");
    Ok(())
}
