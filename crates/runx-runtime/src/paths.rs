//! Every location runx reads or writes, resolved in one place.
//!
//! Three roots:
//! - the home (`RUNX_HOME`, else `~/.runx`) holds the operator's config,
//!   keys, install state, the bundled skills and the packages read for
//!   inspection. With neither `RUNX_HOME` nor a rooted user home it falls
//!   back to the workspace, and then counts for nothing (D30);
//! - the workspace (`RUNX_CWD`, else the nearest directory holding `.git`,
//!   `skills/` or `pnpm-workspace.yaml`, else the nearest holding
//!   `connectors/`, else the working directory) holds the project's files.
//!   The filesystem root is no one's workspace: a `/skills` a machine holds
//!   marks nothing, and from the root itself the user's home stands in;
//! - the project directory (`RUNX_PROJECT_DIR`, else `<workspace>/.runx`)
//!   holds the project's runx state: the skills and connectors runx added,
//!   the kernel store, receipts, authoring records, test runs and scratch.
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

/// Where a project keeps its detached processes, inside its runx directory.
pub(crate) const PROCESSES_DIR: &str = "processes";

const RUNX_DIR: &str = ".runx";
pub(crate) const ARTIFACTS_DIR: &str = "artifacts";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunxPaths {
    cwd: PathBuf,
    home: PathBuf,
    workspace: PathBuf,
    project: PathBuf,
    receipts: Option<PathBuf>,
    home_fell_back: bool,
    workspace_found: bool,
}

