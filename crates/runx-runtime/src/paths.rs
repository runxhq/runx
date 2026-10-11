//! Every location runx reads or writes, resolved in one place.
//!
//! Three roots:
//! - the home (`RUNX_HOME`, else `~/.runx`) holds the operator's config,
//!   keys, install state and registry caches;
//! - the workspace (`RUNX_CWD`, else the nearest directory holding
//!   `pnpm-workspace.yaml` or `.runx/project.json`, else the working
//!   directory) holds the project's files;
//! - the project directory (`RUNX_PROJECT_DIR`, else `<workspace>/.runx`)
//!   holds the project's runx state: the kernel store, receipts, authoring
//!   records, harness runs and scratch.
//!
//! A relative `RUNX_CWD` resolves against the working directory; every other
//! relative location resolves against the workspace.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::path_util::lexical_normalize;

pub const RUNX_HOME_ENV: &str = "RUNX_HOME";
pub const RUNX_CWD_ENV: &str = "RUNX_CWD";
pub const RUNX_PROJECT_DIR_ENV: &str = "RUNX_PROJECT_DIR";
pub const RUNX_RECEIPT_DIR_ENV: &str = "RUNX_RECEIPT_DIR";

const RUNX_DIR: &str = ".runx";
const PROJECT_STATE: &str = "project.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunxPaths {
    cwd: PathBuf,
    home: PathBuf,
    workspace: PathBuf,
    project: PathBuf,
    receipts: Option<PathBuf>,
}

impl RunxPaths {
    #[must_use]
    pub fn resolve(env: &BTreeMap<String, String>, cwd: &Path) -> Self {
        let cwd = lexical_normalize(cwd);
        let workspace = env_path(env, RUNX_CWD_ENV).map_or_else(
            || find_workspace(&cwd).unwrap_or_else(|| cwd.clone()),
            |path| lexical_normalize(&cwd.join(path)),
        );
        let within = |path: &Path| lexical_normalize(&workspace.join(path));
        let home = env_path(env, RUNX_HOME_ENV)
            .map_or_else(|| user_home(env, &workspace).join(RUNX_DIR), &within);
        let project =
            env_path(env, RUNX_PROJECT_DIR_ENV).map_or_else(|| workspace.join(RUNX_DIR), &within);
        let receipts = env_path(env, RUNX_RECEIPT_DIR_ENV).map(within);
        Self {
            cwd,
            home,
            workspace,
            project,
            receipts,
        }
    }

    /// The directory runx was invoked from.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    #[must_use]
    pub fn project(&self) -> &Path {
        &self.project
    }

    /// A path the operator named: absolute as given, else within the workspace.
    #[must_use]
    pub fn in_workspace(&self, path: impl AsRef<Path>) -> PathBuf {
        lexical_normalize(&self.workspace.join(path))
    }

