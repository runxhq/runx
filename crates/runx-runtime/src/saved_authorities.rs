//! Saved authorities (authority.md, D31): an authority a person saves in
//! their runx home by name, for the workspaces it lists by workspace id, and
//! which applies where a run or a server names it with `--authority`.
//!
//! Nothing else applies one unnamed, because no host stated doesn't mean a
//! person is there, but the one a person saves as standing: their own data,
//! `private` alone, for the runs in its workspaces that state no authority,
//! as an agent's memory is there for its own work. Naming one from a
//! workspace it doesn't list refuses, because a repository's `.mcp.json` can
//! name one. A home that fell back to the workspace keeps none, and one
//! planted there is never read (D30). It speaks in connector and package
//! names, and the root's admission resolves them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::kernel::authority::RunAuthority;
use crate::paths::{RUNX_CWD_ENV, RUNX_PROJECT_DIR_ENV, RunxPaths};
use crate::private_file::{
    PrivateFileFault, ensure_private_directory, read_private, replace_private,
};
use crate::workspace_ids::{self, WorkspaceIdError};

/// The most a saved authority's file may hold.
const MAX_SAVED_AUTHORITY_BYTES: u64 = 1024 * 1024;
const MAX_NAME_LENGTH: usize = 64;

/// What a saved authority's file holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedAuthority {
    /// The ids of the workspaces it's for.
    pub workspaces: Vec<String>,
    /// The authority it gives, in connector and package names.
    pub authority: RunAuthority,
    /// It's what every run in its workspaces that states no authority starts
    /// with. It gives only the person's own data, and a workspace has one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub standing: bool,
}

/// Why a saved authority can't be saved, read or applied.
#[derive(Debug, Error)]
pub enum SavedAuthorityError {
    #[error(
        "the runx home fell back to the workspace, so it keeps no saved authority, which a repository could ship: set HOME or RUNX_HOME"
    )]
    HomeFellBack,
    #[error(
        "{name:?} is not an authority name: lowercase letters, digits and -, starting with a letter or digit, at most {MAX_NAME_LENGTH}"
    )]
    InvalidName { name: String },
    #[error("no saved authority is named {name}; save one with runx authority save {name} <file>")]
    NotSaved { name: String },
    #[error(
        "the saved authority {name} isn't for this workspace; save it for this one too with runx authority save {name} <file> --workspace <path>"
    )]
    OtherWorkspace { name: String },
    #[error(
        "{path} is no workspace, so no saved authority is for it: a workspace is a directory with .git, pnpm-workspace.yaml, skills/ or connectors/, or one inside it"
    )]
    NoWorkspace { path: String },
    #[error("the authority doesn't resolve in {workspace}: {reason}")]
    Unresolved { workspace: String, reason: String },
    #[error(
        "a standing authority applies to runs that state none, so it gives only the person's own data: delegations on private, and nothing else"
    )]
    NotOwnData,
    #[error(
        "this workspace's standing authority is {other}; remove it with runx authority remove {other} first"
    )]
    StandingTaken { other: String },
    #[error(transparent)]
    WorkspaceId(#[from] WorkspaceIdError),
    #[error("{path} {fault}")]
    File {
        path: String,
        fault: PrivateFileFault,
    },
    #[error("{path} is not a saved authority: {message}")]
    Malformed { path: String, message: String },
}

