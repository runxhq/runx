//! Workspace ids (D17, memory.md): each workspace a person runs runx in is
//! named by an id minted in their runx home, keyed by the workspace's
//! canonical path. The id is never read from the workspace, so a repository
//! can't ship another workspace's id and read that workspace's memory, and a
//! saved authority's workspace check rests on the same id.
//!
//! runx can't tell a move from a copy, so a moved workspace is minted a new id
//! where it now is, and `runx workspace rebind <old path>` gives it back the
//! id it had.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::paths::RunxPaths;
use crate::private_file::{
    PrivateFileFault, ensure_private_directory, read_private, replace_private,
};

/// The most a workspace ids file may hold.
const MAX_WORKSPACE_IDS_BYTES: u64 = 4 * 1024 * 1024;

/// One workspace a person ran runx in, and the id it was minted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceIdEntry {
    /// The workspace's canonical path.
    pub path: String,
    pub id: String,
    pub minted_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceIds {
    workspaces: Vec<WorkspaceIdEntry>,
}

/// What a rebind did: the id the workspace now has, and the one minted for it
/// here, if any, which it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebound {
    pub path: String,
    pub id: String,
    pub replaced: Option<String>,
}

/// Why a workspace id can't be minted, read or rebound.
#[derive(Debug, Error)]
pub enum WorkspaceIdError {
    #[error(
        "the runx home fell back to the workspace, so it can't hold a workspace id a repository couldn't ship: set HOME or RUNX_HOME"
    )]
    HomeFellBack,
    #[error(
        "runx has no workspace here: a workspace is a directory with .git, skills/ or connectors/, or one RUNX_CWD names"
    )]
    NoWorkspace,
    #[error("{path} has no workspace id in this runx home, so there's nothing to rebind")]
    NotKnown { path: String },
    #[error("{path} {fault}")]
    File {
        path: String,
        fault: PrivateFileFault,
    },
    #[error("{path} could not be read or written: {message}")]
    Io { path: String, message: String },
    #[error("{path} is not a workspace ids file: {message}")]
    Malformed { path: String, message: String },
}

/// The id of the workspace runx runs in, minted in the runx home the first
/// time runx runs there. `None` when there's no workspace. A home that fell
/// back to the workspace mints none, because a repository could ship it.
///
/// # Errors
///
/// Returns why the id can't be read or minted.
pub fn workspace_id(paths: &RunxPaths, now: &str) -> Result<Option<String>, WorkspaceIdError> {
    if !paths.workspace_found() {
        return Ok(None);
    }
    if paths.home_fell_back() {
        return Err(WorkspaceIdError::HomeFellBack);
    }
    let workspace = canonical(paths.workspace())?;
    with_ids(paths, |ids| {
        if let Some(entry) = ids.workspaces.iter().find(|entry| entry.path == workspace) {
            return Ok((entry.id.clone(), false));
        }
        let id = mint()?;
        ids.workspaces.push(WorkspaceIdEntry {
            path: workspace.clone(),
            id: id.clone(),
            minted_at: now.to_owned(),
        });
        Ok((id, true))
    })
    .map(Some)
}

/// Every workspace this runx home has minted an id for, by path.
///
/// # Errors
///
/// Returns why the home's workspace ids can't be read.
pub fn list(paths: &RunxPaths) -> Result<Vec<WorkspaceIdEntry>, WorkspaceIdError> {
    if paths.home_fell_back() {
        return Err(WorkspaceIdError::HomeFellBack);
    }
    let file = ids_file(paths);
    let mut ids = read(&file)?;
    ids.workspaces
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(ids.workspaces)
}

/// Give the workspace runx runs in the id `old` had, for a workspace moved
/// from `old`. Any id minted for it where it now is is replaced, and `old`
/// keeps none.
///
/// # Errors
///
/// Returns why it can't: no workspace here, a home that fell back, or no id
/// for `old`.
pub fn rebind(paths: &RunxPaths, old: &Path) -> Result<Rebound, WorkspaceIdError> {
    if !paths.workspace_found() {
        return Err(WorkspaceIdError::NoWorkspace);
    }
    if paths.home_fell_back() {
        return Err(WorkspaceIdError::HomeFellBack);
    }
    let workspace = canonical(paths.workspace())?;
    // The old path may be gone, so it's matched as written once made absolute.
    let old = old_path(paths, old);
    with_ids(paths, |ids| {
        let Some(at) = ids.workspaces.iter().position(|entry| entry.path == old) else {
            return Err(WorkspaceIdError::NotKnown { path: old.clone() });
        };
        let moved = ids.workspaces.remove(at);
        let replaced = ids
            .workspaces
            .iter()
            .position(|entry| entry.path == workspace)
            .map(|at| ids.workspaces.remove(at).id);
        ids.workspaces.push(WorkspaceIdEntry {
            path: workspace.clone(),
            id: moved.id.clone(),
            minted_at: moved.minted_at,
        });
        Ok((
            Rebound {
                path: workspace.clone(),
                id: moved.id,
                replaced,
            },
            true,
        ))
    })
}

