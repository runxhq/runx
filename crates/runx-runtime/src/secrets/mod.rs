//! The secrets the operator keeps in runx: each a name in the runx home that
//! holds one value, sealed here or read through a secrets manager by
//! reference. `runx secret` sets, lists, removes and reads them; a connector
//! binding, the managed agent and a host each name one. This module is the
//! only writer and the only reader of a value.

pub(crate) mod reference;
pub(crate) mod sealed;

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::{SecretName, SecretReference};
use serde::Serialize;
use thiserror::Error;

pub use reference::Purpose;
pub use sealed::SealedError;

use crate::config::{ConfigError, RunxSecretRecord, load_runx_config_file, write_runx_config_file};
use crate::credentials::SecretString;
use crate::paths::RunxPaths;

/// What a sealed secret's file is named after.
const SEALED_KIND: &str = "local_secret";

/// A secret as `runx secret list` shows it: its name and where its value is,
/// never the value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecretSummary {
    pub name: SecretName,
    #[serde(flatten)]
    pub source: SecretSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SecretSource {
    /// Sealed in the runx home.
    Stored,
    /// In a secrets manager, at `reference`, where setting it pinned.
    Reference {
        reference: SecretReference,
        #[serde(skip_serializing_if = "Option::is_none")]
        pinned: Option<String>,
    },
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("no secret is named {name}; set it with `runx secret set {name}`")]
    NotSet { name: SecretName },
    #[error("a secret's value is never empty")]
    Empty,
    #[error("{0}")]
    Read(String),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Sealed(#[from] SealedError),
}

/// Seal `value` as the secret `name`: a new secret, or the next value of one
/// already set, which everything that names it serves from its next call.
pub fn set_value(paths: &RunxPaths, name: &SecretName, value: &str) -> Result<(), SecretError> {
    if value.trim().is_empty() {
        return Err(SecretError::Empty);
    }
    let material_id = sealed::store(paths, value, SEALED_KIND)?;
    replace(
        paths,
        name,
        RunxSecretRecord::Stored {
            material_id: material_id.clone(),
        },
    )
    .inspect_err(|_| {
        // The record did not change, so the new file is nobody's.
        let _ = sealed::remove(paths, &material_id);
    })
}

/// Point the secret `name` at `reference`. It is read once now, through the
/// manager's CLI and the operator's sign-in, to prove access and pin where
/// it is; the home keeps the reference and the pin, never the value.
pub fn set_reference(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    name: &SecretName,
    reference: &SecretReference,
) -> Result<(), SecretError> {
    let read =
        reference::read(env, cwd, reference, None, Purpose::Bind).map_err(SecretError::Read)?;
    replace(
        &RunxPaths::resolve(env, cwd),
        name,
        RunxSecretRecord::Reference {
            reference: reference.clone(),
            pinned: read.pinned,
        },
    )
}

/// Every secret this home keeps, by name.
pub fn list(paths: &RunxPaths) -> Result<Vec<SecretSummary>, SecretError> {
    Ok(load_runx_config_file(&paths.config_file())?
        .secrets
        .into_iter()
        .map(|(name, record)| SecretSummary {
            name,
            source: match record {
                RunxSecretRecord::Stored { .. } => SecretSource::Stored,
                RunxSecretRecord::Reference { reference, pinned } => {
                    SecretSource::Reference { reference, pinned }
                }
            },
        })
        .collect())
}

/// Whether the secret `name` is set.
pub fn is_set(paths: &RunxPaths, name: &SecretName) -> Result<bool, SecretError> {
    Ok(load_runx_config_file(&paths.config_file())?
        .secrets
        .contains_key(name))
}

/// Remove the secret `name`, and its sealed file. Whatever still names it
/// refuses at its next use. `false` when no secret had that name.
pub fn remove(paths: &RunxPaths, name: &SecretName) -> Result<bool, SecretError> {
    let path = paths.config_file();
    let mut config = load_runx_config_file(&path)?;
    let Some(record) = config.secrets.remove(name) else {
        return Ok(false);
    };
    write_runx_config_file(&path, &config)?;
    if let RunxSecretRecord::Stored { material_id } = record {
        sealed::remove(paths, &material_id)?;
    }
    Ok(true)
}

/// The secret `name`'s value as text, for `runx secret read` to print to the
/// operator: the one place a value leaves runx on purpose. Read as the
/// operator at a terminal is, so a manager's own message shows.
pub fn reveal(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    name: &SecretName,
) -> Result<String, SecretError> {
    Ok(read(env, cwd, name, Purpose::Bind)?.expose().to_owned())
}

/// The secret `name`'s value, read now: unsealed, or read through its
/// manager at the pin. Nothing is kept: every use reads again.
pub(crate) fn read(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    name: &SecretName,
    purpose: Purpose,
) -> Result<SecretString, SecretError> {
    let paths = RunxPaths::resolve(env, cwd);
    let record = load_runx_config_file(&paths.config_file())?
        .secrets
        .remove(name)
        .ok_or_else(|| SecretError::NotSet { name: name.clone() })?;
    let value = match record {
        RunxSecretRecord::Stored { material_id } => sealed::load(&paths, &material_id)?,
        RunxSecretRecord::Reference { reference, pinned } => {
            reference::read(env, cwd, &reference, pinned.as_deref(), purpose)
                .map_err(SecretError::Read)?
                .value
        }
    };
    if value.trim().is_empty() {
        return Err(SecretError::Empty);
    }
    Ok(SecretString::new(value))
}

/// An HMAC keyed by this home over `message`: proof that this home made a
/// binding, which a project store cannot forge.
pub(crate) fn prove(paths: &RunxPaths, message: &[u8]) -> Result<String, SealedError> {
    sealed::prove(paths, message)
}

/// Record `record` as the secret `name`, and drop the file a stored value it
/// replaces was sealed in.
fn replace(
    paths: &RunxPaths,
    name: &SecretName,
    record: RunxSecretRecord,
) -> Result<(), SecretError> {
    let path = paths.config_file();
    let mut config = load_runx_config_file(&path)?;
    let replaced = config.secrets.insert(name.clone(), record);
    write_runx_config_file(&path, &config)?;
    if let Some(RunxSecretRecord::Stored { material_id }) = replaced {
        sealed::remove(paths, &material_id)?;
    }
    Ok(())
}