/// Save `authority` as `name` for `workspaces`, or, with none, for the
/// workspace runx runs in, once it resolves in each of them: each
/// delegation's connector stands for an instance there. `standing`, it's
/// what those workspaces' runs that state no authority start with.
///
/// # Errors
///
/// Returns why it can't: a home that fell back, a bad name, a path that's no
/// workspace, an authority that doesn't resolve in one of them, or, standing,
/// one that gives more than the person's own data or a workspace that has a
/// standing authority already.
pub fn save(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    name: &str,
    authority: RunAuthority,
    workspaces: &[PathBuf],
    standing: bool,
    now: &str,
) -> Result<SavedAuthority, SavedAuthorityError> {
    let paths = RunxPaths::resolve(env, cwd);
    if paths.home_fell_back() {
        return Err(SavedAuthorityError::HomeFellBack);
    }
    let file = authority_file(&paths, name)?;
    if standing && !own_data(&authority) {
        return Err(SavedAuthorityError::NotOwnData);
    }
    let places = if workspaces.is_empty() {
        vec![(env.clone(), paths.clone())]
    } else {
        workspaces
            .iter()
            .map(|path| {
                // Another workspace, as runx would find it run there.
                let mut there = env.clone();
                there.insert(RUNX_CWD_ENV.to_owned(), path.display().to_string());
                there.remove(RUNX_PROJECT_DIR_ENV);
                let place = RunxPaths::resolve(&there, path);
                (there, place)
            })
            .collect()
    };
    let mut ids = Vec::with_capacity(places.len());
    for (env, place) in &places {
        let workspace = place.workspace().display().to_string();
        let id = workspace_ids::workspace_id(place, now)?.ok_or_else(|| {
            SavedAuthorityError::NoWorkspace {
                path: workspace.clone(),
            }
        })?;
        crate::kernel::admission::resolved_authority(
            authority.clone(),
            place.workspace(),
            env,
            place.workspace(),
        )
        .map_err(|reason| SavedAuthorityError::Unresolved {
            workspace: workspace.clone(),
            reason,
        })?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if standing
        && let Some((other, _)) = list(&paths)?.into_iter().find(|(other, saved)| {
            other != name && saved.standing && saved.workspaces.iter().any(|id| ids.contains(id))
        })
    {
        return Err(SavedAuthorityError::StandingTaken { other });
    }
    let saved = SavedAuthority {
        workspaces: ids,
        authority,
        standing,
    };
    let directory = paths.saved_authorities();
    ensure_private_directory(&directory).map_err(|fault| SavedAuthorityError::File {
        path: directory.display().to_string(),
        fault,
    })?;
    let bytes =
        serde_json::to_vec_pretty(&saved).map_err(|error| SavedAuthorityError::Malformed {
            path: file.display().to_string(),
            message: error.to_string(),
        })?;
    replace_private(&file, &bytes).map_err(|fault| SavedAuthorityError::File {
        path: file.display().to_string(),
        fault,
    })?;
    Ok(saved)
}

/// Every saved authority, by name.
///
/// # Errors
///
/// Returns why the home's saved authorities can't be read.
pub fn list(paths: &RunxPaths) -> Result<Vec<(String, SavedAuthority)>, SavedAuthorityError> {
    if paths.home_fell_back() {
        return Err(SavedAuthorityError::HomeFellBack);
    }
    let directory = paths.saved_authorities();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(SavedAuthorityError::File {
                path: directory.display().to_string(),
                fault: crate::private_file::io_fault(&error),
            });
        }
    };
    let mut names = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|file| file.strip_suffix(".json"))
                .filter(|name| valid_name(name))
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .map(|name| show(paths, &name).map(|saved| (name, saved)))
        .collect()
}

/// The authority saved as `name`.
///
/// # Errors
///
/// Returns why it can't be read: a home that fell back, a bad name, none
/// saved, or a file runx didn't write.
pub fn show(paths: &RunxPaths, name: &str) -> Result<SavedAuthority, SavedAuthorityError> {
    if paths.home_fell_back() {
        return Err(SavedAuthorityError::HomeFellBack);
    }
    let file = authority_file(paths, name)?;
    let bytes = read_private(&file, MAX_SAVED_AUTHORITY_BYTES)
        .map_err(|fault| SavedAuthorityError::File {
            path: file.display().to_string(),
            fault,
        })?
        .ok_or_else(|| SavedAuthorityError::NotSaved {
            name: name.to_owned(),
        })?;
    serde_json::from_slice(&bytes).map_err(|error| SavedAuthorityError::Malformed {
        path: file.display().to_string(),
        message: error.to_string(),
    })
}

/// Remove the authority saved as `name`.
///
/// # Errors
///
/// Returns why it can't: a home that fell back, a bad name, or none saved.
pub fn remove(paths: &RunxPaths, name: &str) -> Result<(), SavedAuthorityError> {
    show(paths, name)?;
    let file = authority_file(paths, name)?;
    std::fs::remove_file(&file).map_err(|error| SavedAuthorityError::File {
        path: file.display().to_string(),
        fault: crate::private_file::io_fault(&error),
    })
}

