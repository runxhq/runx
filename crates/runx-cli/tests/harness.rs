use std::fs;
use std::process::Command;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn package_mode_runs_every_test_in_tests() -> TestResult {
    let root = crate::support::temp_root("runx-package-test-union");
    let skill_dir = root.join("skill");
    let receipt_dir = root.join("receipts");
    write_process_tool_skill(&skill_dir)?;
    fs::write(
        skill_dir.join("tests/conventional.yaml"),
        r#"
name: conventional
method: default
expect:
  status: sealed
  output:
    subset:
      ok: true
"#,
    )?;

    let output = unsigned_runx_command()?
        .args([
            "test",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(report["status"], "passed");
    assert_eq!(report["case_count"], 2);
    let names = report["case_names"]
        .as_array()
        .ok_or("missing case_names")?;
    assert!(names.contains(&serde_json::Value::String("smoke".to_owned())));
    assert!(names.contains(&serde_json::Value::String("conventional".to_owned())));
    let stored_receipts = fs::read_dir(&receipt_dir)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("sha256-") && name.ends_with(".json"))
        })
        .count();
    // Each case seals its graph receipt and its one step receipt.
    assert_eq!(stored_receipts, 4, "every test's receipts persist");
    Ok(())
}

#[test]
fn package_mode_keeps_default_receipts_after_isolated_replay() -> TestResult {
    let root = crate::support::temp_root("runx-package-test-default-receipts");
    let skill_dir = root.join("skill");
    let project = root.join(".runx");
    let receipt_dir = project.join("test-receipts");
    fs::create_dir_all(&skill_dir)?;
    write_process_tool_skill(&skill_dir)?;

    let output = unsigned_runx_command()?
        .env("RUNX_CWD", &root)
        .env("RUNX_PROJECT_DIR", &project)
        .args([
            "test",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    let receipt_id = report["receipt_ids"]
        .as_array()
        .and_then(|ids| ids.first())
        .and_then(serde_json::Value::as_str)
        .ok_or("missing receipt id")?;
    let file_name = format!(
        "sha256-{}.json",
        receipt_id
            .strip_prefix("sha256:")
            .ok_or("invalid receipt id")?
    );

    assert!(receipt_dir.join(file_name).is_file());
    assert!(receipt_dir.join("index.json").is_file());
    assert!(root.join(".runx/harness").read_dir()?.next().is_none());
    Ok(())
}

#[test]
fn package_mode_persists_complete_nested_receipt_lineage() -> TestResult {
    let root = crate::support::temp_root("runx-package-test-nested-receipts");
    let skill_dir = root.join("parent");
    let child_dir = root.join("child");
    let receipt_dir = root.join("receipts");
    fs::create_dir_all(skill_dir.join("tests"))?;
    write_nested_test_parent(&skill_dir)?;
    write_process_tool_skill(&child_dir)?;
    fs::write(
        skill_dir.join("tests/conventional.yaml"),
        r#"
name: nested-conventional
method: default
expect:
  status: sealed
"#,
    )?;

    for _ in 0..2 {
        let output = unsigned_runx_command()?
            .args([
                "test",
                skill_dir.to_str().ok_or("non-utf8 skill dir")?,
                "--receipt-dir",
                receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
                "--json",
            ])
            .output()?;
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    for entry in fs::read_dir(&receipt_dir)? {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("sha256-") && name.ends_with(".json"))
        {
            continue;
        }
        let receipt = serde_json::from_slice::<serde_json::Value>(&fs::read(&path)?)?;
        for child in receipt["lineage"]["children"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let child_id = child["uri"]
                .as_str()
                .and_then(|uri| uri.strip_prefix("runx:receipt:"))
                .ok_or("invalid child receipt reference")?;
            let child_path = receipt_dir.join(format!(
                "sha256-{}.json",
                child_id
                    .strip_prefix("sha256:")
                    .ok_or("invalid child receipt id")?
            ));
            let stored = serde_json::from_slice::<serde_json::Value>(&fs::read(child_path)?)?;
            assert_eq!(child["locator"], stored["digest"]);
        }
    }
    Ok(())
}

#[test]
fn inline_test_rejects_receipt_expectation_drift() -> TestResult {
    assert_inline_expectation_fails(
        "receipt:\n    schema: runx.receipt.v1\n    state: deferred",
        "expect.receipt.state",
    )
}

#[test]
fn inline_test_rejects_step_expectation_drift() -> TestResult {
    assert_inline_expectation_fails("steps: [missing-step]", "expect.steps")
}

#[test]
fn inline_test_rejects_step_output_expectation_drift() -> TestResult {
    assert_inline_expectation_fails(
        "step_outputs:\n    run:\n      subset:\n        status: expected",
        "expect.step_outputs.run.subset.status",
    )
}

/// Every test seals under the fixture identity: the caller's signing
/// variables, even a partial set, never reach a case.
#[test]
fn package_test_seals_under_the_fixture_identity_whatever_the_signer_env() -> TestResult {
    let root = crate::support::temp_root("runx-inline-test-signer");
    let skill_dir = root.join("skill");
    let receipt_dir = root.join("receipts");
    fs::create_dir_all(&skill_dir)?;
    write_process_tool_skill(&skill_dir)?;

    let mut command = unsigned_runx_command()?;
    command.env("RUNX_RECEIPT_SIGN_KID", "partial-explicit-key");
    let output = command
        .args([
            "test",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(report["status"], "passed");

    Ok(())
}

/// A host that refuses host processes, as a registry running a publisher's
/// harness does, never runs the package's process tool, and its case's own
/// environment cannot lift the refusal.
#[test]
fn a_refusing_host_never_runs_a_packages_process_tool() -> TestResult {
    let root = crate::support::temp_root("runx-run-refuses-processes");
    let skill_dir = root.join("skill");
    fs::create_dir_all(&skill_dir)?;
    write_process_tool_skill(&skill_dir)?;

    let output = unsigned_runx_command()?
        .env(
            "RUNX_HOSTED_WORKSPACE_POLICY_JSON",
            r#"{"refuseHostProcesses":true}"#,
        )
        .args([
            "test",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--receipt-dir",
            root.join("receipts")
                .to_str()
                .ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(report["status"], "failed");
    assert!(
        report["assertion_errors"]
            .to_string()
            .contains("starts no trusted host process"),
        "{report}"
    );
    Ok(())
}

#[test]
fn standalone_fixture_persists_to_requested_store() -> TestResult {
    let root = crate::support::temp_root("runx-standalone-test-receipts");
    let skill_dir = root.join("skill");
    let receipt_dir = root.join(".runx/receipts");
    write_process_tool_skill(&skill_dir)?;
    let fixture_path = skill_dir.join("tests/standalone.yaml");
    fs::write(
        &fixture_path,
        r#"
name: standalone
method: default
expect:
  status: sealed
  output:
    subset:
      ok: true
"#,
    )?;

    let output = unsigned_runx_command()?
        .args([
            "test",
            fixture_path.to_str().ok_or("non-utf8 fixture path")?,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    let receipt_id = receipt["id"].as_str().ok_or("missing receipt id")?;
    let receipt_path = receipt_dir.join(format!(
        "sha256-{}.json",
        receipt_id
            .strip_prefix("sha256:")
            .ok_or("invalid receipt id")?
    ));
    assert!(receipt_path.is_file());
    assert!(receipt_dir.join("index.json").is_file());
    Ok(())
}

fn unsigned_runx_command() -> TestResult<Command> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_runx"));
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("NO_COLOR", "1");
    command.current_dir(crate::support::repo_root()?);
    crate::support::isolate_project(&mut command);
    Ok(command)
}

fn write_process_tool_skill(skill_dir: &std::path::Path) -> TestResult {
    fs::create_dir_all(skill_dir.join("tests"))?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-hint\ndescription: test-hint fixture.\n---\n# Harness Hint\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
version: "0.1.0"
minimum_runx: 0.12.0

methods:
  default:
    steps:
      - id: default
        tool: {command: sh, args: [-c, 'printf "{\"ok\":true}"'], timeout_seconds: 5}
        outputs:
          ok: {type: boolean}
"#,
    )?;
    fs::write(
        skill_dir.join("tests/smoke.yaml"),
        "name: smoke\nmethod: default\nexpect:\n  status: sealed\n",
    )?;
    Ok(())
}

fn write_nested_test_parent(skill_dir: &std::path::Path) -> TestResult {
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-parent\ndescription: test-parent fixture.\n---\n# Harness Parent\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
version: "0.1.0"
minimum_runx: 0.12.0

methods:
  default:
    result_from: [nested]
    steps:
      - id: nested
        skill: ../child#default
"#,
    )?;
    Ok(())
}

fn assert_inline_expectation_fails(expectation: &str, expected_field: &str) -> TestResult {
    let root = crate::support::temp_root("runx-inline-test-negative");
    let skill_dir = root.join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-negative\ndescription: test-negative fixture.\n---\n# Harness Negative\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
version: "0.1.0"
minimum_runx: 0.12.0
methods:
  default:
    result_from: [run]
    steps:
      - id: run
        tool: {command: sh, args: [-c, 'printf "{\"ok\":true}"'], timeout_seconds: 5}
        outputs:
          ok: {type: boolean}
"#,
    )?;
    fs::create_dir_all(skill_dir.join("tests"))?;
    fs::write(
        skill_dir.join("tests/rejects-drift.yaml"),
        format!("name: rejects-drift\nexpect:\n  {expectation}\n"),
    )?;

    let output = unsigned_runx_command()?
        .args([
            "test",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(report["status"], "failed");
    assert!(
        report["assertion_errors"]
            .as_array()
            .is_some_and(|errors| errors.iter().any(|error| error
                .as_str()
                .is_some_and(|message| message.contains(expected_field)))),
        "missing expected assertion field {expected_field}: {report}"
    );
    Ok(())
}