impl RunxPaths {
    #[must_use]
    pub fn resolve(env: &BTreeMap<String, String>, cwd: &Path) -> Self {
        let cwd = lexical_normalize(cwd);
        let workspace = env_path(env, RUNX_CWD_ENV).map_or_else(
            || {
                find_workspace(&cwd).unwrap_or_else(|| {
                    if is_root(&cwd) {
                        user_home(env, &cwd)
                    } else {
                        cwd.clone()
                    }
                })
            },
            |path| lexical_normalize(&cwd.join(path)),
        );
        // runx names the workspace it resolved in RUNX_CWD for what it
        // runs, so a workspace is one its markers make, wherever it was named.
        let workspace_found = find_workspace(&workspace).is_some();
        let within = |path: &Path| lexical_normalize(&workspace.join(path));
        let home_fell_back =
            env_path(env, RUNX_HOME_ENV).is_none() && rooted_user_home(env).is_none();
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
            home_fell_back,
            workspace_found,
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

    /// Whether the home fell back to the workspace: neither `RUNX_HOME` nor
    /// a rooted `HOME` or `USERPROFILE` names one, so it is `<workspace>/.runx`,
    /// which a repository can ship. What the home holds for a person (their
    /// trust, saved authorities, registry keys, cached tier statements) then
    /// counts for nothing (D30).
    #[must_use]
    pub fn home_fell_back(&self) -> bool {
        self.home_fell_back
    }

    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Whether runx runs in a workspace: a directory with `.git`,
    /// `pnpm-workspace.yaml`, `skills/` or `connectors/`, or inside one.
    /// Elsewhere runx uses the directory it was invoked from, which has no
    /// workspace id.
    #[must_use]
    pub fn workspace_found(&self) -> bool {
        self.workspace_found
    }

    /// The workspace ids this runx home minted, keyed by canonical path
    /// (memory.md).
    #[must_use]
    pub fn workspace_ids(&self) -> PathBuf {
        self.home.join("workspaces.json")
    }

    /// The authorities a person saved, one file each by name, in their runx
    /// home (authority.md).
    #[must_use]
    pub fn saved_authorities(&self) -> PathBuf {
        self.home.join("authorities")
    }

    /// The directories holding the skills the user writes, which take
    /// precedence over what runx added (core-shape §6.5): the invocation
    /// directory's and the workspace's `skills/`, then the roots configured
    /// with the platform-separated RUNX_SKILL_ROOTS.
    #[must_use]
    pub fn skill_roots(&self, env: &BTreeMap<String, String>) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        let configured = env
            .get("RUNX_SKILL_ROOTS")
            .into_iter()
            .flat_map(std::env::split_paths)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| self.in_workspace(path));
        for root in [self.cwd.join("skills"), self.skills()]
            .into_iter()
            .chain(configured)
        {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots
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

    /// The execution closures a person trusted, in their home, which holds
    /// none when it fell back ([`Self::home_fell_back`]).
    #[must_use]
    pub fn trust_records(&self) -> PathBuf {
        self.home.join("trust")
    }

    /// Where a registry package read for inspection is kept, its skills,
    /// connectors and install records apart from any project's (N6).
    #[must_use]
    pub fn inspection_cache(&self) -> PathBuf {
        self.home.join("inspected")
    }

    /// The skills the CLI bundles, each version at `<name>/<version>/`: they
    /// bind against the digest the CLI carries, so they materialize here
    /// even in a home that fell back, and never into a project.
    #[must_use]
    pub fn bundled_skills(&self) -> PathBuf {
        self.home.join("bundled")
    }

    /// The registry's tier statements, kept by content digest.
    #[must_use]
    pub fn tier_statements(&self) -> PathBuf {
        self.home.join("tiers")
    }

    /// The operator's registry keys, beside the CLI's built-in ones.
    #[must_use]
    pub fn registry_keys_file(&self) -> PathBuf {
        self.home.join("registry-keys.json")
    }

    /// The installed copy of a connector tool package, named by its digest.
    #[must_use]
    pub fn connector_tool(&self, digest: &str) -> PathBuf {
        self.home.join("tools").join(digest.replace(':', "-"))
    }

    /// The skills the user writes, beside the source (core-shape §6.5): runx
    /// reads them first and never writes there.
    #[must_use]
    pub fn skills(&self) -> PathBuf {
        self.workspace.join("skills")
    }

    /// Every skill a registry added to the project (core-shape §6.5), at
    /// `<publisher>/<name>/<version>/` in the project directory, each version
    /// written once with its `signature.json`. Named apart from `skills/`, which marks a workspace, so
    /// the project directory is never taken for one.
    #[must_use]
    pub fn added_skills(&self) -> PathBuf {
        self.project.join("added-skills")
    }

    /// Every connector runx added to the project (core-shape §6.5): those
    /// from a registry and the runtime capabilities the release carries, at
    /// `<name>/<version>/` in the project directory, each version written
    /// once. Named apart from `connectors/`, which marks a plugin: the user's
    /// own connectors are the project's plugin.
    #[must_use]
    pub fn added_connectors(&self) -> PathBuf {
        self.project.join("added-connectors")
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
        self.project.join(ARTIFACTS_DIR)
    }

    /// The SQLite file of the `store` connector when no instance names one.
    #[must_use]
    pub fn workspace_store(&self) -> PathBuf {
        self.project.join("store.sqlite")
    }

    /// The SQLite file of the `private` connector when no instance names
    /// one: the person's own data, in their runx home, never a workspace.
    #[must_use]
    pub fn private_store(&self) -> PathBuf {
        self.home.join("private.sqlite")
    }

    /// Where a test keeps the receipts of passing cases: the directory
    /// asked for, else `RUNX_RECEIPT_DIR`, else the project's own.
    #[must_use]
    pub fn test_receipts(&self, requested: Option<&Path>) -> PathBuf {
        match (requested, &self.receipts) {
            (Some(dir), _) => self.in_workspace(dir),
            (None, Some(dir)) => dir.clone(),
            (None, None) => self.project.join("test-receipts"),
        }
    }

    #[must_use]
    pub fn test_runs(&self) -> PathBuf {
        self.project.join("test")
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

/// The project `start` lies in: the nearest ancestor that is a repository,
/// a workspace, or holds the user's `skills/` directory, so a skill in
/// `skills/<name>/` belongs to the project above it, never to one of its
/// own. Nothing marks a project but what is already there. Where none of those marks one, the
/// plugin `start` lies in (the nearest ancestor holding `connectors/`) is the
/// project, so the project's connectors are the plugin's.
fn find_workspace(start: &Path) -> Option<PathBuf> {
    let candidates = || start.ancestors().filter(|dir| !is_root(dir));
    candidates()
        .find(|dir| {
            dir.join(".git").exists()
                || dir.join("pnpm-workspace.yaml").exists()
                || dir.join("skills").is_dir()
        })
        .or_else(|| candidates().find(|dir| dir.join("connectors").is_dir()))
        .map(Path::to_path_buf)
}

/// The filesystem root, which is no one's workspace.
fn is_root(dir: &Path) -> bool {
    dir.parent().is_none()
}

/// The user's home, else the workspace, so every directory of one workspace
/// shares one home.
fn user_home(env: &BTreeMap<String, String>, workspace: &Path) -> PathBuf {
    rooted_user_home(env).unwrap_or_else(|| workspace.to_path_buf())
}

/// The user's home, `HOME` else `USERPROFILE`, when it is rooted. An empty or
/// relative one is none: it would put what lives in the home wherever runx
/// happens to run.
pub fn rooted_user_home(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(|key| env.get(key))
        .map(PathBuf::from)
        .find(|home| home.has_root())
}

#[cfg(test)]
mod tests;
