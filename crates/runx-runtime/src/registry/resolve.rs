//! The one resolver for a skill or connector ref: `runx skill`, `runx
//! connector`, `runx registry install` and a turn's load all find a package
//! here, as a path, an installed or bundled package, or a registry ref
//! fetched into the project.

use std::collections::BTreeMap;
use std::path::Path;

mod skill;
mod target;

pub use skill::{
    AbsentPins, RegistryTrustState, ResolvedSkillRef, SkillRefKind, SkillResolverOptions,
    resolve_skill_ref_details,
};
pub use target::{
    LocalRegistrySourceKind, RegistryTarget, registry_source_description, resolve_registry_target,
};

use runx_contracts::connector::ConnectorName;

use super::{
    AcquireOptions, AcquiredRegistrySkill, FileRegistryStore, InstallCandidate,
    InstallRegistryConnectorOptions, InstalledRegistryConnector, RegistryClient,
    RegistryManifestSourceAuthority, RegistryResolveOptions, RegistrySkillResolution,
    TrustedRegistryManifestKey, install_registry_connector, resolve_registry_skill,
};

/// Why a ref did not resolve.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// The ref or the environment is malformed; the caller's to fix.
    #[error("{0}")]
    Usage(String),
    /// Finding, fetching or installing the package failed.
    #[error("{0}")]
    Failed(String),
}

impl ResolveError {
    #[must_use]
    pub fn into_message(self) -> String {
        match self {
            Self::Usage(message) | Self::Failed(message) => message,
        }
    }
}

