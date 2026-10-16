//! Installs a published connector version into the project's connectors
//! directory, `connectors/<name>/<version>/`. The version's files
//! are the registry's exactly: its signed manifest must name this connector,
//! version and content digest under a trusted key, and the runtime's own
//! loader and packager must reproduce those files from the staged copy before
//! it is renamed into place. A version directory is written once and never
//! rewritten.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runx_contracts::connector::{ConnectorName, Digest};
use runx_contracts::{RegistryContentFile, is_registry_version, registry_content_digest};
use serde::{Deserialize, Serialize};

use super::install::{InstallStatus, unique_suffix};
use super::install_record::{InstallRecord, write_install_record};
use super::package_files::validate_registry_package_file_path;
use super::publish_package::connector_package;
use super::source_authority::RegistryManifestSourceAuthority;
use super::trust_anchor::{
    RegistryManifestVerificationFailure, TrustedRegistryManifestKey,
    default_trusted_registry_manifest_keys, registry_manifest_key_allows,
    verify_registry_signed_manifest,
};
use super::types::{
    RegistryPackageFile, RegistrySignedManifest, RegistrySubject, RegistrySubjectKind, TrustTier,
};
use crate::kernel::connectors::load_installed;

/// One published connector version as the registry serves it for install.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryConnectorVersion {
    #[serde(rename = "ref")]
    pub name: ConnectorName,
    pub version: String,
    pub trust_tier: TrustTier,
    pub files: Vec<RegistryPackageFile>,
    pub signed_manifest: RegistrySignedManifest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallRegistryConnectorOptions {
    /// The project's connectors directory, `RunxPaths::connectors()`.
    pub connectors_root: PathBuf,
    /// Where the project records each package a registry added
    /// (`RunxPaths::install_records()`).
    pub install_records: PathBuf,
    /// A content digest the caller pinned (`--digest`).
    pub expected_content_digest: Option<String>,
    /// Empty means the default registry trust anchor.
    pub trusted_manifest_keys: Vec<TrustedRegistryManifestKey>,
    pub manifest_source_authority: RegistryManifestSourceAuthority,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InstalledRegistryConnector {
    pub status: InstallStatus,
    #[serde(rename = "ref")]
    pub name: ConnectorName,
    pub version: String,
    pub content_digest: Digest,
    pub manifest_digest: Digest,
    pub connector_digest: Digest,
    pub trust_tier: TrustTier,
    pub destination: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectorInstallError {
    #[error("connector {name}: version {version:?} is not a registry version label")]
    InvalidVersion {
        name: ConnectorName,
        version: String,
    },
    #[error("connector {name}: {reason}")]
    InvalidPackage { name: ConnectorName, reason: String },
    #[error("connector {name}: content digest is {actual}, expected {expected}")]
    DigestMismatch {
        name: ConnectorName,
        expected: String,
        actual: String,
    },
    #[error("connector {name}: signed manifest key {key_id} is not trusted")]
    UnknownManifestKey { name: ConnectorName, key_id: String },
    #[error("connector {name}: signed manifest is invalid: {reason}")]
    InvalidManifestSignature {
        name: ConnectorName,
        reason: &'static str,
    },
    #[error("connector {name}: signed manifest names {actual}, expected {expected}")]
    ManifestIdentityMismatch {
        name: ConnectorName,
        expected: String,
        actual: String,
    },
    #[error("connector {name}: signer is out of scope: {reason}")]
    ManifestTrustScopeViolation { name: ConnectorName, reason: String },
    #[error("connector {name}: {} already holds other files for this version", .destination.display())]
    ConflictingVersion {
        name: ConnectorName,
        destination: PathBuf,
    },
    #[error("{}: {source}", .path.display())]
    Io { path: PathBuf, source: io::Error },
}

/// Writes `connector` into a project's `connectors_root` at
/// `<name>/<version>/`, once, packaged exactly as it publishes;
/// returns that directory. Its bytes come from `source` (the running release
/// when `None`): a runtime capability the release embeds, or a connector an
/// installed skill package carries. Either way they carry no registry
/// signature of their own; the release, or the skill package's verified
/// digests, authenticated them, and a reader checks the copy against the
/// digests it binds.
pub(crate) fn write_connector_version(
    connectors_root: &Path,
    connector: &crate::kernel::connectors::LoadedConnector,
    source: Option<&Path>,
) -> Result<PathBuf, String> {
    let package = connector_package(connector, source).map_err(|error| error.to_string())?;
    let versions = crate::kernel::connectors::name_directory(connectors_root, &package.name);
    let destination = versions.join(&package.version);
    if destination.exists() {
        return Ok(destination);
    }
    fs::create_dir_all(&versions).map_err(|error| format!("{}: {error}", versions.display()))?;
    let staged = versions.join(format!(".staging-{}", unique_suffix()));
    let written = package
        .files
        .iter()
        .try_for_each(|file| {
            validate_registry_package_file_path(&file.path)?;
            let path = staged.join(&file.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("{}: {error}", parent.display()))?;
            }
            fs::write(&path, file.content.as_bytes())
                .map_err(|error| format!("{}: {error}", path.display()))
        })
        .and_then(|()| match fs::rename(&staged, &destination) {
            Ok(()) => Ok(()),
            // Another run seeded this version first.
            Err(_) if destination.exists() => Ok(()),
            Err(error) => Err(format!("{}: {error}", destination.display())),
        });
    if staged.exists() {
        let _ignored = fs::remove_dir_all(&staged);
    }
    written.map(|()| destination)
}

/// Verifies `published` and writes it to
/// `<connectors_root>/<publisher>/<id>/<version>/`, or finds that exact
/// version already there.
///
/// # Errors
///
/// Returns an error when the version label, a file path, the content digest,
/// the signed manifest, the signer's scope or the loaded connector does not
/// match what was published, or when the version directory holds other files.
pub fn install_registry_connector(
    published: &RegistryConnectorVersion,
    options: &InstallRegistryConnectorOptions,
) -> Result<InstalledRegistryConnector, ConnectorInstallError> {
    let connector_ref = &published.name;
    if !is_registry_version(&published.version) {
        return Err(ConnectorInstallError::InvalidVersion {
            name: connector_ref.clone(),
            version: published.version.clone(),
        });
    }
    for file in &published.files {
        validate_registry_package_file_path(&file.path)
            .map_err(|reason| invalid_package(connector_ref, reason))?;
    }
    let content_digest = content_digest_of(connector_ref, &published.files)?;
    if let Some(expected) = &options.expected_content_digest
        && expected != content_digest.as_str()
    {
        return Err(ConnectorInstallError::DigestMismatch {
            name: connector_ref.clone(),
            expected: expected.clone(),
            actual: content_digest.into_string(),
        });
    }
    verify_signed_manifest(published, &content_digest, options)?;

    let versions =
        crate::kernel::connectors::name_directory(&options.connectors_root, connector_ref);
    let destination = versions.join(&published.version);
    if destination.exists() {
        return existing_version(published, &content_digest, destination)
            .and_then(|installed| recorded(installed, published, options));
    }
    fs::create_dir_all(&versions).map_err(|source| io_error(&versions, source))?;
    let staged = versions.join(format!(".staging-{}", unique_suffix()));
    let written = stage(published, &content_digest, &staged).and_then(|loaded| {
        match fs::rename(&staged, &destination) {
            Ok(()) => Ok(installed(
                published,
                content_digest.clone(),
                loaded,
                InstallStatus::Installed,
                &destination,
            )),
            // Another install placed this version first: it must be these files.
            Err(_) if destination.exists() => {
                existing_version(published, &content_digest, destination.clone())
            }
            Err(source) => Err(io_error(&destination, source)),
        }
    });
    if staged.exists() {
        let _ignored = fs::remove_dir_all(&staged);
    }
    written.and_then(|installed| recorded(installed, published, options))
}

/// `installed`, once the project records it as the registry's.
fn recorded(
    installed: InstalledRegistryConnector,
    published: &RegistryConnectorVersion,
    options: &InstallRegistryConnectorOptions,
) -> Result<InstalledRegistryConnector, ConnectorInstallError> {
    write_install_record(
        &options.install_records,
        &options.connectors_root,
        &installed.destination,
        &InstallRecord {
            reference: format!("{}@{}", published.name, published.version),
            trust_tier: published.trust_tier,
        },
    )
    .map_err(|reason| {
        invalid_package(&published.name, format!("recording the install: {reason}"))
    })?;
    Ok(installed)
}

/// The signed manifest must verify under a trusted key, name this connector
/// and version, come from a signer allowed for its publisher, and sign these
/// files' content digest.
fn verify_signed_manifest(
    published: &RegistryConnectorVersion,
    content_digest: &Digest,
    options: &InstallRegistryConnectorOptions,
) -> Result<(), ConnectorInstallError> {
    let connector_ref = &published.name;
    let manifest = &published.signed_manifest;
    let trusted_keys = if options.trusted_manifest_keys.is_empty() {
        default_trusted_registry_manifest_keys().map_err(|_| {
            ConnectorInstallError::InvalidManifestSignature {
                name: connector_ref.clone(),
                reason: "the default registry trust anchor is malformed",
            }
        })?
    } else {
        options.trusted_manifest_keys.clone()
    };
    let key = verify_registry_signed_manifest(manifest, &trusted_keys)
        .map_err(|failure| verification_error(connector_ref, &manifest.signer.key_id, failure))?;
    let subject = RegistrySubject {
        kind: RegistrySubjectKind::Connector,
        subject_ref: connector_ref.as_str().to_owned(),
    };
    if !manifest.signs(&subject, &published.version) {
        return Err(ConnectorInstallError::ManifestIdentityMismatch {
            name: connector_ref.clone(),
            expected: format!("connector {connector_ref}@{}", published.version),
            actual: format!(
                "{} {}@{}",
                manifest.subject.kind.as_str(),
                manifest.subject.subject_ref,
                manifest.version
            ),
        });
    }
    registry_manifest_key_allows(
        key,
        RegistrySubjectKind::Connector,
        connector_ref.as_str(),
        &published.trust_tier,
        Some(&options.manifest_source_authority),
    )
    .map_err(
        |reason| ConnectorInstallError::ManifestTrustScopeViolation {
            name: connector_ref.clone(),
            reason,
        },
    )?;
    if manifest.content_digest != content_digest.as_str() {
        return Err(ConnectorInstallError::DigestMismatch {
            name: connector_ref.clone(),
            expected: manifest.content_digest.clone(),
            actual: content_digest.as_str().to_owned(),
        });
    }
    Ok(())
}

/// Writes the files below `staged`, then loads and repackages them as the
/// runtime would: the connector must be this ref, and its package these exact
/// files.
fn stage(
    published: &RegistryConnectorVersion,
    content_digest: &Digest,
    staged: &Path,
) -> Result<Loaded, ConnectorInstallError> {
    for file in &published.files {
        let path = staged.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        fs::write(&path, &file.content).map_err(|source| io_error(&path, source))?;
    }
    load(published, content_digest, staged)
}

/// A version directory that already exists must hold exactly these files.
fn existing_version(
    published: &RegistryConnectorVersion,
    content_digest: &Digest,
    destination: PathBuf,
) -> Result<InstalledRegistryConnector, ConnectorInstallError> {
    match load(published, content_digest, &destination) {
        Ok(loaded) => Ok(installed(
            published,
            content_digest.clone(),
            loaded,
            InstallStatus::Unchanged,
            &destination,
        )),
        Err(
            ConnectorInstallError::InvalidPackage { .. }
            | ConnectorInstallError::DigestMismatch { .. },
        ) => Err(ConnectorInstallError::ConflictingVersion {
            name: published.name.clone(),
            destination,
        }),
        Err(error) => Err(error),
    }
}

struct Loaded {
    manifest_digest: Digest,
    connector_digest: Digest,
}

fn load(
    published: &RegistryConnectorVersion,
    content_digest: &Digest,
    directory: &Path,
) -> Result<Loaded, ConnectorInstallError> {
    let connector_ref = &published.name;
    // An installed version, not a source tree: a later runx/* version may
    // differ from the copy embedded in this binary.
    let connector = load_installed(directory)
        .map_err(|error| invalid_package(connector_ref, error.to_string()))?;
    if connector.name != *connector_ref {
        return Err(invalid_package(
            connector_ref,
            format!("the package loads as connector {}", connector.name),
        ));
    }
    let package = connector_package(&connector, Some(directory))
        .map_err(|error| invalid_package(connector_ref, error.to_string()))?;
    if package.content_digest != *content_digest {
        return Err(ConnectorInstallError::DigestMismatch {
            name: connector_ref.clone(),
            expected: content_digest.as_str().to_owned(),
            actual: package.content_digest.into_string(),
        });
    }
    Ok(Loaded {
        manifest_digest: connector.manifest_digest.clone(),
        connector_digest: connector.connector_digest.clone(),
    })
}

fn installed(
    published: &RegistryConnectorVersion,
    content_digest: Digest,
    loaded: Loaded,
    status: InstallStatus,
    destination: &Path,
) -> InstalledRegistryConnector {
    InstalledRegistryConnector {
        status,
        name: published.name.clone(),
        version: published.version.clone(),
        content_digest,
        manifest_digest: loaded.manifest_digest,
        connector_digest: loaded.connector_digest,
        trust_tier: published.trust_tier,
        destination: destination.to_path_buf(),
    }
}

fn content_digest_of(
    name: &ConnectorName,
    files: &[RegistryPackageFile],
) -> Result<Digest, ConnectorInstallError> {
    registry_content_digest(files.iter().map(|file| RegistryContentFile {
        path: &file.path,
        content: &file.content,
    }))
    .map_err(|error| invalid_package(name, error.to_string()))
    .and_then(|digest| {
        Digest::new(digest).ok_or_else(|| invalid_package(name, "no content digest"))
    })
}

fn verification_error(
    name: &ConnectorName,
    key_id: &str,
    failure: RegistryManifestVerificationFailure,
) -> ConnectorInstallError {
    let reason = match failure {
        RegistryManifestVerificationFailure::UnknownKey => {
            return ConnectorInstallError::UnknownManifestKey {
                name: name.clone(),
                key_id: key_id.to_owned(),
            };
        }
        RegistryManifestVerificationFailure::UnsupportedSchema => "unsupported schema",
        RegistryManifestVerificationFailure::UnsupportedAlgorithm => "unsupported algorithm",
        RegistryManifestVerificationFailure::MalformedPayload => "malformed terms",
        RegistryManifestVerificationFailure::MalformedKey => "malformed trusted key",
        RegistryManifestVerificationFailure::MalformedSignature => "malformed signature",
        RegistryManifestVerificationFailure::SignatureMismatch => "signature mismatch",
    };
    ConnectorInstallError::InvalidManifestSignature {
        name: name.clone(),
        reason,
    }
}

fn invalid_package(name: &ConnectorName, reason: impl Into<String>) -> ConnectorInstallError {
    ConnectorInstallError::InvalidPackage {
        name: name.clone(),
        reason: reason.into(),
    }
}

fn io_error(path: &Path, source: io::Error) -> ConnectorInstallError {
    ConnectorInstallError::Io {
        path: path.to_path_buf(),
        source,
    }
}
