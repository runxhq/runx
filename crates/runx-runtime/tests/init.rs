use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use runx_runtime::{
    InitGeneratedValues, RunxInitOptions, RunxPaths, ensure_runx_install_state, runx_init,
};

static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

#[test]
fn init_project_state_is_reused() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::create("init-project")?;
    let project_dir = temp.path().join(".runx");
    let options = init_options(&temp);

    let created = runx_init(&options)?;
    let reused = runx_init(&RunxInitOptions {
        generated: generated("proj_other", "inst_other", "2026-05-19T01:02:03.004Z"),
        ..options
    })?;

    assert!(created.created);
    assert!(!reused.created);
    assert_eq!(created.project_id, reused.project_id);
    assert!(project_dir.join("project.json").exists());
    assert!(project_dir.join("tools").is_dir());
    Ok(())
}

/// The installation's state is recorded on first use, once.
#[test]
fn the_installation_is_recorded_once() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::create("install-state")?;
    let paths = paths_in(temp.path());
    let created = ensure_runx_install_state(&paths, "inst_fixture", "2026-05-19T01:02:03.004Z")?;
    let reused = ensure_runx_install_state(&paths, "inst_other", "2026-05-19T01:02:03.004Z")?;

    assert!(created.created);
    assert!(!reused.created);
    assert_eq!(reused.state.installation_id, "inst_fixture");
    assert!(temp.path().join("home").join("install.json").exists());
    Ok(())
}

fn init_options(temp: &TestDir) -> RunxInitOptions {
    RunxInitOptions {
        paths: paths_in(temp.path()),
        generated: generated("proj_fixture", "inst_fixture", "2026-05-19T01:02:03.004Z"),
    }
}

fn paths_in(root: &Path) -> RunxPaths {
    let env = [
        ("RUNX_CWD", root.to_path_buf()),
        ("RUNX_HOME", root.join("home")),
    ]
    .into_iter()
    .map(|(key, path)| (key.to_owned(), path.to_string_lossy().into_owned()))
    .collect();
    RunxPaths::resolve(&env, root)
}

fn generated(project_id: &str, installation_id: &str, created_at: &str) -> InitGeneratedValues {
    InitGeneratedValues {
        project_id: project_id.to_owned(),
        installation_id: installation_id.to_owned(),
        created_at: created_at.to_owned(),
    }
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn create(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "runx-runtime-init-{label}-{}-{id}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.path);
    }
}