/// Read the home's ids, change them under its lock, and write them back when
/// `change` says it changed them.
fn with_ids<T>(
    paths: &RunxPaths,
    change: impl FnOnce(&mut WorkspaceIds) -> Result<(T, bool), WorkspaceIdError>,
) -> Result<T, WorkspaceIdError> {
    let file = ids_file(paths);
    let home = paths.home();
    ensure_private_directory(home).map_err(|fault| WorkspaceIdError::File {
        path: home.display().to_string(),
        fault,
    })?;
    let lock_path = home.join("workspaces.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| io(&lock_path, &error))?;
    lock.lock_exclusive()
        .map_err(|error| io(&lock_path, &error))?;
    let mut ids = read(&file)?;
    let (value, changed) = change(&mut ids)?;
    if changed {
        let bytes = serde_json::to_vec_pretty(&ids).map_err(|error| WorkspaceIdError::Io {
            path: file.display().to_string(),
            message: error.to_string(),
        })?;
        replace_private(&file, &bytes).map_err(|fault| WorkspaceIdError::File {
            path: file.display().to_string(),
            fault,
        })?;
    }
    drop(lock);
    Ok(value)
}

fn read(file: &Path) -> Result<WorkspaceIds, WorkspaceIdError> {
    let Some(bytes) =
        read_private(file, MAX_WORKSPACE_IDS_BYTES).map_err(|fault| WorkspaceIdError::File {
            path: file.display().to_string(),
            fault,
        })?
    else {
        return Ok(WorkspaceIds::default());
    };
    let ids: WorkspaceIds =
        serde_json::from_slice(&bytes).map_err(|error| WorkspaceIdError::Malformed {
            path: file.display().to_string(),
            message: error.to_string(),
        })?;
    // One id per path and one path per id, or the file is not one runx wrote.
    let mut by_path = BTreeMap::new();
    let mut by_id = BTreeMap::new();
    for entry in &ids.workspaces {
        if by_path.insert(&entry.path, ()).is_some() || by_id.insert(&entry.id, ()).is_some() {
            return Err(WorkspaceIdError::Malformed {
                path: file.display().to_string(),
                message: format!("{} or its id is listed twice", entry.path),
            });
        }
    }
    Ok(ids)
}

fn ids_file(paths: &RunxPaths) -> PathBuf {
    paths.workspace_ids()
}

fn canonical(path: &Path) -> Result<String, WorkspaceIdError> {
    std::fs::canonicalize(path)
        .map(|path| path.display().to_string())
        .map_err(|error| io(path, &error))
}

fn old_path(paths: &RunxPaths, old: &Path) -> String {
    let absolute = if old.is_absolute() {
        old.to_path_buf()
    } else {
        paths.cwd().join(old)
    };
    let absolute = crate::path_util::lexical_normalize(&absolute);
    // A moved workspace's old path is gone, so its deepest ancestor that
    // still exists is made canonical and the rest kept as written, as the
    // path was when its id was minted.
    let mut rest = Vec::new();
    let mut existing = absolute.as_path();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(existing) {
            return rest
                .iter()
                .rev()
                .fold(canonical, |path, part| path.join(part))
                .display()
                .to_string();
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(part)) => {
                rest.push(part.to_owned());
                existing = parent;
            }
            _ => return absolute.display().to_string(),
        }
    }
}

/// A new workspace id: `ws_` and 128 random bits in hex.
fn mint() -> Result<String, WorkspaceIdError> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| WorkspaceIdError::Io {
            path: "the system's random source".to_owned(),
            message: "could not be read".to_owned(),
        })?;
    Ok(format!(
        "ws_{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn io(path: &Path, error: &std::io::Error) -> WorkspaceIdError {
    WorkspaceIdError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
