use std::fs;
use std::path::Path;

use crate::support::{isolated_runx_command_with_inherited_cwd, temp_root};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A workspace at `dir` with one skill whose step echoes `$run.workspace_id`.
fn write_workspace(dir: &Path) -> TestResult {
    let skill = dir.join("skills/echo");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: echo\ndescription: Echo the run's workspace id.\n---\n# Echo\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  main:\n    steps:\n      - id: echo\n        tool: {module: echo.mjs, export: echo}\n        inputs: {workspace: $run.workspace_id}\n        outputs:\n          workspace: {}\n",
    )?;
    fs::write(
        skill.join("echo.mjs"),
        "export const echo = ({ workspace }) => ({ workspace });\n",
    )?;
    Ok(())
}

/// Run `runx <args> --json` in `cwd` under the runx home `home`.
fn runx(
    cwd: &Path,
    home: &Path,
    args: &[&str],
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let output = isolated_runx_command_with_inherited_cwd("workspace-ids")
        .current_dir(cwd)
        .env("RUNX_HOME", home)
        .args(args)
        .arg("--json")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "runx {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn workspace_id(cwd: &Path, home: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let run = runx(cwd, home, &["skill", "skills/echo"])?;
    run["result"]["workspace"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("the run read no workspace id: {run}").into())
}

#[test]
fn a_moved_workspace_is_rebound_to_the_id_it_had() -> TestResult {
    let root = temp_root("runx-workspace-rebind");
    let home = root.join("home");
    let before = root.join("before");
    write_workspace(&before)?;
    fs::create_dir_all(before.join(".git"))?;

    let had = workspace_id(&before, &home)?;
    let after = root.join("after");
    fs::rename(&before, &after)?;
    // runx can't tell a move from a copy, so the moved workspace is minted anew.
    let fresh = workspace_id(&after, &home)?;
    assert_ne!(fresh, had);

    let rebound = runx(
        &after,
        &home,
        &["workspace", "rebind", &before.display().to_string()],
    )?;
    assert_eq!(rebound["id"], had.as_str(), "{rebound}");
    assert_eq!(rebound["replaced"], fresh.as_str(), "{rebound}");
    assert_eq!(workspace_id(&after, &home)?, had);

    let listed = runx(&after, &home, &["workspace", "list"])?;
    let workspaces = listed["workspaces"]
        .as_array()
        .ok_or_else(|| format!("no workspaces: {listed}"))?;
    assert_eq!(workspaces.len(), 1, "{listed}");
    assert_eq!(workspaces[0]["id"], had.as_str());
    fs::remove_dir_all(root)?;
    Ok(())
}
