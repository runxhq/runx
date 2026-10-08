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
    assert_eq!(skill_json["requests"][0]["kind"], "agent_act");

    let harness_fixture = write_sequential_graph_smoke_harness(&temp)?;
    let harness = native_command()?
        .args([
            "harness",
            harness_fixture
                .to_str()
                .ok_or("non-utf8 harness fixture path")?,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    assert_success(&harness)?;
    let receipt = serde_json::from_slice::<serde_json::Value>(&harness.stdout)?;
    assert_eq!(receipt["schema"], "runx.receipt.v1");
    // Flat receipts carry no nested harness state; a terminal seal is the
    // "sealed" signal, and this graph closes cleanly.
    assert_eq!(receipt["seal"]["disposition"], "closed");

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

fn write_sequential_graph_smoke_harness(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let graph_path = crate::support::repo_root()?.join("fixtures/graphs/sequential/graph.yaml");
    let harness_path = root.join("sequential-graph-smoke.yaml");
    fs::write(
        &harness_path,
        format!(
            r#"
name: sequential-graph-smoke
kind: graph
target: {}
expect:
  status: sealed
  steps:
    - first
    - second
"#,
            graph_path.display()
        ),
    )?;
    Ok(harness_path)
}
