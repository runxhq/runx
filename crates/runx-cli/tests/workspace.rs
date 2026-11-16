use std::fs;
use std::path::Path;

use serde_json::json;

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
    let old = fs::canonicalize(&before)?.display().to_string();
    let after = root.join("after");
    fs::rename(&before, &after)?;
    // runx can't tell a move from a copy, so the moved workspace is minted anew.
    let fresh = workspace_id(&after, &home)?;
    assert_ne!(fresh, had);
    // Its memory reads nothing until it has its id back, and inspection names
    // the path it may have been moved from, which only the person can say.
    let inspect = |cwd: &Path| runx(cwd, &home, &["skill", "inspect", "runx/memory#index"]);
    let moved = inspect(&after)?.to_string();
    assert!(
        moved.contains(&format!(
            "if this workspace was moved from {old}, which no longer exists, ask the person, and if it was, runx workspace rebind {old} gives it back its id"
        )),
        "{moved}"
    );

    let rebound = runx(
        &after,
        &home,
        &["workspace", "rebind", &before.display().to_string()],
    )?;
    assert_eq!(rebound["id"], had.as_str(), "{rebound}");
    assert_eq!(rebound["replaced"], fresh.as_str(), "{rebound}");
    assert_eq!(workspace_id(&after, &home)?, had);
    // A copy, its original still where it was, is named nothing to rebind.
    let copy = root.join("copy");
    write_workspace(&copy)?;
    fs::create_dir_all(copy.join(".git"))?;
    let copied = inspect(&copy)?.to_string();
    assert!(!copied.contains("runx workspace rebind"), "{copied}");

    let listed = runx(&after, &home, &["workspace", "list"])?;
    let workspaces = listed["workspaces"]
        .as_array()
        .ok_or_else(|| format!("no workspaces: {listed}"))?;
    assert_eq!(workspaces.len(), 1, "{listed}");
    assert_eq!(workspaces[0]["id"], had.as_str());
    fs::remove_dir_all(root)?;
    Ok(())
}

/// A workspace whose folder is gone for good is forgotten by the person,
/// with what was kept under its id: its memory, its standing authority and
/// the id itself, so no other workspace is offered it as a move. A folder
/// still there, or a run, can't make the forget.
#[test]
fn a_gone_workspace_is_forgotten_with_what_was_kept_under_it() -> TestResult {
    let root = temp_root("runx-workspace-forget");
    let home = root.join("home");
    let before = root.join("before");
    write_workspace(&before)?;
    fs::create_dir_all(before.join(".git"))?;
    let had = workspace_id(&before, &home)?;
    let old = fs::canonicalize(&before)?.display().to_string();

    // The person's memory there, and their standing authority for it.
    let term = json!({"delegations": [{"connector": "private", "terms": [{
        "verbs": ["read", "write"],
        "bounds": {
            "operations": ["private/*"],
            "scopes": ["store.read", "store.append"],
            "targets": ["runx/memory/said:*", "runx/memory/observed:*"],
            "packages": ["runx/memory"],
        },
    }]}]});
    fs::write(before.join("standing.json"), term.to_string())?;
    let saved = runx(
        &before,
        &home,
        &["authority", "save", "mine", "standing.json", "--standing"],
    )?;
    assert_eq!(saved["standing"], true, "{saved}");
    fs::write(
        before.join("observe.json"),
        json!({"level": "workspace", "text": "The release copy goes out on Fridays.", "summary": "Release copy on Fridays", "date": "2026-11-16", "sources": ["chat:release"]}).to_string(),
    )?;
    let kept = runx(
        &before,
        &home,
        &["skill", "runx/memory#observe", "--inputs", "observe.json"],
    )?;
    assert_eq!(kept["outcome"], "completed", "{kept}");

    let forget = |cwd: &Path, run_token: Option<&str>| -> std::io::Result<std::process::Output> {
        let mut command = isolated_runx_command_with_inherited_cwd("workspace-ids");
        command.current_dir(cwd).env("RUNX_HOME", &home).args([
            "workspace",
            "forget",
            &old,
            "--json",
        ]);
        if let Some(token) = run_token {
            command.env("RUNX_WORKSPACE_LOCK_TOKEN", token);
        }
        command.output()
    };
    let refused = forget(&before, None)?;
    assert!(
        !refused.status.success()
            && String::from_utf8_lossy(&refused.stdout).contains("still exists"),
        "{}",
        String::from_utf8_lossy(&refused.stdout)
    );

    fs::remove_dir_all(&before)?;
    let other = root.join("other");
    write_workspace(&other)?;
    fs::create_dir_all(other.join(".git"))?;
    let inspect = |cwd: &Path| runx(cwd, &home, &["skill", "inspect", "runx/memory#index"]);
    let offered = inspect(&other)?.to_string();
    assert!(
        offered.contains(&format!(
            "if it wasn't, runx workspace forget {old} removes that path and what was kept under it"
        )),
        "{offered}"
    );
    let in_a_run = forget(&other, Some("token"))?;
    assert!(
        !in_a_run.status.success()
            && String::from_utf8_lossy(&in_a_run.stdout).contains("inside a runx run"),
        "{}",
        String::from_utf8_lossy(&in_a_run.stdout)
    );

    let forgotten = forget(&other, None)?;
    let forgotten: serde_json::Value = serde_json::from_slice(&forgotten.stdout)?;
    assert_eq!(forgotten["id"], had.as_str(), "{forgotten}");
    assert_eq!(
        forgotten["streams"],
        json!([format!("runx/memory/observed:workspace:{had}")]),
        "{forgotten}"
    );
    assert_eq!(
        forgotten["authorities"],
        json!([{"name": "mine", "removed": true}]),
        "{forgotten}"
    );
    let store = rusqlite::Connection::open(home.join("private.sqlite"))?;
    let naming: i64 = store.query_row(
        "SELECT count(*) FROM store_events WHERE instr(stream, ?1) > 0",
        [&had],
        |row| row.get(0),
    )?;
    assert_eq!(naming, 0);
    let listed = runx(&other, &home, &["workspace", "list"])?;
    assert!(
        !listed.to_string().contains(&had) && !listed.to_string().contains(&old),
        "{listed}"
    );
    assert!(!inspect(&other)?.to_string().contains("runx workspace"));
    let again = forget(&other, None)?;
    assert!(!again.status.success(), "a forgotten workspace is gone");
    fs::remove_dir_all(root)?;
    Ok(())
}
