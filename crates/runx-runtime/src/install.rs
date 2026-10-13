//! The installation's identity: an id recorded in the runx home the first
//! time something needs it, and read back after. Nothing sets it up ahead.

use std::path::PathBuf;

use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::paths::RunxPaths;
use crate::private_file::{create_private_new, read_private};

/// The largest install state runx reads.
const MAX_INSTALL_STATE_BYTES: u64 = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct InstallState {
    version: u8,
    installation_id: String,
    created_at: String,
}

#[derive(Debug, Error)]
pub enum InstallStateError {
    #[error("install state {} {fault}", path.display())]
    File { path: PathBuf, fault: String },
    #[error("install state {} is invalid: {message}", path.display())]
    Invalid { path: PathBuf, message: String },
    #[error("creating the runx home {}: {source}", path.display())]
    Home {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("no randomness for an installation id")]
    Randomness,
}

/// This installation's id, recorded in the runx home on first use.
///
/// # Errors
///
/// Returns an error when the recorded state cannot be read or is invalid, or
/// when a new one cannot be written.
pub fn installation_id(paths: &RunxPaths) -> Result<String, InstallStateError> {
    let path = paths.install_state();
    if let Some(recorded) = read_state(&path)? {
        return Ok(recorded.installation_id);
    }
    let state = InstallState {
        version: 1,
        installation_id: format!("inst_{}", random_id()?),
        created_at: crate::time::now_iso8601(),
    };
    std::fs::create_dir_all(paths.home()).map_err(|source| InstallStateError::Home {
        path: paths.home().to_path_buf(),
        source,
    })?;
    let contents =
        serde_json::to_vec_pretty(&state).map_err(|error| InstallStateError::Invalid {
            path: path.clone(),
            message: error.to_string(),
        })?;
    match create_private_new(&path, &contents) {
        Ok(true) => Ok(state.installation_id),
        // Another process recorded it first; its id is the installation's.
        Ok(false) => read_state(&path)?
            .map(|recorded| recorded.installation_id)
            .ok_or(InstallStateError::File {
                path,
                fault: "vanished as it was recorded".to_owned(),
            }),
        Err(fault) => Err(InstallStateError::File {
            path,
            fault: fault.to_string(),
        }),
    }
}

fn read_state(path: &std::path::Path) -> Result<Option<InstallState>, InstallStateError> {
    let Some(bytes) =
        read_private(path, MAX_INSTALL_STATE_BYTES).map_err(|fault| InstallStateError::File {
            path: path.to_path_buf(),
            fault: fault.to_string(),
        })?
    else {
        return Ok(None);
    };
    let invalid = |message: String| InstallStateError::Invalid {
        path: path.to_path_buf(),
        message,
    };
    let state = serde_json::from_slice::<InstallState>(&bytes)
        .map_err(|error| invalid(error.to_string()))?;
    if state.version != 1 || state.installation_id.is_empty() || state.created_at.is_empty() {
        return Err(invalid(
            "expected version 1, installation_id and created_at".to_owned(),
        ));
    }
    Ok(Some(state))
}

fn random_id() -> Result<String, InstallStateError> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| InstallStateError::Randomness)?;
    Ok(runx_contracts::hex_lower(&bytes))
}

#[cfg(test)]
mod tests {
    use super::installation_id;
    use crate::paths::RunxPaths;

    /// The installation's id is recorded once, on first use, and read back
    /// after.
    #[test]
    fn the_installation_id_is_recorded_once() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let env = [
            ("RUNX_CWD".to_owned(), root.path().display().to_string()),
            (
                "RUNX_HOME".to_owned(),
                root.path().join("home").display().to_string(),
            ),
        ]
        .into();
        let paths = RunxPaths::resolve(&env, root.path());

        let first = installation_id(&paths)?;
        let again = installation_id(&paths)?;

        assert!(first.starts_with("inst_"));
        assert_eq!(first, again);
        assert!(root.path().join("home").join("install.json").exists());
        Ok(())
    }
}
