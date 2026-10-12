use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn policy_inspect_json_redacts_raw_locators() -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::support::repo_runx_command()
        .args([
            "policy",
            "inspect",
            "fixtures/operational-policy/provider-like.json",
            "--json",
        ])
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains(r#""action": "inspect""#));
    assert!(stdout.contains(r#""status": "success""#));
    assert!(stdout.contains(r#""policy_id": "provider-issue-flow""#));
    assert!(stdout.contains(r#""locator_count": 1"#));
    assert!(!stdout.contains("slack://example"));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    Ok(())
}

#[test]
fn policy_lint_json_reports_semantic_failure() -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::support::repo_runx_command()
        .args([
            "policy",
            "lint",
            "fixtures/operational-policy/invalid-no-available-runner.json",
            "--json",
        ])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains(r#""action": "lint""#));
    assert!(stdout.contains(r#""status": "failure""#));
    assert!(stdout.contains(r#""code": "target_action_without_runner""#));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    Ok(())
}

#[test]
fn policy_inspect_human_output_is_stable() -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::support::repo_runx_command()
        .args([
            "policy",
            "inspect",
            "fixtures/operational-policy/minimal-single-repo.json",
        ])
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("policy inspect  success"));
    assert!(stdout.contains("policy    single-repo-review-flow"));
    assert!(stdout.contains("sources"));
    assert!(stdout.contains("github-issues: github; locators=1; thread=comment"));
    assert!(stdout.contains("targets"));
    assert!(stdout.contains("example/project: runners=local-review; available=1; owners=1"));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    Ok(())
}

#[test]
fn policy_missing_path_exits_usage() -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::support::repo_runx_command()
        .args(["policy", "inspect", "--json"])
        .output()?;

    assert_eq!(output.status.code(), Some(64));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "failure");
    assert_eq!(value["error"]["code"], "invalid_args");
    assert!(value["error"]["message"].as_str().is_some_and(|message| {
        message.contains("runx policy inspect|lint requires exactly one policy path")
    }));
    Ok(())
}

#[test]
fn policy_json_exposes_redacted_readback_surface() -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::support::repo_runx_command()
        .args([
            "policy",
            "inspect",
            "fixtures/operational-policy/provider-like.json",
            "--json",
        ])
        .output()?;

    let actual = String::from_utf8(output.stdout)?;
    assert_json_subset(
        &actual,
        crate::support::repo_root()?.join("fixtures/operational-policy/provider-like.json"),
    )?;
    Ok(())
}

#[test]
fn policy_rejects_invalid_created_at_contract_value() -> Result<(), Box<dyn std::error::Error>> {
    let policy_path = write_invalid_created_at_policy()?;
    let policy_arg = policy_path.to_string_lossy().into_owned();
    let output = crate::support::repo_runx_command()
        .args(["policy", "inspect", policy_arg.as_str(), "--json"])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("datetime must be YYYY-MM-DDTHH:MM:SS[.fraction]Z"),
        "{stderr}"
    );
    Ok(())
}

fn assert_json_subset(
    actual: &str,
    fixture_path: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo = crate::support::repo_root()?;
    let display_path = fixture_path
        .strip_prefix(&repo)
        .unwrap_or(&fixture_path)
        .to_string_lossy()
        .replace('\\', "/");

    assert!(actual.contains(r#""schema_version": "runx.operational_policy.v1""#));
    assert!(actual.contains(r#""sources""#));
    assert!(actual.contains(r#""runners""#));
    assert!(actual.contains(r#""targets""#));
    assert!(actual.contains(&format!(r#""path": "{}""#, display_path)));
    Ok(())
}

fn write_invalid_created_at_policy() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let temp_dir = std::env::temp_dir().join(format!(
        "runx-policy-cli-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir_all(&temp_dir)?;
    let path = temp_dir.join("invalid-created-at.json");
    let raw = fs::read_to_string(
        crate::support::repo_root()?.join("fixtures/operational-policy/provider-like.json"),
    )?;
    fs::write(
        &path,
        raw.replace(
            r#""created_at": "2026-05-19T00:00:00.000Z""#,
            r#""created_at": "not-a-date""#,
        ),
    )?;
    Ok(path)
}
