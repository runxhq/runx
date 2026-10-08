use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use runx_runtime::{InitAction, InitGeneratedValues, RunxInitOptions, RunxPaths, runx_init};

static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

#[test]
fn init_project_state_is_reused() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::create("init-project")?;
    let project_dir = temp.path().join(".runx");
    let options = init_options(InitAction::Project, &temp);

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

#[test]
fn init_global_prefetches_official_cache() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::create("init-global")?;
    let home = temp.path().join("home");
    let official = temp.path().join("official");
    let result = runx_init(&RunxInitOptions {
        action: InitAction::Global,
        paths: paths_in(temp.path()),
        official_cache_dir: official.clone(),
        prefetch_official: true,
        generated: generated("proj_fixture", "inst_fixture", "2026-05-19T01:02:03.004Z"),
    })?;

    assert!(result.created);
    assert_eq!(result.global_home_dir, Some(home.clone()));
    assert_eq!(result.official_cache_dir, Some(official.clone()));
    assert!(home.join("install.json").exists());
    assert!(official.is_dir());
    Ok(())
}

fn init_options(action: InitAction, temp: &TestDir) -> RunxInitOptions {
    RunxInitOptions {
        action,
        paths: paths_in(temp.path()),
        official_cache_dir: temp.path().join("official"),
        prefetch_official: false,
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