impl From<super::RegistryClientError> for ResolveError {
    fn from(error: super::RegistryClientError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<super::LocalRegistryError> for ResolveError {
    fn from(error: super::LocalRegistryError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<super::InstallError> for ResolveError {
    fn from(error: super::InstallError) -> Self {
        let kind = match &error {
            super::InstallError::UnsignedManifest(_) => Some("unsigned_manifest"),
            super::InstallError::UnknownManifestKey { .. } => Some("unknown_key"),
            super::InstallError::InvalidManifestSignature { .. } => Some("invalid_signature"),
            super::InstallError::DigestMismatch { .. } => Some("digest_mismatch"),
            _ => None,
        };
        Self::Failed(match kind {
            Some(kind) => format!("registry install {kind}: {error}"),
            None => error.to_string(),
        })
    }
}

impl From<super::RegistryManifestTrustEnvError> for ResolveError {
    fn from(error: super::RegistryManifestTrustEnvError) -> Self {
        match error {
            super::RegistryManifestTrustEnvError::InvalidKey => Self::Failed(error.to_string()),
            super::RegistryManifestTrustEnvError::MissingKeyId
            | super::RegistryManifestTrustEnvError::MissingOwner
            | super::RegistryManifestTrustEnvError::MissingSource => Self::Usage(error.to_string()),
        }
    }
}

/// The skill `subject` names at `version` in `target`, and how the registry
/// served it; none when the registry publishes no skill under that name and
/// version.
///
/// # Errors
///
/// Returns an error when the registry cannot be reached or read.
pub fn install_candidate(
    subject: &str,
    version: Option<&str>,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Option<(InstallCandidate, Option<AcquiredRegistrySkill>)>, ResolveError> {
    let source_authority = target.manifest_source_authority();
    match target {
        RegistryTarget::Remote { registry_url } => {
            let client = registry_client(&registry_url, env)?;
            // One segment reaches the registry only as a root name, which it
            // resolves to the canonical `<owner>/<name>` it installs.
            let skill_id = if subject.contains('/') {
                subject.to_owned()
            } else {
                match client.read_root(subject, version)? {
                    Some(detail) => detail.skill_id,
                    None => return Ok(None),
                }
            };
            let installation_id = installation_id(env, cwd)?;
            let acquired = match client.acquire(
                &skill_id,
                AcquireOptions {
                    installation_id: &installation_id,
                    version,
                    channel: Some("cli"),
                },
            ) {
                Err(super::RegistryClientError::HttpStatus { status: 404, .. }) => {
                    return Ok(None);
                }
                acquired => acquired?,
            };
            Ok(Some((
                candidate_from_acquired(&skill_id, &acquired, source_authority),
                Some(acquired),
            )))
        }
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => {
            let Some(resolution) = resolve_registry_skill(
                &FileRegistryStore::new(registry_path),
                subject,
                RegistryResolveOptions {
                    version: version.map(ToOwned::to_owned),
                    registry_url,
                },
            )?
            else {
                return Ok(None);
            };
            Ok(Some((
                candidate_from_resolution(subject, resolution, source_authority),
                None,
            )))
        }
    }
}

/// Adds connector `name` at `version`, or its current version, from the
/// hosted registry at `registry_url` to the project's connectors; none when
/// the registry publishes no connector under that name and version.
///
/// # Errors
///
/// Returns an error when the registry cannot be read, or what it serves is
/// not the connector asked for or does not verify.
pub fn install_connector(
    name: &ConnectorName,
    version: Option<&str>,
    expected_content_digest: Option<&str>,
    registry_url: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Option<InstalledRegistryConnector>, ResolveError> {
    let source_authority = RegistryTarget::Remote {
        registry_url: registry_url.to_owned(),
    }
    .manifest_source_authority();
    let client = registry_client(registry_url, env)?;
    let version = match version {
        Some(version) => version.to_owned(),
        None => match client.read_connector(name, None)? {
            Some(detail) => detail.version,
            None => return Ok(None),
        },
    };
    let Some(published) = client.connector_package(name, &version)? else {
        return Ok(None);
    };
    if &published.name != name || published.version != version {
        return Err(ResolveError::Failed(format!(
            "the registry served {}@{} for {name}@{version}",
            published.name, published.version
        )));
    }
    install_registry_connector(
        &published,
        &InstallRegistryConnectorOptions {
            connectors_root: crate::RunxPaths::resolve(env, cwd).connectors(),
            install_records: crate::RunxPaths::resolve(env, cwd).install_records(),
            expected_content_digest: expected_content_digest.map(ToOwned::to_owned),
            trusted_manifest_keys: trusted_manifest_keys_for_source(env, source_authority.clone())?,
            manifest_source_authority: source_authority,
        },
    )
    .map(Some)
    .map_err(|error| ResolveError::Failed(format!("registry install: {error}")))
}

/// The canonical `<owner>/<name>` root name `subject` addresses in the
/// registry `registry` targets; none where no skill holds it, or the registry
/// is a directory, which holds no root names.
///
/// # Errors
///
/// Returns an error when the registry cannot be reached or read.
pub fn resolve_root_name(
    subject: &str,
    version: Option<&str>,
    registry: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Option<String>, ResolveError> {
    let RegistryTarget::Remote { registry_url } = resolve_registry_target(registry, env, cwd)
    else {
        return Ok(None);
    };
    Ok(registry_client(&registry_url, env)?
        .read_root(subject, version)?
        .map(|detail| detail.skill_id))
}

/// A client of the hosted registry at `registry_url`.
///
/// # Errors
///
/// Returns an error when the transport cannot be built.
pub fn registry_client(
    registry_url: &str,
    env: &BTreeMap<String, String>,
) -> Result<RegistryClient, ResolveError> {
    let transport = crate::hosted_api_transport(crate::hosted_private_network_allowed(false, env))
        .map_err(|error| ResolveError::Failed(error.to_string()))?;
    RegistryClient::with_transport(registry_url, transport).map_err(ResolveError::from)
}

/// The manifest keys a package from `source_authority` must be signed by.
///
/// # Errors
///
/// Returns an error when the environment names a malformed trust key.
pub fn trusted_manifest_keys_for_source(
    env: &BTreeMap<String, String>,
    source_authority: RegistryManifestSourceAuthority,
) -> Result<Vec<TrustedRegistryManifestKey>, ResolveError> {
    super::trusted_registry_manifest_keys_from_env_with_source(env, Some(source_authority))
        .map_err(ResolveError::from)
}

fn installation_id(env: &BTreeMap<String, String>, cwd: &Path) -> Result<String, ResolveError> {
    if let Some(installation_id) = env
        .get("RUNX_INSTALLATION_ID")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(installation_id.to_owned());
    }
    crate::installation_id(&crate::RunxPaths::resolve(env, cwd))
        .map_err(|error| ResolveError::Usage(error.to_string()))
}

fn candidate_from_resolution(
    registry_ref: &str,
    resolution: RegistrySkillResolution,
    source_authority: RegistryManifestSourceAuthority,
) -> InstallCandidate {
    InstallCandidate {
        markdown: resolution.markdown,
        profile_document: resolution.profile_document,
        package_files: resolution.package_files,
        package_digest: resolution.package_digest,
        source: resolution.source,
        source_label: resolution.source_label,
        r#ref: registry_ref.to_owned(),
        skill_id: Some(resolution.skill_id),
        version: Some(resolution.version),
        signed_manifest: resolution.signed_manifest,
        profile_digest: resolution.profile_digest,
        runner_names: resolution.runner_names,
        trust_tier: Some(resolution.trust_tier),
        manifest_source_authority: Some(source_authority),
    }
}

fn candidate_from_acquired(
    registry_ref: &str,
    acquired: &AcquiredRegistrySkill,
    source_authority: RegistryManifestSourceAuthority,
) -> InstallCandidate {
    InstallCandidate {
        markdown: acquired.markdown.clone(),
        profile_document: acquired.profile_document.clone(),
        package_files: acquired.package_files.clone(),
        package_digest: acquired.package_digest.clone(),
        source: "runx-registry".to_owned(),
        source_label: "runx registry".to_owned(),
        r#ref: registry_ref.to_owned(),
        skill_id: Some(acquired.skill_id.clone()),
        version: Some(acquired.version.clone()),
        signed_manifest: acquired.signed_manifest.clone(),
        profile_digest: acquired.profile_digest.clone(),
        runner_names: acquired.runner_names.clone(),
        trust_tier: Some(acquired.trust_tier),
        manifest_source_authority: Some(source_authority),
    }
}
