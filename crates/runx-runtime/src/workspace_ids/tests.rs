use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::{WorkspaceIdError, list, rebind, workspace_id};
use crate::paths::RunxPaths;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NOW: &str = "2026-11-09T00:00:00Z";

/// A rooted runx home, and a workspace at `name` beside it.
fn paths(home: &Path, workspace: &Path) -> RunxPaths {
    RunxPaths::resolve(
        &BTreeMap::from([("RUNX_HOME".to_owned(), home.display().to_string())]),
        workspace,
    )
}

fn workspace(root: &Path, name: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let dir = root.join(name);
    fs::create_dir_all(dir.join(".git"))?;
    Ok(dir)
}

#[test]
fn a_workspace_is_minted_one_id_in_the_home_and_keeps_it() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let first = workspace(temp.path(), "first")?;
    let second = workspace(temp.path(), "second")?;

    let minted = workspace_id(&paths(&home, &first), NOW)?.ok_or("no id for first")?;
    assert!(minted.starts_with("ws_") && minted.len() == 35, "{minted}");
    // Every later run there reads the same id, from a subdirectory as well.
    fs::create_dir_all(first.join("src"))?;
    assert_eq!(
        workspace_id(&paths(&home, &first.join("src")), NOW)?.as_deref(),
        Some(minted.as_str())
    );
    let other = workspace_id(&paths(&home, &second), NOW)?.ok_or("no id for second")?;
    assert_ne!(minted, other);
    // Nothing is written into either workspace.
    assert!(!first.join(".runx").exists() && !second.join(".runx").exists());
    assert_eq!(list(&paths(&home, &first))?.len(), 2);
    Ok(())
}

#[test]
fn a_workspace_reached_through_a_link_is_the_same_workspace() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let real = workspace(temp.path(), "real")?;
    let link = temp.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link)?;
    #[cfg(not(unix))]
    return Ok(());
    assert_eq!(
        workspace_id(&paths(&home, &real), NOW)?,
        workspace_id(&paths(&home, &link), NOW)?
    );
    Ok(())
}

#[test]
fn a_directory_that_is_no_workspace_has_no_id() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let plain = temp.path().join("plain");
    fs::create_dir_all(&plain)?;
    assert_eq!(workspace_id(&paths(&home, &plain), NOW)?, None);
    assert!(!home.join("workspaces.json").exists());
    Ok(())
}

#[test]
fn a_home_that_fell_back_mints_no_id() -> TestResult {
    let temp = tempfile::tempdir()?;
    let ws = workspace(temp.path(), "ws")?;
    // With no RUNX_HOME and no rooted HOME, the home is the workspace's own.
    let fell_back = RunxPaths::resolve(&BTreeMap::new(), &ws);
    assert!(fell_back.home_fell_back());
    assert!(matches!(
        workspace_id(&fell_back, NOW),
        Err(WorkspaceIdError::HomeFellBack)
    ));
    assert!(!ws.join(".runx/workspaces.json").exists());
    Ok(())
}

#[test]
fn a_moved_workspace_is_rebound_to_the_id_it_had() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let before = workspace(temp.path(), "before")?;
    let had = workspace_id(&paths(&home, &before), NOW)?.ok_or("no id before")?;
    let after = temp.path().join("after");
    fs::rename(&before, &after)?;
    // runx can't tell a move from a copy, so the moved workspace is minted anew.
    let fresh = workspace_id(&paths(&home, &after), NOW)?.ok_or("no id after")?;
    assert_ne!(fresh, had);

    let rebound = rebind(&paths(&home, &after), &before)?;
    assert_eq!(rebound.id, had);
    assert_eq!(rebound.replaced.as_deref(), Some(fresh.as_str()));
    assert_eq!(
        workspace_id(&paths(&home, &after), NOW)?.as_deref(),
        Some(had.as_str())
    );
    // The old path keeps no id, so rebinding it again finds nothing.
    assert!(matches!(
        rebind(&paths(&home, &after), &before),
        Err(WorkspaceIdError::NotKnown { .. })
    ));
    assert_eq!(list(&paths(&home, &after))?.len(), 1);
    Ok(())
}
