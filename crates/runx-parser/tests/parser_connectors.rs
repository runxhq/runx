//! The execution profile's `connectors:` block and its operation steps.

use runx_parser::{parse_runner_manifest_yaml, validate_runner_manifest};

fn profile(connectors: &str, step: &str) -> String {
    format!(
        r#"skill: notify
runners:
  default:
    default: true
    type: graph
    graph:
      name: notify
      result_from: [deliver]
      steps:
        - id: deliver
{step}
{connectors}
"#
    )
}

const SLACK: &str = "connectors:\n  slack:\n    operations: [channel.post, channel.post.read]";

fn validate(source: &str) -> Result<runx_parser::SkillRunnerManifest, String> {
    let raw = parse_runner_manifest_yaml(source).map_err(|error| error.to_string())?;
    validate_runner_manifest(raw).map_err(|error| error.to_string())
}

#[test]
fn concrete_requirements_validate_with_their_operation_steps() -> Result<(), String> {
    let manifest = validate(&profile(
        SLACK,
        "          operation: slack/channel.post\n          inputs:\n            target: slack://acme/C1",
    ))?;
    let requirement = manifest
        .connectors
        .get("slack")
        .ok_or("missing requirement")?;
    assert!(requirement.lists("channel.post"));
    assert!(requirement.connector_from.is_none());
    Ok(())
}

#[test]
fn a_run_time_choice_needs_a_source_and_a_closed_list() -> Result<(), String> {
    let chosen = "connectors:\n  delivery:\n    operations: [message.send]\n    connector_from: $input.connector.provider\n    allowed_connectors: [slack]";
    let manifest = validate(&profile(
        chosen,
        "          operation: delivery/message.send\n          inputs:\n            target: x",
    ))?;
    let requirement = manifest
        .connectors
        .get("delivery")
        .ok_or("missing requirement")?;
    assert_eq!(requirement.allowed_connectors.len(), 1);
    let open = "connectors:\n  delivery:\n    operations: [message.send]\n    connector_from: $input.connector.provider";
    let error = validate(&profile(open, "          operation: delivery/message.send"))
        .err()
        .ok_or("a run-time choice without a list must fail")?;
    assert!(error.contains("allowed_connectors"), "{error}");
    Ok(())
}

#[test]
fn operation_steps_must_name_a_declared_requirement_and_listed_operation() -> Result<(), String> {
    let undeclared = validate(&profile(SLACK, "          operation: github/issue.read"))
        .err()
        .ok_or("undeclared requirement must fail")?;
    assert!(undeclared.contains("does not declare"), "{undeclared}");
    let unlisted = validate(&profile(SLACK, "          operation: slack/thread.reply"))
        .err()
        .ok_or("unlisted operation must fail")?;
    assert!(unlisted.contains("is not listed"), "{unlisted}");
    Ok(())
}
