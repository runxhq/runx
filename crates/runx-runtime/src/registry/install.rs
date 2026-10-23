// Module rationale: local registry installs keep digest
// validation, binding checks, conflict planning, and atomic writes in one
// transaction module.
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::sha256_prefixed;
use runx_parser::{SkillInstallOrigin, ValidatedSkillInstall, validate_skill_install};

use super::install_record::{InstallRecord, write_install_record};
use super::package_files::{
    registry_bundle_digest, skill_content_digest, validate_registry_package_file_path,
    validate_registry_skill_package,
};
use super::refs::safe_skill_package_parts;
use super::source_authority::RegistryManifestSourceAuthority;
use super::trust_anchor::{
    RegistryManifestVerificationFailure, TrustedRegistryManifestKey,
    default_trusted_registry_manifest_keys, registry_manifest_key_allows,
    verify_registry_signed_manifest,
};
use super::types::{
    RegistryPackageFile, RegistrySignedManifest, RegistrySubject, RegistrySubjectKind, TrustTier,
    UnreadVersion,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallCandidate {
    pub markdown: String,
    pub profile_document: Option<String>,
    pub package_files: Vec<RegistryPackageFile>,
    pub bundle_digest: Option<String>,
    pub source: String,
    pub source_label: String,
    /// `<publisher>/<name>@<version>`, as the candidate was named.
    pub package: String,
    pub skill_id: Option<String>,
    pub version: Option<String>,
    pub signed_manifest: Option<RegistrySignedManifest>,
    pub profile_digest: Option<String>,
    pub runner_names: Vec<String>,
    pub trust_tier: Option<TrustTier>,
    pub manifest_source_authority: Option<RegistryManifestSourceAuthority>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallLocalSkillOptions {
    pub destination_root: PathBuf,
    /// The project's added connectors: every connector the installed package
    /// carries is added there, so the project can bind its instances.
    pub connectors_root: PathBuf,
    /// Where the project records each package a registry added
    /// (`RunxPaths::install_records()`).
    pub install_records: PathBuf,
    pub expected_digest: Option<String>,
    pub trusted_manifest_keys: Vec<TrustedRegistryManifestKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    Installed,
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct InstallLocalSkillResult {
    pub status: InstallStatus,
    pub destination: PathBuf,
    pub skill_name: String,
    pub source: String,
    pub source_label: String,
    pub skill_id: Option<String>,
    pub version: Option<String>,
    pub digest: String,
    pub profile_digest: Option<String>,
    pub runner_names: Vec<String>,
    pub trust_tier: Option<TrustTier>,
    /// The newest published version, when this runx does not read it and
    /// installed this one in its place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_unread: Option<UnreadVersion>,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The package is intact, but this runx does not read its documents or a
    /// connector it carries: the version is another runx's to run.
    #[error("runx {} does not read {ref_name}: {reason}", crate::RUNX_VERSION)]
    Unreadable { ref_name: String, reason: String },
    #[error("digest mismatch for {ref_name}: expected {expected}, received {actual}")]
    DigestMismatch {
        ref_name: String,
        expected: String,
        actual: String,
    },
    #[error("registry signed manifest is required for {0}")]
    UnsignedManifest(String),
    #[error("registry signed manifest for {ref_name} was signed by unknown key id '{key_id}'")]
    UnknownManifestKey { ref_name: String, key_id: String },
    #[error("registry signed manifest signature is invalid for {ref_name}: {reason}")]
    InvalidManifestSignature { ref_name: String, reason: String },
    #[error(
        "registry signed manifest identity mismatch for {ref_name}: expected {expected}, manifest declares {actual}"
    )]
    ManifestIdentityMismatch {
        ref_name: String,
        expected: String,
        actual: String,
    },
    #[error("registry signed manifest identity for {ref_name} is missing {field}")]
    ManifestIdentityMissing {
        ref_name: String,
        field: &'static str,
    },
    #[error("registry signed manifest trust tier is required for {0}")]
    ManifestTrustTierMissing(String),
    #[error("registry signed manifest signer is out of scope for {ref_name}: {reason}")]
    ManifestTrustScopeViolation { ref_name: String, reason: String },
    #[error("binding digest mismatch for {ref_name}: expected {expected}, received {actual}")]
    ProfileDigestMismatch {
        ref_name: String,
        expected: String,
        actual: String,
    },
    #[error("package digest mismatch for {ref_name}: expected {expected}, received {actual}")]
    BundleDigestMismatch {
        ref_name: String,
        expected: String,
        actual: String,
    },
    #[error("registry package file is invalid for {ref_name}: {reason}")]
    InvalidPackageFile { ref_name: String, reason: String },
    #[error("runner manifest skill '{manifest_skill}' does not match skill '{skill_name}'")]
    ManifestSkillMismatch {
        manifest_skill: String,
        skill_name: String,
    },
    #[error("runner manifest runners do not match advertised runner metadata for skill '{0}'")]
    RunnerMetadataMismatch(String),
    #[error("skill install destination already exists with different content: {0}")]
    ConflictingSkill(PathBuf),
    #[error("skill install runner manifest already exists with different content: {0}")]
    ConflictingRunnerManifest(PathBuf),
    #[error("skill install package file already exists with different content: {0}")]
    ConflictingPackageFile(PathBuf),
    #[error("a connector the skill package carries could not be added to the project: {0}")]
    CarriedConnector(String),
    #[error("recording the install: {0}")]
    Record(String),
    #[error("io error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

pub fn install_local_skill(
    candidate: &InstallCandidate,
    options: &InstallLocalSkillOptions,
) -> Result<InstallLocalSkillResult, InstallError> {
    install_skill(
        candidate,
        options,
        allows_unsigned_local_registry_candidate(candidate),
        candidate.trust_tier,
    )
}

// Bundled bytes are authenticated by the installed runtime artifact. They use
// the same package validation and file transaction as acquired packages, but
// do not require a registry signature or contact a registry.
pub(super) fn install_bundled_skill(
    candidate: &InstallCandidate,
    options: &InstallLocalSkillOptions,
) -> Result<InstallLocalSkillResult, InstallError> {
    install_skill(candidate, options, true, Some(TrustTier::FirstParty))
}

/// Install `candidate`, recording it as a registry's package of `trust_tier`
/// when it has one.
fn install_skill(
    candidate: &InstallCandidate,
    options: &InstallLocalSkillOptions,
    allow_unsigned: bool,
    trust_tier: Option<TrustTier>,
) -> Result<InstallLocalSkillResult, InstallError> {
    let validated = validate_install_candidate(candidate, options, allow_unsigned)?;
    let paths = install_paths(candidate, options, &validated.install.skill.name);
    let status = place_package(
        &paths,
        &PackageDocuments {
            markdown: &validated.install.markdown,
            profile_document: candidate.profile_document.as_deref(),
            package_files: &candidate.package_files,
            ref_name: &exact_package(candidate),
        },
    )?;
    let record = trust_tier.map(|trust_tier| InstallRecord {
        package: exact_package(candidate),
        trust_tier,
    });
    if let Some(record) = &record {
        write_install_record(
            &options.install_records,
            &options.destination_root,
            &paths.package_root,
            record,
        )
        .map_err(InstallError::Record)?;
    }
    add_carried_connectors(
        &paths.package_root,
        &options.connectors_root,
        record
            .as_ref()
            .map(|record| (&options.install_records, record.trust_tier)),
    )?;

    Ok(InstallLocalSkillResult {
        status,
        destination: paths.destination,
        skill_name: validated.install.skill.name,
        source: validated.install.origin.source,
        source_label: validated.install.origin.source_label,
        skill_id: validated.install.origin.skill_id,
        version: validated.install.origin.version,
        digest: validated.actual_digest,
        profile_digest: validated.profile_digest,
        runner_names: validated.runner_names,
        trust_tier: candidate.trust_tier,
        newest_unread: None,
    })
}

/// `<skill>@<version>` for a registry's package, the ref it came by otherwise.
fn exact_package(candidate: &InstallCandidate) -> String {
    match (&candidate.skill_id, &candidate.version) {
        (Some(skill_id), Some(version)) => format!("{skill_id}@{version}"),
        _ => candidate.package.clone(),
    }
}

/// Adds every connector the installed package at `package_root` carries to
/// `connectors_root`, the project's added connectors, as the version it publishes: the package's
/// own copy, checked against the digests its bundle binds when it is loaded.
/// A registry's package records each one at its own tier.
fn add_carried_connectors(
    package_root: &Path,
    connectors_root: &Path,
    record: Option<(&PathBuf, TrustTier)>,
) -> Result<(), InstallError> {
    let invalid = |message: String| InstallError::CarriedConnector(message);
    let scope = crate::kernel::connectors::ConnectorScope::at(package_root)
        .map_err(|error| invalid(error.to_string()))?;
    if !scope.is_bundle() {
        return Ok(());
    }
    for connector in scope
        .connectors()
        .map_err(|error| invalid(error.to_string()))?
    {
        let source = scope.connector_directory(&connector);
        let directory = crate::registry::write_connector_version(
            connectors_root,
            &connector,
            source.as_deref(),
        )
        .map_err(invalid)?;
        if let Some((records, trust_tier)) = record {
            let version = directory
                .file_name()
                .map(|version| version.to_string_lossy().into_owned())
                .unwrap_or_default();
            write_install_record(
                records,
                connectors_root,
                &directory,
                &InstallRecord {
                    package: format!("{}@{version}", connector.name),
                    trust_tier,
                },
            )
            .map_err(InstallError::Record)?;
        }
    }
    Ok(())
}

struct ValidatedLocalInstall {
    actual_digest: String,
    profile_digest: Option<String>,
    runner_names: Vec<String>,
    install: ValidatedSkillInstall,
}

struct InstallPaths {
    package_root: PathBuf,
    destination: PathBuf,
    runner_manifest_path: Option<PathBuf>,
}

struct InstallWritePlan {
    writes_skill: bool,
    writes_runner_manifest: bool,
    package_files: Vec<PackageFileWritePlan>,
}

struct PackageFileWritePlan {
    path: PathBuf,
    content: String,
    write: bool,
}

impl InstallWritePlan {
    fn status(&self) -> InstallStatus {
        if self.writes_skill
            || self.writes_runner_manifest
            || self.package_files.iter().any(|file| file.write)
        {
            InstallStatus::Installed
        } else {
            InstallStatus::Unchanged
        }
    }
}

/// The documents a validated candidate installs.
#[derive(Clone, Copy)]
struct PackageDocuments<'a> {
    markdown: &'a str,
    profile_document: Option<&'a str>,
    package_files: &'a [RegistryPackageFile],
    ref_name: &'a str,
}

/// Puts the package at `paths`. A new version is written beside its
/// destination and moved into place only once every connector it carries
/// reads, so a version this runx does not read leaves nothing behind. An
/// existing version must already hold these files; any it lacks are written.
fn place_package(
    paths: &InstallPaths,
    documents: &PackageDocuments<'_>,
) -> Result<InstallStatus, InstallError> {
    if !paths.package_root.exists() {
        // Every path is checked, and a conflicting copy found, before a write.
        prepare_install_write_plan(paths, documents)?;
        if stage_new_version(paths, documents)? {
            return Ok(InstallStatus::Installed);
        }
    }
    let plan = prepare_install_write_plan(paths, documents)?;
    commit_install_write_plan(paths, &plan, documents)?;
    read_carried_connectors(documents.ref_name, &paths.package_root)?;
    Ok(plan.status())
}

/// Writes a new version into a staging directory beside its destination and
/// renames it into place once it reads; false when another install placed
/// the version first.
fn stage_new_version(
    paths: &InstallPaths,
    documents: &PackageDocuments<'_>,
) -> Result<bool, InstallError> {
    let Some(versions) = paths.package_root.parent() else {
        return Err(InstallError::Io {
            path: paths.package_root.clone(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "package root has no parent"),
        });
    };
    fs::create_dir_all(versions).map_err(|source| InstallError::Io {
        path: versions.to_path_buf(),
        source,
    })?;
    let root = versions.join(format!(".staging-{}", unique_suffix()));
    let staged = InstallPaths {
        destination: root.join("SKILL.md"),
        runner_manifest_path: paths
            .runner_manifest_path
            .as_ref()
            .map(|_| root.join(runx_parser::PROFILE_FILE)),
        package_root: root,
    };
    let placed = prepare_install_write_plan(&staged, documents)
        .and_then(|plan| commit_install_write_plan(&staged, &plan, documents))
        .and_then(|()| read_carried_connectors(documents.ref_name, &staged.package_root))
        .and_then(
            |()| match fs::rename(&staged.package_root, &paths.package_root) {
                Ok(()) => Ok(true),
                Err(_) if paths.package_root.exists() => Ok(false),
                Err(source) => Err(InstallError::Io {
                    path: paths.package_root.clone(),
                    source,
                }),
            },
        );
    if staged.package_root.exists() {
        let _ignored = fs::remove_dir_all(&staged.package_root);
    }
    placed
}

/// Reads every connector the package at `root` carries, as a run of it will.
fn read_carried_connectors(ref_name: &str, root: &Path) -> Result<(), InstallError> {
    crate::kernel::connectors::ConnectorScope::at(root)
        .map(|_| ())
        .map_err(|error| InstallError::Unreadable {
            ref_name: ref_name.to_owned(),
            reason: match error {
                crate::kernel::connectors::ConnectorRegistryError::InvalidManifest {
                    path,
                    message,
                } => {
                    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
                    format!(
                        "{}: {message}",
                        path.strip_prefix(&root).unwrap_or(&path).display()
                    )
                }
                other => other.to_string(),
            },
        })
}

fn validate_install_candidate(
    candidate: &InstallCandidate,
    options: &InstallLocalSkillOptions,
    allow_unsigned_local: bool,
) -> Result<ValidatedLocalInstall, InstallError> {
    let actual_digest = verify_signed_manifest_anchor(candidate, options, allow_unsigned_local)?;
    let profile_digest = validate_candidate_profile_digest(candidate, allow_unsigned_local)?;
    validate_candidate_bundle_digest(candidate)?;
    // The bytes are the ones published; what remains is whether this runx
    // reads them.
    let unreadable = |reason: String| InstallError::Unreadable {
        ref_name: exact_package(candidate),
        reason,
    };
    let origin = install_origin(candidate, &actual_digest, profile_digest.as_deref());
    let install = validate_skill_install(&candidate.markdown, origin)
        .map_err(|error| unreadable(error.to_string()))?;
    let package = validate_registry_skill_package(
        &candidate.markdown,
        candidate.profile_document.as_deref(),
        &candidate.package_files,
    )
    .map_err(|error| unreadable(error.to_string()))?;
    let runner_names = validate_install_binding_manifest(
        &install.skill.name,
        package.root_manifest(),
        &candidate.runner_names,
    )?;
    Ok(ValidatedLocalInstall {
        actual_digest,
        profile_digest,
        runner_names,
        install,
    })
}

/// Checks the candidate's content against the registry's signed envelope (or,
/// for an unsigned local registry, against nothing but an expected digest)
/// and returns its manual digest for the install origin. The envelope signs
/// one digest over every file, so the manual, profile and package files are
/// all bound by it.
fn verify_signed_manifest_anchor(
    candidate: &InstallCandidate,
    options: &InstallLocalSkillOptions,
    allow_unsigned_local: bool,
) -> Result<String, InstallError> {
    let content_digest = skill_content_digest(
        &candidate.markdown,
        candidate.profile_document.as_deref(),
        &candidate.package_files,
    )
    .map_err(|error| InstallError::InvalidPackageFile {
        ref_name: candidate.package.clone(),
        reason: error.to_string(),
    })?;
    if let Some(expected) = options
        .expected_digest
        .as_ref()
        .filter(|expected| !digest_matches(expected, &content_digest))
    {
        return Err(InstallError::DigestMismatch {
            ref_name: candidate.package.clone(),
            expected: expected.clone(),
            actual: content_digest,
        });
    }
    let actual_digest = sha256_prefixed(candidate.markdown.as_bytes());
    let Some(manifest) = candidate.signed_manifest.as_ref() else {
        if allow_unsigned_local {
            return Ok(actual_digest);
        }
        return Err(InstallError::UnsignedManifest(candidate.package.clone()));
    };
    let trusted_keys = trusted_manifest_keys(options)?;
    let key = verify_registry_signed_manifest(manifest, &trusted_keys).map_err(|failure| {
        manifest_verification_error(candidate.package.clone(), &manifest.signer.key_id, failure)
    })?;
    validate_manifest_identity(candidate, manifest)?;
    validate_manifest_trust_scope(candidate, key)?;
    if manifest.content_digest != content_digest {
        return Err(InstallError::DigestMismatch {
            ref_name: candidate.package.clone(),
            expected: manifest.content_digest.clone(),
            actual: content_digest,
        });
    }
    Ok(actual_digest)
}

fn allows_unsigned_local_registry_candidate(candidate: &InstallCandidate) -> bool {
    matches!(
        candidate.manifest_source_authority.as_ref(),
        Some(RegistryManifestSourceAuthority::RegistrySource(source))
            if source.starts_with("local:") || source.starts_with("file:")
    )
}

fn validate_manifest_trust_scope(
    candidate: &InstallCandidate,
    key: &TrustedRegistryManifestKey,
) -> Result<(), InstallError> {
    let skill_id = candidate
        .skill_id
        .as_deref()
        .ok_or(InstallError::ManifestIdentityMissing {
            ref_name: candidate.package.clone(),
            field: "skill_id",
        })?;
    let trust_tier = candidate
        .trust_tier
        .as_ref()
        .ok_or_else(|| InstallError::ManifestTrustTierMissing(candidate.package.clone()))?;
    registry_manifest_key_allows(
        key,
        super::types::RegistrySubjectKind::Skill,
        skill_id,
        trust_tier,
        candidate.manifest_source_authority.as_ref(),
    )
    .map_err(|reason| InstallError::ManifestTrustScopeViolation {
        ref_name: candidate.package.clone(),
        reason,
    })
}

fn trusted_manifest_keys(
    options: &InstallLocalSkillOptions,
) -> Result<Vec<TrustedRegistryManifestKey>, InstallError> {
    if options.trusted_manifest_keys.is_empty() {
        return default_trusted_registry_manifest_keys().map_err(|_error| {
            InstallError::InvalidManifestSignature {
                ref_name: "default registry trust anchor".to_owned(),
                reason: "malformed trusted key".to_owned(),
            }
        });
    }
    Ok(options.trusted_manifest_keys.clone())
}

struct DigestMismatch {
    expected: String,
    actual: String,
}

fn check_digest_link(
    actual: Option<&str>,
    expected: Option<&str>,
    expected_label_when_missing: &str,
) -> Result<(), DigestMismatch> {
    match (actual, expected) {
        (Some(actual), Some(expected)) if digest_matches(expected, actual) => Ok(()),
        (Some(actual), Some(expected)) => Err(DigestMismatch {
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        }),
        (Some(actual), None) => Err(DigestMismatch {
            expected: expected_label_when_missing.to_owned(),
            actual: actual.to_owned(),
        }),
        (None, Some(expected)) => Err(DigestMismatch {
            expected: expected.to_owned(),
            actual: "none".to_owned(),
        }),
        (None, None) => Ok(()),
    }
}

fn validate_candidate_profile_digest(
    candidate: &InstallCandidate,
    allow_unsigned_local: bool,
) -> Result<Option<String>, InstallError> {
    let profile_digest = candidate
        .profile_document
        .as_ref()
        .map(|document| sha256_prefixed(document.as_bytes()));
    // A signed candidate's profile is bound by the envelope's content digest;
    // an unsigned local record states its own profile digest.
    if candidate.signed_manifest.is_some() || !allow_unsigned_local {
        return Ok(profile_digest);
    }
    let expected_profile_digest = candidate.profile_digest.as_ref();
    check_digest_link(
        profile_digest.as_deref(),
        expected_profile_digest.map(String::as_str),
        "local registry profile digest",
    )
    .map_err(|mismatch| InstallError::ProfileDigestMismatch {
        ref_name: candidate.package.clone(),
        expected: mismatch.expected,
        actual: mismatch.actual,
    })?;
    Ok(profile_digest)
}

/// The package digest the registry record states must be its files' digest.
/// The signed envelope binds the files themselves through its content digest.
fn validate_candidate_bundle_digest(candidate: &InstallCandidate) -> Result<(), InstallError> {
    let actual = registry_bundle_digest(&candidate.package_files);
    check_digest_link(
        actual.as_deref(),
        candidate.bundle_digest.as_deref(),
        "registry bundle digest",
    )
    .map_err(|mismatch| InstallError::BundleDigestMismatch {
        ref_name: candidate.package.clone(),
        expected: mismatch.expected,
        actual: mismatch.actual,
    })
}

fn validate_manifest_identity(
    candidate: &InstallCandidate,
    manifest: &RegistrySignedManifest,
) -> Result<(), InstallError> {
    let Some(skill_id) = &candidate.skill_id else {
        return Err(InstallError::ManifestIdentityMissing {
            ref_name: candidate.package.clone(),
            field: "skill_id",
        });
    };
    let Some(version) = &candidate.version else {
        return Err(InstallError::ManifestIdentityMissing {
            ref_name: candidate.package.clone(),
            field: "version",
        });
    };
    let subject = RegistrySubject {
        kind: RegistrySubjectKind::Skill,
        name: skill_id.clone(),
    };
    if !manifest.signs(&subject, version) {
        return Err(InstallError::ManifestIdentityMismatch {
            ref_name: candidate.package.clone(),
            expected: format!("skill {skill_id}@{version}"),
            actual: format!(
                "{} {}@{}",
                manifest.subject.kind.as_str(),
                manifest.subject.name,
                manifest.version
            ),
        });
    }
    Ok(())
}

fn manifest_verification_error(
    ref_name: String,
    key_id: &str,
    failure: RegistryManifestVerificationFailure,
) -> InstallError {
    match failure {
        RegistryManifestVerificationFailure::UnknownKey => InstallError::UnknownManifestKey {
            ref_name,
            key_id: key_id.to_owned(),
        },
        RegistryManifestVerificationFailure::UnsupportedSchema => {
            InstallError::InvalidManifestSignature {
                ref_name,
                reason: "unsupported schema".to_owned(),
            }
        }
        RegistryManifestVerificationFailure::UnsupportedAlgorithm => {
            InstallError::InvalidManifestSignature {
                ref_name,
                reason: "unsupported algorithm".to_owned(),
            }
        }
        RegistryManifestVerificationFailure::MalformedPayload => {
            InstallError::InvalidManifestSignature {
                ref_name,
                reason: "malformed payload".to_owned(),
            }
        }
        RegistryManifestVerificationFailure::MalformedKey => {
            InstallError::InvalidManifestSignature {
                ref_name,
                reason: "malformed key".to_owned(),
            }
        }
        RegistryManifestVerificationFailure::MalformedSignature => {
            InstallError::InvalidManifestSignature {
                ref_name,
                reason: "malformed signature".to_owned(),
            }
        }
        RegistryManifestVerificationFailure::SignatureMismatch => {
            InstallError::InvalidManifestSignature {
                ref_name,
                reason: "signature mismatch".to_owned(),
            }
        }
    }
}

fn install_origin(
    candidate: &InstallCandidate,
    actual_digest: &str,
    profile_digest: Option<&str>,
) -> SkillInstallOrigin {
    SkillInstallOrigin {
        source: candidate.source.clone(),
        source_label: candidate.source_label.clone(),
        package: candidate.package.clone(),
        skill_id: candidate.skill_id.clone(),
        version: candidate.version.clone(),
        digest: Some(actual_digest.to_owned()),
        profile_digest: profile_digest.map(ToOwned::to_owned),
        runner_names: Some(candidate.runner_names.clone()),
        trust_tier: candidate.trust_tier.as_ref().map(trust_tier_string),
    }
}

fn install_paths(
    candidate: &InstallCandidate,
    options: &InstallLocalSkillOptions,
    skill_name: &str,
) -> InstallPaths {
    let package_parts =
        safe_skill_package_parts(&candidate.package, skill_name, candidate.version.as_deref());
    let package_root = package_parts
        .iter()
        .fold(options.destination_root.clone(), |path, part| {
            path.join(part)
        });
    let destination = package_root.join("SKILL.md");
    let runner_manifest_path = candidate
        .profile_document
        .as_ref()
        .map(|_| package_root.join(runx_parser::PROFILE_FILE));
    InstallPaths {
        package_root,
        destination,
        runner_manifest_path,
    }
}

// Function rationale: install planning compares all destination
// files before writing so package, profile, and lock updates remain atomic.
fn prepare_install_write_plan(
    paths: &InstallPaths,
    documents: &PackageDocuments<'_>,
) -> Result<InstallWritePlan, InstallError> {
    let PackageDocuments {
        markdown,
        profile_document,
        package_files,
        ref_name,
    } = *documents;
    let existing = read_optional(&paths.destination)?;
    let existing_runner_manifest = match &paths.runner_manifest_path {
        Some(path) => read_optional(path)?,
        None => None,
    };
    if let Some(existing) = &existing
        && sha256_prefixed(existing.as_bytes()) != sha256_prefixed(markdown.as_bytes())
    {
        return Err(InstallError::ConflictingSkill(paths.destination.clone()));
    }
    if let (Some(path), Some(existing), Some(next)) = (
        &paths.runner_manifest_path,
        &existing_runner_manifest,
        profile_document,
    ) && existing != next
    {
        return Err(InstallError::ConflictingRunnerManifest(path.clone()));
    }
    let mut seen_package_paths = BTreeSet::new();
    let mut package_file_plans = Vec::with_capacity(package_files.len());
    for file in package_files {
        validate_registry_package_file_path(&file.path).map_err(|reason| {
            InstallError::InvalidPackageFile {
                ref_name: ref_name.to_owned(),
                reason,
            }
        })?;
        if !seen_package_paths.insert(file.path.clone()) {
            return Err(InstallError::InvalidPackageFile {
                ref_name: ref_name.to_owned(),
                reason: format!("duplicate package file '{}'", file.path),
            });
        }
        let path = paths.package_root.join(&file.path);
        let existing = read_optional(&path)?;
        if let Some(existing) = &existing
            && existing != &file.content
        {
            return Err(InstallError::ConflictingPackageFile(path));
        }
        package_file_plans.push(PackageFileWritePlan {
            path,
            content: file.content.clone(),
            write: existing.is_none(),
        });
    }
    Ok(InstallWritePlan {
        writes_skill: existing.is_none(),
        writes_runner_manifest: paths.runner_manifest_path.is_some()
            && existing_runner_manifest.is_none(),
        package_files: package_file_plans,
    })
}

fn commit_install_write_plan(
    paths: &InstallPaths,
    write_plan: &InstallWritePlan,
    documents: &PackageDocuments<'_>,
) -> Result<(), InstallError> {
    let PackageDocuments {
        markdown,
        profile_document,
        package_files,
        ..
    } = *documents;
    debug_assert_eq!(write_plan.package_files.len(), package_files.len());
    fs::create_dir_all(&paths.package_root).map_err(|source| InstallError::Io {
        path: paths.package_root.clone(),
        source,
    })?;
    if write_plan.writes_skill {
        write_atomic(&paths.destination, markdown)?;
    }
    if let (Some(path), true, Some(document)) = (
        &paths.runner_manifest_path,
        write_plan.writes_runner_manifest,
        profile_document,
    ) {
        write_atomic(path, document)?;
    }
    for file in &write_plan.package_files {
        if !file.write {
            continue;
        }
        if let Some(parent) = file.path.parent() {
            fs::create_dir_all(parent).map_err(|source| InstallError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_atomic(&file.path, &file.content)?;
    }
    Ok(())
}

fn validate_install_binding_manifest(
    skill_name: &str,
    manifest: Option<&runx_parser::SkillRunnerManifest>,
    advertised_runner_names: &[String],
) -> Result<Vec<String>, InstallError> {
    let Some(manifest) = manifest else {
        return Ok(advertised_runner_names.to_vec());
    };
    if manifest.skill != skill_name {
        return Err(InstallError::ManifestSkillMismatch {
            manifest_skill: manifest.skill.clone(),
            skill_name: skill_name.to_owned(),
        });
    }
    let runner_names = manifest.runners.keys().cloned().collect::<Vec<_>>();
    if !advertised_runner_names.is_empty() && advertised_runner_names != runner_names {
        return Err(InstallError::RunnerMetadataMismatch(skill_name.to_owned()));
    }
    Ok(runner_names)
}

fn write_atomic(destination: &Path, contents: &str) -> Result<(), InstallError> {
    let temp_path = destination.with_extension(format!("tmp-{}", unique_suffix()));
    fs::write(&temp_path, contents).map_err(|source| InstallError::Io {
        path: temp_path.clone(),
        source,
    })?;
    if destination.exists() {
        let _ = fs::remove_file(&temp_path);
        return Err(InstallError::Io {
            path: destination.to_path_buf(),
            source: io::Error::new(io::ErrorKind::AlreadyExists, "destination exists"),
        });
    }
    fs::rename(&temp_path, destination).map_err(|source| {
        let _ = fs::remove_file(&temp_path);
        InstallError::Io {
            path: destination.to_path_buf(),
            source,
        }
    })
}

fn read_optional(path: &Path) -> Result<Option<String>, InstallError> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn digest_matches(expected: &str, actual_prefixed: &str) -> bool {
    expected == actual_prefixed
        || actual_prefixed
            .strip_prefix("sha256:")
            .is_some_and(|actual_hex| expected == actual_hex)
}

fn trust_tier_string(value: &TrustTier) -> String {
    match value {
        TrustTier::FirstParty => "first_party",
        TrustTier::Verified => "verified",
        TrustTier::Community => "community",
    }
    .to_owned()
}

pub(super) fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}