    /// A path the operator named that should already exist: the workspace is
    /// tried first, then the invocation directory.
    #[must_use]
    pub fn existing_input(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        [&self.workspace, &self.cwd]
            .into_iter()
            .map(|base| base.join(path))
            .find(|candidate| candidate.exists())
            .unwrap_or_else(|| self.in_workspace(path))
    }

    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.home.join("config.json")
    }

    #[must_use]
    pub fn keys(&self) -> PathBuf {
        self.home.join("keys")
    }

    /// The home's own receipt signing key: the local identity its receipts
    /// are sealed under.
    #[must_use]
    pub fn local_receipt_key(&self) -> PathBuf {
        self.keys().join("local-receipt-ed25519.json")
    }

    #[must_use]
    pub fn install_state(&self) -> PathBuf {
        self.home.join("install.json")
    }

    /// The project's identity, written by `runx init`; its presence in a
    /// workspace's `.runx` marks the workspace.
    #[must_use]
    pub fn project_state(&self) -> PathBuf {
        self.project.join(PROJECT_STATE)
    }

    #[must_use]
    pub fn project_tools(&self) -> PathBuf {
        self.project.join("tools")
    }

    #[must_use]
    pub fn registry_cache(&self) -> PathBuf {
        self.home.join("registry")
    }

    /// The installed copy of a connector tool package, named by its digest.
    #[must_use]
    pub fn connector_tool(&self, digest: &str) -> PathBuf {
        self.home.join("tools").join(digest.replace(':', "-"))
    }

    #[must_use]
    pub fn official_skills(&self) -> PathBuf {
        self.home.join("official-skills")
    }

    #[must_use]
    pub fn registry_skills(&self) -> PathBuf {
        self.home.join("registry-skills")
    }

    /// The receipt root named by `RUNX_RECEIPT_DIR`, when one is.
    #[must_use]
    pub fn receipts_override(&self) -> Option<&Path> {
        self.receipts.as_deref()
    }

    #[must_use]
    pub fn receipts(&self) -> PathBuf {
        self.receipts
            .clone()
            .unwrap_or_else(|| self.project.join("receipts"))
    }

    /// Digest-bound JSON artifacts a run hands back by reference.
    #[must_use]
    pub fn artifacts(&self) -> PathBuf {
        self.project.join("artifacts")
    }

    #[must_use]
    pub fn assistant(&self) -> PathBuf {
        self.project.join("assistant")
    }

    #[must_use]
    pub fn continuations(&self) -> PathBuf {
        self.project.join("continuations")
    }

    /// The SQLite file of the `store` connector when no instance names one.
    #[must_use]
    pub fn workspace_store(&self) -> PathBuf {
        self.project.join("store.sqlite")
    }

    /// Where a harness keeps the receipts of passing cases: the directory
    /// asked for, else `RUNX_RECEIPT_DIR`, else the project's own.
    #[must_use]
    pub fn harness_receipts(&self, requested: Option<&Path>) -> PathBuf {
        match (requested, &self.receipts) {
            (Some(dir), _) => self.in_workspace(dir),
            (None, Some(dir)) => dir.clone(),
            (None, None) => self.project.join("harness-receipts"),
        }
    }

    #[must_use]
    pub fn harness_runs(&self) -> PathBuf {
        self.project.join("harness")
    }

    #[must_use]
    pub fn tmp(&self) -> PathBuf {
        self.project.join("tmp")
    }

    #[must_use]
    pub fn authoring_locks(&self) -> PathBuf {
        self.project.join("authoring").join("locks")
    }

    #[must_use]
    pub fn authoring_applications(&self) -> PathBuf {
        self.project.join("authoring").join("applications")
    }

    #[must_use]
    pub fn registry_step_skills(&self) -> PathBuf {
        self.project.join("registry-step-skills")
    }

    /// A file the operator keeps in the project directory.
    #[must_use]
    pub fn project_file(&self, name: &str) -> PathBuf {
        self.project.join(name)
    }
}

/// The tools a directory keeps for runx, found walking up from a skill.
#[must_use]
pub fn tools_in(dir: &Path) -> PathBuf {
    dir.join(RUNX_DIR).join("tools")
}

/// Point a run's home and project directory into `root`, so nothing it
/// records reaches the operator's workspace or another run.
pub fn isolate(env: &mut BTreeMap<String, String>, root: &Path) {
    for (key, dir) in [(RUNX_HOME_ENV, "home"), (RUNX_PROJECT_DIR_ENV, "project")] {
        env.insert(
            key.to_owned(),
            root.join(dir).to_string_lossy().into_owned(),
        );
    }
}

fn env_path<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a Path> {
    env.get(key)
        .filter(|value| !value.trim().is_empty())
        .map(Path::new)
}

fn find_workspace(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| {
            dir.join("pnpm-workspace.yaml").exists()
                || dir.join(RUNX_DIR).join(PROJECT_STATE).exists()
        })
        .map(Path::to_path_buf)
}

/// The user's home, else the workspace, so every directory of one workspace
/// shares one home.
fn user_home(env: &BTreeMap<String, String>, workspace: &Path) -> PathBuf {
    env.get("HOME")
        .or_else(|| env.get("USERPROFILE"))
        .map_or_else(|| workspace.to_path_buf(), PathBuf::from)
}

#[cfg(test)]
mod tests;
