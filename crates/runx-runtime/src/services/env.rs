use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::paths::{RUNX_CWD_ENV, RUNX_PROJECT_DIR_ENV, RunxPaths};
use crate::receipts::paths::{ReceiptPathInputs, ResolvedReceiptPath, resolve_receipt_path};
use crate::services::tool_roots::inferred_tool_roots;
use thiserror::Error;

const WORKSPACE_ENV_FILE: &str = ".env";
/// Runx's own variables: where state lives, which Connect and registry it
/// trusts, how it signs, and what a host admits. They come only from the
/// process a host or operator starts, never from a workspace's files.
const RUNX_NAME_PREFIX: &str = "RUNX_";
/// The names a workspace's `.env` sets to a value other than the process
/// environment's, comma-separated, never their values. The process value is
/// the one kept; a credential read refuses such a name instead of acting on
/// whichever value happened to win.
pub(crate) const RUNX_ENV_FILE_DIFFERS_ENV: &str = "RUNX_ENV_FILE_DIFFERS";
/// The names a workspace's `.env` filled because the process environment
/// lacked them, comma-separated, never their values. Only the operator's own
/// environment signs in to a secrets manager or configures it, so a
/// manager's CLI never sees one of these (`process_environment`).
pub(crate) const RUNX_ENV_FILE_FILLED_ENV: &str = "RUNX_ENV_FILE_FILLED";

#[derive(Clone, PartialEq, Eq)]
pub struct WorkspaceEnv {
    env: BTreeMap<String, String>,
    cwd: PathBuf,
    paths: RunxPaths,
    env_file_loaded: bool,
}

impl WorkspaceEnv {
    /// Capture one immutable local-operator environment for a Runx workspace.
    ///
    /// The workspace is resolved from the ambient environment before `.env` is
    /// read. The file is parsed as data and only fills missing keys; a file
    /// that sets one of Runx's own `RUNX_` variables is refused. This method
    /// never mutates the process environment.
    pub fn load_process(cwd: PathBuf) -> Result<Self, WorkspaceEnvError> {
        Self::load(std::env::vars().collect(), cwd)
    }

    pub(crate) fn load(
        ambient_env: BTreeMap<String, String>,
        cwd: PathBuf,
    ) -> Result<Self, WorkspaceEnvError> {
        require_absolute_cwd(&cwd)?;
        let invoked = cwd;
        let cwd = RunxPaths::resolve(&ambient_env, &invoked)
            .workspace()
            .to_path_buf();
        require_absolute_cwd(&cwd)?;
        let env_path = cwd.join(WORKSPACE_ENV_FILE);
        let mut env = ambient_env;
        let env_file = merge_env_file(&mut env, &env_path)?;
        // Recomputed for this workspace; never inherited from a parent.
        for (marker, names) in [
            (RUNX_ENV_FILE_DIFFERS_ENV, &env_file.differs),
            (RUNX_ENV_FILE_FILLED_ENV, &env_file.filled),
        ] {
            env.remove(marker);
            if !names.is_empty() {
                env.insert(marker.to_owned(), names.join(","));
            }
        }

        // Freeze workspace identity after discovery. A value inside `.env`
        // cannot redirect Runx to a different workspace after the file was read.
        env.insert(RUNX_CWD_ENV.to_owned(), cwd.to_string_lossy().into_owned());
        let paths = RunxPaths::resolve(&env, &invoked);

        Ok(Self {
            env,
            cwd,
            paths,
            env_file_loaded: env_file.loaded,
        })
    }

    pub(crate) fn new(
        mut env: BTreeMap<String, String>,
        cwd: PathBuf,
    ) -> Result<Self, WorkspaceEnvError> {
        require_absolute_cwd(&cwd)?;
        let invoked = cwd;
        let cwd = RunxPaths::resolve(&env, &invoked).workspace().to_path_buf();
        require_absolute_cwd(&cwd)?;
        env.insert(RUNX_CWD_ENV.to_owned(), cwd.to_string_lossy().into_owned());
        let paths = RunxPaths::resolve(&env, &invoked);
        Ok(Self {
            env,
            cwd,
            paths,
            env_file_loaded: false,
        })
    }

    pub(crate) fn from_admitted(env: BTreeMap<String, String>) -> Result<Self, WorkspaceEnvError> {
        let cwd = env
            .get(RUNX_CWD_ENV)
            .map(PathBuf::from)
            .ok_or(WorkspaceEnvError::MissingCwd)?;
        Self::new(env, cwd)
    }

    #[must_use]
    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    #[must_use]
    pub fn paths(&self) -> &RunxPaths {
        &self.paths
    }