/// What forgetting a workspace did to one saved authority: it's no longer for
/// that workspace, and `removed` when it was for no other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Narrowed {
    pub name: String,
    pub removed: bool,
}

/// Take the workspace `id` out of every saved authority, removing one that
/// was for it alone, as forgetting a workspace does.
///
/// # Errors
///
/// Returns why the home's saved authorities can't be read or written.
pub fn forget_workspace(paths: &RunxPaths, id: &str) -> Result<Vec<Narrowed>, SavedAuthorityError> {
    let mut narrowed = Vec::new();
    for (name, mut saved) in list(paths)? {
        if !saved.workspaces.iter().any(|workspace| workspace == id) {
            continue;
        }
        saved.workspaces.retain(|workspace| workspace != id);
        let file = authority_file(paths, &name)?;
        if saved.workspaces.is_empty() {
            std::fs::remove_file(&file).map_err(|error| SavedAuthorityError::File {
                path: file.display().to_string(),
                fault: crate::private_file::io_fault(&error),
            })?;
        } else {
            let bytes = serde_json::to_vec_pretty(&saved).map_err(|error| {
                SavedAuthorityError::Malformed {
                    path: file.display().to_string(),
                    message: error.to_string(),
                }
            })?;
            replace_private(&file, &bytes).map_err(|fault| SavedAuthorityError::File {
                path: file.display().to_string(),
                fault,
            })?;
        }
        narrowed.push(Narrowed {
            name,
            removed: saved.workspaces.is_empty(),
        });
    }
    Ok(narrowed)
}

/// The authority a run or a server named `name` starts with: saved in this
/// home, for the workspace runx runs in.
///
/// # Errors
///
/// Returns why it doesn't apply: a home that fell back, none saved, no
/// workspace here, or one it isn't for.
pub fn named(
    paths: &RunxPaths,
    name: &str,
    now: &str,
) -> Result<RunAuthority, SavedAuthorityError> {
    let saved = show(paths, name)?;
    let id = workspace_ids::workspace_id(paths, now)?.ok_or_else(|| {
        SavedAuthorityError::NoWorkspace {
            path: paths.workspace().display().to_string(),
        }
    })?;
    if !saved.workspaces.contains(&id) {
        return Err(SavedAuthorityError::OtherWorkspace {
            name: name.to_owned(),
        });
    }
    Ok(saved.authority)
}

/// The authority a run in the workspace runx runs in starts with when it
/// states none: the one the person saved as standing for it, if any. A
/// workspace this home has minted no id for has none, and none is minted.
///
/// # Errors
///
/// Returns why the home's saved authorities or workspace ids can't be read.
pub fn standing(paths: &RunxPaths) -> Result<Option<RunAuthority>, SavedAuthorityError> {
    if paths.home_fell_back() || !paths.workspace_found() {
        return Ok(None);
    }
    let standing = list(paths)?
        .into_iter()
        .filter(|(_, saved)| saved.standing)
        .collect::<Vec<_>>();
    if standing.is_empty() {
        return Ok(None);
    }
    let Some(id) = workspace_ids::known_id(paths)? else {
        return Ok(None);
    };
    Ok(standing
        .into_iter()
        .find(|(_, saved)| saved.workspaces.contains(&id))
        .map(|(_, saved)| saved.authority))
}

/// Whether `authority` gives only the person's own data: delegations on
/// `private`, and nothing else a run's authority can say.
fn own_data(authority: &RunAuthority) -> bool {
    !authority.delegations.is_empty()
        && authority
            .delegations
            .iter()
            .all(|delegation| delegation.connector.as_str() == crate::drivers::store::PRIVATE)
        && RunAuthority {
            delegations: Vec::new(),
            ..authority.clone()
        } == RunAuthority::default()
}

fn authority_file(paths: &RunxPaths, name: &str) -> Result<PathBuf, SavedAuthorityError> {
    if !valid_name(name) {
        return Err(SavedAuthorityError::InvalidName {
            name: name.to_owned(),
        });
    }
    Ok(paths.saved_authorities().join(format!("{name}.json")))
}

fn valid_name(name: &str) -> bool {
    name.len() <= MAX_NAME_LENGTH
        && name
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
