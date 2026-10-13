use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::InitError;
use super::ids::{now_iso8601, random_uuid_v4};
use crate::paths::RunxPaths;

/// `runx init`: mark the workspace as a runx project, so runx resolves it
/// from any directory beneath it. The installation's own state is recorded
/// on first use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunxInitOptions {
    pub paths: RunxPaths,
    pub generated: InitGeneratedValues,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitGeneratedValues {
    pub project_id: String,
    pub installation_id: String,
    pub created_at: String,
}

impl InitGeneratedValues {
    #[must_use]
    pub fn generate() -> Self {
        Self {
            project_id: format!("proj_{}", random_uuid_v4()),
            installation_id: format!("inst_{}", random_uuid_v4()),
            created_at: now_iso8601(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunxProjectState {
    pub version: u8,
    pub project_id: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunxInstallState {
    pub version: u8,
    pub installation_id: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunxInitResult {
    pub created: bool,
    pub project_dir: PathBuf,
    pub project_id: String,
}

pub fn runx_init(options: &RunxInitOptions) -> Result<RunxInitResult, InitError> {
    let ensured = ensure_runx_project_state(
        &options.paths,
        &options.generated.project_id,
        &options.generated.created_at,
    )?;
    let tools_dir = options.paths.project_tools();
    fs::create_dir_all(&tools_dir)
        .map_err(|source| InitError::io("creating tools directory", tools_dir, source))?;
    Ok(RunxInitResult {
        created: ensured.created,
        project_dir: options.paths.project().to_path_buf(),
        project_id: ensured.state.project_id,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnsuredProjectState {
    pub state: RunxProjectState,
    pub created: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnsuredInstallState {
    pub state: RunxInstallState,
    pub created: bool,
}

pub fn ensure_runx_project_state(
    paths: &RunxPaths,
    project_id: &str,
    created_at: &str,
) -> Result<EnsuredProjectState, InitError> {
    if let Some(existing) = read_runx_project_state(&paths.project_state())? {
        return Ok(EnsuredProjectState {
            state: existing,
            created: false,
        });
    }
    let state = RunxProjectState {
        version: 1,
        project_id: project_id.to_owned(),
        created_at: created_at.to_owned(),
    };
    let project_dir = paths.project();
    fs::create_dir_all(project_dir)
        .map_err(|source| InitError::io("creating project directory", project_dir, source))?;
    write_state_json(&paths.project_state(), &state)?;
    Ok(EnsuredProjectState {
        state,
        created: true,
    })
}

pub fn ensure_runx_install_state(
    paths: &RunxPaths,
    installation_id: &str,
    created_at: &str,
) -> Result<EnsuredInstallState, InitError> {
    if let Some(existing) = read_runx_install_state(&paths.install_state())? {
        return Ok(EnsuredInstallState {
            state: existing,
            created: false,
        });
    }
    let state = RunxInstallState {
        version: 1,
        installation_id: installation_id.to_owned(),
        created_at: created_at.to_owned(),
    };
    let home = paths.home();
    fs::create_dir_all(home)
        .map_err(|source| InitError::io("creating global home directory", home, source))?;
    write_state_json(&paths.install_state(), &state)?;
    Ok(EnsuredInstallState {
        state,
        created: true,
    })
}

fn read_runx_project_state(path: &Path) -> Result<Option<RunxProjectState>, InitError> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let state: RunxProjectState = serde_json::from_str(&contents)
                .map_err(|source| InitError::json("reading project state", path, source))?;
            validate_project_state(path.to_path_buf(), state).map(Some)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InitError::io("reading project state", path, source)),
    }
}

fn read_runx_install_state(path: &Path) -> Result<Option<RunxInstallState>, InitError> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let state: RunxInstallState = serde_json::from_str(&contents)
                .map_err(|source| InitError::json("reading install state", path, source))?;
            validate_install_state(path.to_path_buf(), state).map(Some)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InitError::io("reading install state", path, source)),
    }
}

fn validate_project_state(
    path: PathBuf,
    state: RunxProjectState,
) -> Result<RunxProjectState, InitError> {
    if state.version != 1 || state.project_id.is_empty() || state.created_at.is_empty() {
        return Err(InitError::InvalidState {
            path,
            message: "expected version 1, project_id, and created_at".to_owned(),
        });
    }
    Ok(state)
}

fn validate_install_state(
    path: PathBuf,
    state: RunxInstallState,
) -> Result<RunxInstallState, InitError> {
    if state.version != 1 || state.installation_id.is_empty() || state.created_at.is_empty() {
        return Err(InitError::InvalidState {
            path,
            message: "expected version 1, installation_id, and created_at".to_owned(),
        });
    }
    Ok(state)
}

fn write_state_json<T: Serialize>(path: &Path, state: &T) -> Result<(), InitError> {
    let contents = serde_json::to_string_pretty(state)
        .map_err(|source| InitError::json("serializing state", path, source))?;
    fs::write(path, format!("{contents}\n"))
        .map_err(|source| InitError::io("writing state", path, source))?;
    set_private_file_mode(path)
}

#[cfg(unix)]
fn set_private_file_mode(path: &Path) -> Result<(), InitError> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
        .map_err(|source| InitError::io("setting state permissions", path, source))
}

#[cfg(not(unix))]
fn set_private_file_mode(_path: &Path) -> Result<(), InitError> {
    Ok(())
}
