//! Local credential profiles: named secrets in the operator's runx home that
//! connector instances deliver. The secret is stored encrypted at rest; the
//! config records only its reference.

use std::io;

use runx_contracts::connector::{ConnectorRef, Digest};
use serde::Serialize;
use thiserror::Error;

use crate::config::{
    ConfigError, RunxCredentialProfile, load_runx_config_file, remove_local_credential_secret,
    store_local_credential_secret, write_runx_config_file,
};
use crate::services::WorkspaceEnv;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CredentialProfileSummary {
    pub name: String,
    pub connector: ConnectorRef,
    pub credential_declaration_digest: Digest,
    pub credential: String,
}

#[derive(Debug, Error)]
pub enum CredentialProfileError {
    #[error("credential profile name must not be empty")]
    EmptyProfileName,
    #[error("connector credential name must not be empty")]
    EmptyCredential,
    #[error("credential secret must not be empty")]
    EmptySecret,
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Store `secret` as profile `name`, holding `connector`'s credential
/// `credential`, and return the reference to the stored material, which an
/// instance pins: replacing the profile mints a new one.
pub fn set_local_credential_profile(
    workspace: &WorkspaceEnv,
    name: &str,
    connector: &ConnectorRef,
    credential_declaration_digest: &Digest,
    credential: &str,
    secret: &str,
) -> Result<String, CredentialProfileError> {
    let name = required(name, CredentialProfileError::EmptyProfileName)?;
    let credential = required(credential, CredentialProfileError::EmptyCredential)?;
    if secret.trim().is_empty() {
        return Err(CredentialProfileError::EmptySecret);
    }
    let paths = workspace.paths();
    let config_path = paths.config_file();
    let mut config = load_runx_config_file(&config_path)?;
    let credentials = config.credentials.get_or_insert_with(Default::default);
    let prior_ref = credentials
        .profiles
        .get(name)
        .map(|profile| profile.secret_ref.clone());
    let secret_ref = store_local_credential_secret(paths, secret)?;
    credentials.profiles.insert(
        name.to_owned(),
        RunxCredentialProfile {
            connector: connector.clone(),
            credential_declaration_digest: credential_declaration_digest.clone(),
            credential: credential.to_owned(),
            secret_ref: secret_ref.clone(),
        },
    );
    write_runx_config_file(&config_path, &config)?;
    if let Some(prior_ref) = prior_ref {
        remove_local_credential_secret(paths, &prior_ref)?;
    }
    Ok(secret_ref)
}

pub fn list_local_credential_profiles(
    workspace: &WorkspaceEnv,
) -> Result<Vec<CredentialProfileSummary>, CredentialProfileError> {
    let config = load_runx_config_file(&workspace.paths().config_file())?;
    Ok(config
        .credentials
        .unwrap_or_default()
        .profiles
        .into_iter()
        .map(|(name, profile)| CredentialProfileSummary {
            name,
            connector: profile.connector,
            credential_declaration_digest: profile.credential_declaration_digest,
            credential: profile.credential,
        })
        .collect())
}

pub fn remove_local_credential_profile(
    workspace: &WorkspaceEnv,
    name: &str,
) -> Result<bool, CredentialProfileError> {
    let name = required(name, CredentialProfileError::EmptyProfileName)?;
    let paths = workspace.paths();
    let config_path = paths.config_file();
    let mut config = load_runx_config_file(&config_path)?;
    let Some(credentials) = config.credentials.as_mut() else {
        return Ok(false);
    };
    let Some(profile) = credentials.profiles.remove(name) else {
        return Ok(false);
    };
    write_runx_config_file(&config_path, &config)?;
    remove_local_credential_secret(paths, &profile.secret_ref)?;
    Ok(true)
}

fn required(value: &str, error: CredentialProfileError) -> Result<&str, CredentialProfileError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(error);
    }
    Ok(value)
}