    /// The receipt root of a run here: `explicit_dir`, else `RUNX_RECEIPT_DIR`,
    /// else the project's receipts.
    pub(crate) fn receipt_path(&self, explicit_dir: Option<&Path>) -> ResolvedReceiptPath {
        resolve_receipt_path(ReceiptPathInputs {
            explicit_dir,
            runtime_config: None,
            paths: &self.paths,
        })
    }

    #[must_use]
    pub const fn env_file_loaded(&self) -> bool {
        self.env_file_loaded
    }

    pub(crate) fn skill_env_for_skill(&self, skill_dir: &Path) -> BTreeMap<String, String> {
        let mut env = self.env.clone();
        let cwd = self.cwd.to_string_lossy().into_owned();
        env.insert(RUNX_CWD_ENV.to_owned(), cwd);
        env.insert(
            RUNX_PROJECT_DIR_ENV.to_owned(),
            self.paths.project().to_string_lossy().into_owned(),
        );
        merge_inferred_tool_roots(&mut env, skill_dir);
        env
    }
}

impl fmt::Debug for WorkspaceEnv {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceEnv")
            .field("cwd", &self.cwd)
            .field("env_key_count", &self.env.len())
            .field("env_file_loaded", &self.env_file_loaded)
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceEnvError {
    #[error("admitted workspace environment is missing RUNX_CWD")]
    MissingCwd,
    #[error("workspace cwd must be absolute, got {path}")]
    RelativeCwd { path: PathBuf },
    #[error("could not read workspace environment file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("workspace environment file {path} has invalid syntax at byte offset {offset}")]
    Syntax { path: PathBuf, offset: usize },
    #[error("workspace environment file {path} contains an invalid environment reference")]
    EnvironmentReference { path: PathBuf },
    #[error("workspace environment file {path} could not be parsed")]
    Parse { path: PathBuf },
    #[error(
        "workspace environment file {path} sets {name}; Runx's own variables, and the HOME and USERPROFILE that locate its home, come only from the process environment or `runx config`"
    )]
    ReservedName { path: PathBuf, name: String },
}

fn require_absolute_cwd(cwd: &Path) -> Result<(), WorkspaceEnvError> {
    if cwd.is_absolute() {
        Ok(())
    } else {
        Err(WorkspaceEnvError::RelativeCwd {
            path: cwd.to_path_buf(),
        })
    }
}

/// Whether the workspace's `.env` sets `name` to a value other than the
/// process environment's.
pub(crate) fn env_file_differs(env: &BTreeMap<String, String>, name: &str) -> bool {
    env.get(RUNX_ENV_FILE_DIFFERS_ENV)
        .is_some_and(|names| names.split(',').any(|differs| differs == name))
}

