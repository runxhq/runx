//! A publish binds the package's closure, including skills other publishers
//! own. Each registry ref the package names that its project does not hold
//! is read, as the registry signed it, from the registry the publish
//! targets, and installed, verified, into a project of this publish's own,
//! as `runx skill` installs the refs a run names; the package is inspected
//! again, a level at a time, until every ref it names is held. A version is
//! read by its record, never acquired, so a publish counts as no install.
//! A skill-build candidate's harness binds its closure the same way, into
//! the candidate's own project, from the registry its operator selects.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::http::{RegistryClient, RegistryClientError};
use super::install::{
    InstallCandidate, InstallError, InstallLocalSkillOptions, InstallLocalSkillResult,
    install_local_skill,
};
use super::publish_package::{
    RegistryPublishPackageError, RegistryPublishPackageRequest, unheld_publish_registry_refs,
};
use super::refs::parse_registry_ref;
use super::source_authority::{
    RegistryManifestSourceAuthority, registry_manifest_source_authority_from_registry_url,
};
use super::trust_anchor::{RegistryManifestTrustEnvError, trusted_registry_manifest_keys_from_env};
use super::types::RegistrySkillVersion;
use crate::http::RuntimeHttpTransport;

/// A closure deeper than this is a cycle or a fault, not a real graph.
const MAX_CLOSURE_DEPTH: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum PublishClosureError {
    #[error(transparent)]
    Package(#[from] RegistryPublishPackageError),
    #[error(transparent)]
    Client(#[from] RegistryClientError),
    #[error(transparent)]
    Trust(#[from] RegistryManifestTrustEnvError),
    #[error("installing {reference}: {source}")]
    Install {
        reference: String,
        source: InstallError,
    },
    #[error("{0}")]
    Unresolved(String),
    #[error("creating the publish's project: {0}")]
    Project(std::io::Error),
}

/// The project a publish binds its closure in, held for as long as the
/// publish runs.
pub struct PublishClosure {
    project: tempfile::TempDir,
    /// The environment the publish runs in: the publisher's, with this
    /// closure's project as its project.
    pub env: BTreeMap<String, String>,
}

impl PublishClosure {
    /// The project directory the closure's skills are installed in.
    #[must_use]
    pub fn project(&self) -> &Path {
        self.project.path()
    }
}

/// Install, verified, every registry ref the package names from `client`'s
/// registry, at `registry_url`, into a project of the publish's own.
///
/// # Errors
///
/// Returns an error when the package does not load, the registry cannot be
/// read or does not publish a ref, a version it serves does not verify
/// against its signed envelope, or the closure does not close.
pub fn materialize_publish_closure<T: RuntimeHttpTransport>(
    client: &RegistryClient<T>,
    registry_url: &str,
    request: RegistryPublishPackageRequest<'_>,
) -> Result<PublishClosure, PublishClosureError> {
    let project = tempfile::Builder::new()
        .prefix("runx-publish-closure-")
        .tempdir()
        .map_err(PublishClosureError::Project)?;
    let mut env = request.env.clone();
    env.insert(
        crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
        project.path().to_string_lossy().into_owned(),
    );
    install_registry_closure(
        client,
        registry_url,
        RegistryPublishPackageRequest {
            env: &env,
            ..request
        },
    )?;
    Ok(PublishClosure { project, env })
}

/// Install, verified, every registry ref the package names that the project
/// its environment names does not hold: each read by its record from
/// `client`'s registry, at `registry_url`, a level at a time until the
/// closure is held. A package naming none reads nothing.
///
/// # Errors
///
/// Returns an error when the package does not load, the registry cannot be
/// read or does not publish a ref, a version it serves does not verify
/// against its signed envelope, or the closure does not close.
pub fn install_registry_closure<T: RuntimeHttpTransport>(
    client: &RegistryClient<T>,
    registry_url: &str,
    request: RegistryPublishPackageRequest<'_>,
) -> Result<(), PublishClosureError> {
    let installer = RecordInstaller::new(registry_url, request.env, request.cwd)?;
    let mut supplied = BTreeSet::new();
    for _ in 0..=MAX_CLOSURE_DEPTH {
        let unheld = unheld_publish_registry_refs(request)?;
        if unheld.is_empty() {
            return Ok(());
        }
        for reference in unheld {
            // A package binds an exact closure, so its refs name the canonical
            // `<publisher>/<name>`; a root name may move and is never one.
            let resolved = parse_registry_ref(&reference);
            let version = match resolved.version {
                Some(version) => version,
                None => client
                    .read(&resolved.skill_id, None)?
                    .map(|detail| detail.version)
                    .ok_or_else(|| {
                        PublishClosureError::Unresolved(format!(
                            "registry ref '{reference}' names no published skill"
                        ))
                    })?,
            };
            let key = format!("{}@{version}", resolved.skill_id);
            if !supplied.insert(key.clone()) {
                return Err(PublishClosureError::Unresolved(format!(
                    "registry ref '{reference}' is still unresolved after {key} was supplied"
                )));
            }
            let record = client
                .version_record(&resolved.skill_id, &version)?
                .ok_or_else(|| {
                    PublishClosureError::Unresolved(format!(
                        "registry ref '{reference}' names no executable published version"
                    ))
                })?;
            installer
                .install(record)
                .map_err(|source| PublishClosureError::Install { reference, source })?;
        }
    }
    Err(PublishClosureError::Unresolved(format!(
        "the package's registry closure exceeds {MAX_CLOSURE_DEPTH} levels"
    )))
}

/// How a signed version record from one registry becomes an installed skill
/// in a project: verified against the keys trusted for that registry, then
/// installed, as a registry's package. A closure read from the registry and a
/// record a host already holds (`runx registry install --record`) install
/// this one way.
pub struct RecordInstaller {
    source_authority: RegistryManifestSourceAuthority,
    options: InstallLocalSkillOptions,
}

impl RecordInstaller {
    /// Installing into the project `env` names, records the registry at
    /// `registry_url` signed: their source authority is that registry's.
    ///
    /// # Errors
    ///
    /// Returns an error when the environment names a malformed trust key.
    pub fn new(
        registry_url: &str,
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, RegistryManifestTrustEnvError> {
        let paths = crate::RunxPaths::resolve(env, cwd);
        let source_authority = registry_manifest_source_authority_from_registry_url(registry_url);
        let options = InstallLocalSkillOptions {
            destination_root: paths.added_skills(),
            connectors_root: paths.added_connectors(),
            install_records: paths.install_records(),
            expected_digest: None,
            trusted_manifest_keys: trusted_registry_manifest_keys_from_env(env, &source_authority)?,
        };
        Ok(Self {
            source_authority,
            options,
        })
    }

    /// Install `record`, verified: its envelope signed by a trusted key for
    /// this registry, naming this skill and version, over these exact files.
    ///
    /// # Errors
    ///
    /// Returns the install's typed error when the record does not verify or
    /// does not install.
    pub fn install(
        &self,
        record: RegistrySkillVersion,
    ) -> Result<InstallLocalSkillResult, InstallError> {
        install_local_skill(
            &candidate_from_record(record, self.source_authority.clone()),
            &self.options,
        )
    }
}

/// A version's record, as an install candidate from `source_authority`.
fn candidate_from_record(
    record: RegistrySkillVersion,
    source_authority: RegistryManifestSourceAuthority,
) -> InstallCandidate {
    InstallCandidate {
        package: format!("{}@{}", record.skill_id, record.version),
        markdown: record.markdown,
        profile_document: record.profile_document,
        package_files: record.package_files,
        bundle_digest: record.bundle_digest,
        source: "runx-registry".to_owned(),
        source_label: "runx registry".to_owned(),
        skill_id: Some(record.skill_id),
        version: Some(record.version),
        signed_manifest: record.signed_manifest,
        profile_digest: record.profile_digest,
        method_names: record.method_names,
        trust_tier: Some(record.trust_tier),
        manifest_source_authority: Some(source_authority),
    }
}
