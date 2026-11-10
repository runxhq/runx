use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn native_cli_smoke_runs_without_node_or_typescript_env() -> Result<(), Box<dyn std::error::Error>>
{
    let doctor = native_command()?
        .args([
            "doctor",
            "fixtures/doctor/empty-success/workspace",
            "--json",
        ])
        .output()?;
    assert_success(&doctor)?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&doctor.stdout)?["status"],
        "success"
    );

    let list = native_command()?
        .args(["list", "skills", "--json"])
        .output()?;
    assert_success(&list)?;
    let list_json = serde_json::from_slice::<serde_json::Value>(&list.stdout)?;
    assert_eq!(list_json["schema"], "runx.list.v1");

    let temp = crate::support::temp_root("runx-native-no-ts");
    fs::create_dir_all(&temp)?;
    let receipt_dir = temp.join("receipts");

    let history = native_command()?
        .args([
            "history",
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    assert_success(&history)?;
    let history_json = serde_json::from_slice::<serde_json::Value>(&history.stdout)?;
    assert_eq!(history_json["projector_id"], "runx-runtime.run-record.v1");

    let skill_dir = crate::support::write_agent_task_skill(&temp)?;
    let skill = native_command()?
        .args([
            "skill",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    assert_eq!(
        skill.status.code(),
        Some(2),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&skill.stderr),
        String::from_utf8_lossy(&skill.stdout)
    );
    crate::support::assert_json_stderr(&skill.stderr)?;
    let skill_json = serde_json::from_slice::<serde_json::Value>(&skill.stdout)?;
    assert_eq!(skill_json["status"], "needs_agent");
    assert_eq!(skill_json["requests"][0]["kind"], "turn");

    let test_fixture = write_sequential_workflow_smoke_test(&temp)?;
    let test = native_command()?
        .args([
            "test",
            test_fixture.to_str().ok_or("non-utf8 test path")?,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    assert_success(&test)?;
    let receipt = serde_json::from_slice::<serde_json::Value>(&test.stdout)?;
    assert_eq!(receipt["schema"], "runx.receipt.v1");
    // Flat receipts carry no nested test state; a terminal seal is the
    // "sealed" signal, and this workflow closes cleanly.
    assert_eq!(receipt["seal"]["outcome"], "completed");

    Ok(())
}

fn native_command() -> Result<std::process::Command, Box<dyn std::error::Error>> {
    crate::support::isolated_runx_command("native-no-ts-test-key")
}

fn assert_success(output: &std::process::Output) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        output.status.success(),
        "status={:?}\nstderr={}\nstdout={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(String::from_utf8(output.stderr.clone())?, "");
    Ok(())
}

/// A package whose method runs two repository fixture skills in sequence,
/// and the one test that replays it.
fn write_sequential_workflow_smoke_test(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let fixtures = crate::support::repo_root()?.join("fixtures/skills");
    for skill in ["echo", "json-output"] {
        crate::support::copy_dir(&fixtures.join(skill), &root.join("skills").join(skill))?;
    }
    let package = root.join("sequential-echo");
    fs::create_dir_all(package.join("tests"))?;
    fs::write(
        package.join("SKILL.md"),
        "---\nname: sequential-echo\ndescription: Runs json-output, then echo.\n---\n\nUsed only by the native smoke test.\n",
    )?;
    fs::write(
        package.join("run.yaml"),
        r#"minimum_runx: 0.12.0
catalog:
  visibility: internal
methods:
  run:
    steps:
      - id: first
        skill: ../skills/json-output
        inputs:
          message: hello from workflow
      - id: second
        skill: ../skills/echo
        inputs:
          message: $steps.first.result.message
"#,
    )?;
    let test_path = package.join("tests/sequential-workflow-smoke.yaml");
    fs::write(
        &test_path,
        r#"name: sequential-workflow-smoke
expect:
  status: sealed
  steps:
    - first
    - second
"#,
    )?;
    Ok(test_path)
}