/// The process environment a workspace environment was loaded from: `env`
/// without the names the workspace's `.env` filled.
pub(crate) fn process_environment(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let filled = |name: &str| {
        env.get(RUNX_ENV_FILE_FILLED_ENV)
            .is_some_and(|names| names.split(',').any(|filled| filled == name))
    };
    env.iter()
        .filter(|(name, _)| !filled(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// What merging a workspace's `.env` did.
#[derive(Default)]
struct EnvFile {
    loaded: bool,
    /// The names it filled, which the environment lacked.
    filled: Vec<String>,
    /// The names it sets to a value other than the environment's.
    differs: Vec<String>,
}

/// Fill the names `path` sets that `env` lacks.
fn merge_env_file(
    env: &mut BTreeMap<String, String>,
    path: &Path,
) -> Result<EnvFile, WorkspaceEnvError> {
    let entries = match dotenvy::from_path_iter(path) {
        Ok(entries) => entries,
        Err(dotenvy::Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(EnvFile::default());
        }
        Err(error) => return Err(workspace_env_error(path, error)),
    };

    let mut filled = std::collections::BTreeSet::new();
    let mut differs = Vec::new();
    for entry in entries {
        let (key, value) = entry.map_err(|error| workspace_env_error(path, error))?;
        // HOME and USERPROFILE locate the runx home when RUNX_HOME is unset,
        // so a workspace may no more set them than runx's own names.
        if key.starts_with(RUNX_NAME_PREFIX) || matches!(key.as_str(), "HOME" | "USERPROFILE") {
            return Err(WorkspaceEnvError::ReservedName {
                path: path.to_path_buf(),
                name: key,
            });
        }
        match env.get(&key) {
            // The file's own first value stands, as it always has.
            Some(_) if filled.contains(&key) => {}
            Some(existing) if *existing != value => differs.push(key),
            Some(_) => {}
            None => {
                filled.insert(key.clone());
                env.insert(key, value);
            }
        }
    }
    differs.sort();
    differs.dedup();
    Ok(EnvFile {
        loaded: true,
        filled: filled.into_iter().collect(),
        differs,
    })
}

fn workspace_env_error(path: &Path, error: dotenvy::Error) -> WorkspaceEnvError {
    match error {
        dotenvy::Error::Io(source) => WorkspaceEnvError::Read {
            path: path.to_path_buf(),
            source,
        },
        dotenvy::Error::LineParse(_line, offset) => WorkspaceEnvError::Syntax {
            path: path.to_path_buf(),
            offset,
        },
        dotenvy::Error::EnvVar(_error) => WorkspaceEnvError::EnvironmentReference {
            path: path.to_path_buf(),
        },
        _ => WorkspaceEnvError::Parse {
            path: path.to_path_buf(),
        },
    }
}

pub(crate) fn merge_inferred_tool_roots(env: &mut BTreeMap<String, String>, skill_dir: &Path) {
    if let Some(joined) = inferred_tool_roots(skill_dir) {
        let key = crate::services::tool_roots::RUNX_TOOL_ROOTS_ENV.to_owned();
        env.entry(key)
            .and_modify(|existing| *existing = merge_path_env(existing, &joined))
            .or_insert(joined);
    }
}

fn merge_path_env(existing: &str, addition: &str) -> String {
    let mut paths: Vec<PathBuf> = std::env::split_paths(existing).collect();
    for path in std::env::split_paths(addition) {
        if !paths.iter().any(|existing_path| existing_path == &path) {
            paths.push(path);
        }
    }
    std::env::join_paths(paths)
        .ok()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            if existing.is_empty() {
                addition.to_owned()
            } else if addition.is_empty() {
                existing.to_owned()
            } else {
                let separator = if cfg!(windows) { ';' } else { ':' };
                format!("{existing}{separator}{addition}")
            }
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use super::{RUNX_CWD_ENV, WorkspaceEnv, WorkspaceEnvError, merge_path_env};

    #[test]
    fn workspace_env_loads_optional_file_without_mutating_process_env()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let key = format!("WORKSPACE_ENV_TEST_{}", std::process::id());
        assert!(std::env::var(&key).is_err());
        fs::write(temp.path().join(".env"), format!("{key}=from-file\n"))?;

        let workspace = WorkspaceEnv::load(BTreeMap::new(), temp.path().to_path_buf())?;

        assert_eq!(workspace.env().get(&key), Some(&"from-file".to_owned()));
        assert!(workspace.env_file_loaded());
        assert!(std::env::var(&key).is_err());
        Ok(())
    }

    #[test]
    fn workspace_env_preserves_ambient_precedence() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join(".env"), "PROVIDER_TOKEN=from-file\n")?;
        let ambient = BTreeMap::from([("PROVIDER_TOKEN".to_owned(), "from-process".to_owned())]);

        let workspace = WorkspaceEnv::load(ambient, temp.path().to_path_buf())?;

        assert_eq!(
            workspace.env().get("PROVIDER_TOKEN"),
            Some(&"from-process".to_owned())
        );
        assert!(super::env_file_differs(workspace.env(), "PROVIDER_TOKEN"));
        Ok(())
    }

    #[test]
    fn workspace_env_marks_only_the_names_the_file_sets_differently()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(".env"),
            "SAME=one\nDIFFERS=from-file\nFILE_ONLY=x\nFILE_ONLY=y\n",
        )?;
        let ambient = BTreeMap::from([
            ("SAME".to_owned(), "one".to_owned()),
            ("DIFFERS".to_owned(), "from-process".to_owned()),
            (
                super::RUNX_ENV_FILE_DIFFERS_ENV.to_owned(),
                "SAME,FILE_ONLY".to_owned(),
            ),
        ]);

        let workspace = WorkspaceEnv::load(ambient, temp.path().to_path_buf())?;

        assert_eq!(
            workspace.env().get(super::RUNX_ENV_FILE_DIFFERS_ENV),
            Some(&"DIFFERS".to_owned()),
            "recomputed here, never inherited, and never a value"
        );
        assert!(!super::env_file_differs(workspace.env(), "SAME"));
        assert!(!super::env_file_differs(workspace.env(), "FILE_ONLY"));
        assert_eq!(workspace.env().get("FILE_ONLY"), Some(&"x".to_owned()));
        Ok(())
    }

    #[test]
    fn the_process_environment_is_the_workspace_env_without_what_the_file_filled()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join(".env"),
            "SHELL_SET=from-file\nFILLED=from-file\nALSO_FILLED=from-file\n",
        )?;
        let ambient = BTreeMap::from([
            ("SHELL_SET".to_owned(), "from-shell".to_owned()),
            (
                super::RUNX_ENV_FILE_FILLED_ENV.to_owned(),
                "SHELL_SET".to_owned(),
            ),
        ]);

        let workspace = WorkspaceEnv::load(ambient, temp.path().to_path_buf())?;

        assert_eq!(
            workspace.env().get(super::RUNX_ENV_FILE_FILLED_ENV),
            Some(&"ALSO_FILLED,FILLED".to_owned()),
            "recomputed here, never inherited, and never a value"
        );
        let process = super::process_environment(workspace.env());
        assert_eq!(process.get("SHELL_SET"), Some(&"from-shell".to_owned()));
        assert!(!process.contains_key("FILLED"));
        assert!(!process.contains_key("ALSO_FILLED"));
        Ok(())
    }

    /// A workspace's file cannot redirect Runx's state, Connect or signing,
    /// nor admit authority a host supplies.
    #[test]
    fn workspace_env_never_sets_runx_variables() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        for name in [
            "RUNX_CWD",
            "RUNX_HOME",
            "RUNX_PUBLIC_API_BASE_URL",
            "RUNX_RECEIPT_SIGN_KID",
            "HOME",
            "USERPROFILE",
        ] {
            fs::write(temp.path().join(".env"), format!("{name}=from-file\n"))?;
            let refused = WorkspaceEnv::load(BTreeMap::new(), temp.path().to_path_buf())
                .err()
                .ok_or("a runx variable loaded from .env")?;
            assert!(
                matches!(&refused, WorkspaceEnvError::ReservedName { name: refused, .. } if refused == name),
                "{refused}"
            );
        }
        Ok(())
    }

    #[test]
    fn supplied_workspace_env_freezes_resolved_cwd() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;

        let workspace = WorkspaceEnv::new(BTreeMap::new(), temp.path().to_path_buf())?;

        assert_eq!(workspace.cwd(), temp.path());
        assert_eq!(
            workspace.env().get(RUNX_CWD_ENV),
            Some(&temp.path().to_string_lossy().into_owned())
        );
        Ok(())
    }

    #[test]
    fn workspace_env_rejects_relative_or_missing_workspace_identity() {
        assert!(matches!(
            WorkspaceEnv::new(BTreeMap::new(), PathBuf::from("relative")),
            Err(WorkspaceEnvError::RelativeCwd { .. })
        ));
        assert!(matches!(
            WorkspaceEnv::from_admitted(BTreeMap::new()),
            Err(WorkspaceEnvError::MissingCwd)
        ));
    }

    #[test]
    fn workspace_env_errors_do_not_expose_invalid_secret_lines()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let secret = "never-print-this-secret";
        fs::write(
            temp.path().join(".env"),
            format!("PROVIDER_TOKEN='{secret}\n"),
        )?;

        let error = WorkspaceEnv::load(BTreeMap::new(), temp.path().to_path_buf())
            .err()
            .ok_or("invalid .env unexpectedly loaded")?;
        let rendered = error.to_string();

        assert!(rendered.contains("invalid syntax"));
        assert!(!rendered.contains(secret));
        Ok(())
    }

    #[test]
    fn workspace_env_debug_does_not_expose_values() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let secret = "never-debug-this-secret";
        fs::write(
            temp.path().join(".env"),
            format!("PROVIDER_TOKEN={secret}\n"),
        )?;

        let workspace = WorkspaceEnv::load(BTreeMap::new(), temp.path().to_path_buf())?;
        let rendered = format!("{workspace:?}");

        assert!(rendered.contains("env_key_count"));
        assert!(!rendered.contains(secret));
        Ok(())
    }

    #[test]
    fn merge_path_env_appends_new_paths_and_deduplicates_existing_paths()
    -> Result<(), std::env::JoinPathsError> {
        let first = PathBuf::from("/runx/tools");
        let second = PathBuf::from("/runx/skills/data-store/tools");
        let existing = path_list_string([first.as_path()])?;
        let addition = path_list_string([second.as_path(), first.as_path()])?;

        let merged = merge_path_env(&existing, &addition);
        let paths = std::env::split_paths(&merged).collect::<Vec<_>>();

        assert_eq!(paths, vec![first, second]);
        Ok(())
    }

    fn path_list_string<'a>(
        paths: impl IntoIterator<Item = &'a std::path::Path>,
    ) -> Result<String, std::env::JoinPathsError> {
        std::env::join_paths(paths).map(|value| value.to_string_lossy().into_owned())
    }
}
