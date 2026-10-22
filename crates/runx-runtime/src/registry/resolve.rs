//! The one resolver for a skill or connector ref: `runx skill`, `runx
//! connector`, `runx registry install` and a turn's load all find a package
//! here, as a path, an installed or bundled package, or a registry ref
//! fetched into the project.

use std::collections::BTreeMap;
use std::path::Path;

mod skill;
mod target;

pub use skill::{
    AbsentPins, Holder, RegistryTrustState, ResolvedSkillRef, SkillRefKind, SkillResolverOptions,
    resolve_skill_ref_details,
};
pub use target::{
    LocalRegistrySourceKind, RegistryTarget, registry_source_description, resolve_registry_target,
};

use runx_contracts::connector::ConnectorName;

use super::{
    AcquireOptions, AcquiredRegistrySkill, FileRegistryStore, InstallCandidate,
    InstallRegistryConnectorOptions, InstalledRegistryConnector, RegistryClient,
    RegistryConnectorSearchResult, RegistryConnectorVersion, RegistryManifestSourceAuthority,
    RegistryResolveOptions, RegistrySearchResult, RegistrySkillResolution,
    TrustedRegistryManifestKey, UnreadVersion, VersionPage, install_registry_connector,
    resolve_registry_skill,
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

impl From<super::ConnectorInstallError> for ResolveError {
    fn from(error: super::ConnectorInstallError) -> Self {
        Self::Failed(format!("registry install: {error}"))
    }
}

/// What a ref installed: the package fetched, what installing it returned,
/// and the newest published version when this runx did not read it and
/// installed an older one in its place.
#[derive(Clone, Debug)]
pub struct Installed<P, T> {
    pub package: P,
    pub installed: T,
    pub newest_unread: Option<UnreadVersion>,
}

/// A skill fetched for install, and how a hosted registry served it.
pub type FetchedSkill = (InstallCandidate, Option<AcquiredRegistrySkill>);

/// Installs, with `install`, the newest version of the skill `subject` names
/// in `target` that this runx reads: `version` when one is named; otherwise
/// the current version, or when this runx does not read that, the newest
/// older one it does. None when the registry publishes no skill under that
/// name and version.
///
/// # Errors
///
/// Returns an error when the registry cannot be reached or read, when no
/// version this runx reads is published, or when installing fails otherwise.
pub fn install_skill_ref<T>(
    subject: &str,
    version: Option<&str>,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    mut install: impl FnMut(&InstallCandidate) -> Result<T, super::InstallError>,
) -> Result<Option<Installed<FetchedSkill, T>>, ResolveError> {
    let source_authority = target.manifest_source_authority();
    let install = |(candidate, _): &FetchedSkill| install(candidate);
    let unread = |error: &super::InstallError| match error {
        super::InstallError::Unreadable { reason, .. } => Some(reason.clone()),
        _ => None,
    };
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
            let acquire = |version: Option<&str>| -> Result<Option<FetchedSkill>, ResolveError> {
                match client.acquire(
                    &skill_id,
                    AcquireOptions {
                        installation_id: &installation_id,
                        version,
                        channel: Some("cli"),
                    },
                ) {
                    Err(super::RegistryClientError::HttpStatus { status: 404, .. }) => Ok(None),
                    acquired => {
                        let acquired = acquired?;
                        let candidate =
                            candidate_from_acquired(&skill_id, &acquired, source_authority.clone());
                        Ok(Some((candidate, Some(acquired))))
                    }
                }
            };
            let Some(first) = acquire(version)? else {
                return Ok(None);
            };
            newest_readable(
                &skill_id,
                version.is_some(),
                (skill_version(&first), first),
                |cursor| {
                    Ok(client
                        .skill_versions(&skill_id, cursor)?
                        .unwrap_or_default())
                },
                |older| acquire(Some(older)),
                install,
                unread,
            )
            .map(Some)
        }
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => {
            let store = FileRegistryStore::new(registry_path);
            let resolve = |version: Option<&str>| -> Result<Option<FetchedSkill>, ResolveError> {
                Ok(resolve_registry_skill(
                    &store,
                    subject,
                    RegistryResolveOptions {
                        version: version.map(ToOwned::to_owned),
                        registry_url: registry_url.clone(),
                    },
                )?
                .map(|resolution| {
                    let candidate =
                        candidate_from_resolution(subject, resolution, source_authority.clone());
                    (candidate, None)
                }))
            };
            let Some(first) = resolve(version)? else {
                return Ok(None);
            };
            let skill_id = first
                .0
                .skill_id
                .clone()
                .unwrap_or_else(|| subject.to_owned());
            newest_readable(
                &skill_id,
                version.is_some(),
                (skill_version(&first), first),
                // A registry directory lists every version at once, oldest first.
                |_| {
                    let mut versions = store
                        .list_versions(&skill_id)?
                        .into_iter()
                        .map(|record| record.version)
                        .collect::<Vec<_>>();
                    versions.reverse();
                    Ok(VersionPage {
                        versions,
                        next_cursor: None,
                    })
                },
                |older| resolve(Some(older)),
                install,
                unread,
            )
            .map(Some)
        }
    }
}

fn skill_version((candidate, _): &FetchedSkill) -> String {
    candidate.version.clone().unwrap_or_default()
}

/// Installs, with `install`, the newest version of a name this runx reads,
/// beginning with `first`, the version asked for or the current one. Unless a
/// version was `named`, a version this runx does not read gives way to each
/// older one in turn, newest first, fetched with `fetch` from the pages `page`
/// lists; any other failure stops at once.
fn newest_readable<P, T, E: Into<ResolveError>>(
    subject: &str,
    named: bool,
    first: (String, P),
    mut page: impl FnMut(Option<&str>) -> Result<VersionPage, ResolveError>,
    mut fetch: impl FnMut(&str) -> Result<Option<P>, ResolveError>,
    mut install: impl FnMut(&P) -> Result<T, E>,
    unread: impl Fn(&E) -> Option<String>,
) -> Result<Installed<P, T>, ResolveError> {
    let (version, package) = first;
    let reason = match install(&package) {
        Ok(installed) => {
            return Ok(Installed {
                package,
                installed,
                newest_unread: None,
            });
        }
        Err(error) => match unread(&error) {
            Some(reason) if !named => reason,
            _ => return Err(error.into()),
        },
    };
    let newest = UnreadVersion { version, reason };
    let mut cursor = None;
    loop {
        let VersionPage {
            versions,
            next_cursor,
        } = page(cursor.as_deref())?;
        for older in versions.iter().filter(|older| **older != newest.version) {
            let Some(package) = fetch(older)? else {
                continue;
            };
            match install(&package) {
                Ok(installed) => {
                    return Ok(Installed {
                        package,
                        installed,
                        newest_unread: Some(newest),
                    });
                }
                Err(error) if unread(&error).is_some() => {}
                Err(error) => return Err(error.into()),
            }
        }
        match next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Err(ResolveError::Failed(format!(
        "runx {} reads no published version of {subject}; the newest, {subject}@{}: {}",
        crate::EXECUTION_RUNTIME_RELEASE,
        newest.version,
        newest.reason
    )))
}

/// What a registry publishes matching a query: skills, and the connectors a
/// hosted registry holds.
#[derive(Clone, Debug)]
pub struct RegistrySearch {
    /// `remote`, `local` or `file`.
    pub source: &'static str,
    pub results: Vec<RegistrySearchResult>,
    pub connectors: Vec<RegistryConnectorSearchResult>,
}

/// Search the registry `target` names for `query`, at most `limit` of each
/// kind (20 from a hosted registry when unset).
///
/// # Errors
///
/// Returns an error when the registry cannot be reached or read.
pub fn search(
    query: &str,
    limit: Option<usize>,
    target: RegistryTarget,
    env: &BTreeMap<String, String>,
) -> Result<RegistrySearch, ResolveError> {
    let source = target.label();
    Ok(match target {
        // Connectors are published only to a hosted registry.
        RegistryTarget::Remote { registry_url } => {
            let client = registry_client(&registry_url, env)?;
            let limit = limit.unwrap_or(20);
            RegistrySearch {
                source,
                connectors: client.search_connectors(query, limit)?,
                results: client.search_with_limit(query, limit)?,
            }
        }
        RegistryTarget::Local {
            registry_path,
            registry_url,
            ..
        } => RegistrySearch {
            source,
            results: super::search_registry_with_options(
                &FileRegistryStore::new(registry_path),
                query,
                super::RegistrySearchOptions {
                    limit,
                    registry_url,
                },
            )?,
            connectors: Vec::new(),
        },
    })
}

/// Adds connector `name` from the hosted registry at `registry_url` to the
/// project's added connectors: `version`, or the version its content digest
/// pins, when either is named; otherwise the newest version this runx reads,
/// the current one or, when this runx does not read that, the newest older
/// one it does. None when the registry publishes no connector under that name
/// and version.
///
/// # Errors
///
/// Returns an error when the registry cannot be read, when what it serves is
/// not the connector asked for or does not verify, or when no version this
/// runx reads is published.
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
    let options = InstallRegistryConnectorOptions {
        connectors_root: crate::RunxPaths::resolve(env, cwd).added_connectors(),
        install_records: crate::RunxPaths::resolve(env, cwd).install_records(),
        expected_content_digest: expected_content_digest.map(ToOwned::to_owned),
        trusted_manifest_keys: trusted_manifest_keys_for_source(env, source_authority.clone())?,
        manifest_source_authority: source_authority,
    };
    let package = |version: &str| -> Result<Option<RegistryConnectorVersion>, ResolveError> {
        let Some(published) = client.connector_package(name, version)? else {
            return Ok(None);
        };
        if &published.name != name || published.version != version {
            return Err(ResolveError::Failed(format!(
                "the registry served {}@{} for {name}@{version}",
                published.name, published.version
            )));
        }
        Ok(Some(published))
    };
    let first_version = match version {
        Some(version) => version.to_owned(),
        None => match client.read_connector(name, None)? {
            Some(detail) => detail.version,
            None => return Ok(None),
        },
    };
    let Some(first) = package(&first_version)? else {
        return Ok(None);
    };
    let Installed {
        installed,
        newest_unread,
        ..
    } = newest_readable(
        name.as_str(),
        version.is_some() || expected_content_digest.is_some(),
        (first_version, first),
        |cursor| Ok(client.connector_versions(name, cursor)?.unwrap_or_default()),
        package,
        |published| install_registry_connector(published, &options),
        |error| match error {
            super::ConnectorInstallError::Unreadable { reason, .. } => Some(reason.clone()),
            _ => None,
        },
    )?;
    Ok(Some(InstalledRegistryConnector {
        newest_unread,
        ..installed
    }))
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// A package this runx reads, does not read, or fails to install otherwise.
    #[derive(Clone, Debug)]
    enum Fake {
        Reads,
        Unread,
        Broken,
    }

    enum Attempt {
        Unread,
        Broken,
    }

    impl From<Attempt> for ResolveError {
        fn from(attempt: Attempt) -> Self {
            Self::Failed(match attempt {
                Attempt::Unread => "unread".to_owned(),
                Attempt::Broken => "broken".to_owned(),
            })
        }
    }

    /// Runs the rule over `published`, newest first, in pages of `per_page`,
    /// recording each version fetched.
    fn run(
        published: &[(&str, Fake)],
        per_page: usize,
        named: bool,
    ) -> (Result<Installed<String, String>, ResolveError>, Vec<String>) {
        let fetched = RefCell::new(Vec::new());
        let package = |version: &str| {
            published
                .iter()
                .find(|(listed, _)| *listed == version)
                .map(|(listed, fake)| ((*listed).to_owned(), fake.clone()))
        };
        let Some(first) = published.first().and_then(|(current, _)| package(current)) else {
            return (
                Err(ResolveError::Failed("nothing is published".to_owned())),
                Vec::new(),
            );
        };
        let result = newest_readable(
            "acme/echo",
            named,
            (first.0.clone(), first),
            |cursor| {
                let start = cursor.map_or(0, |cursor| cursor.parse::<usize>().unwrap_or(0));
                let versions = published
                    .iter()
                    .skip(start)
                    .take(per_page)
                    .map(|(version, _)| (*version).to_owned())
                    .collect();
                let next = start + per_page;
                Ok(VersionPage {
                    versions,
                    next_cursor: (next < published.len()).then(|| next.to_string()),
                })
            },
            |version| {
                fetched.borrow_mut().push(version.to_owned());
                Ok(package(version))
            },
            |(version, fake): &(String, Fake)| match fake {
                Fake::Reads => Ok(version.clone()),
                Fake::Unread => Err(Attempt::Unread),
                Fake::Broken => Err(Attempt::Broken),
            },
            |attempt| {
                matches!(attempt, Attempt::Unread).then(|| "colour is not supported".to_owned())
            },
        )
        .map(|installed| Installed {
            package: installed.package.0,
            installed: installed.installed,
            newest_unread: installed.newest_unread,
        });
        (result, fetched.into_inner())
    }

    /// The refusal a run ends in, or why it installed instead.
    fn refusal(result: Result<Installed<String, String>, ResolveError>) -> Result<String, String> {
        match result {
            Ok(installed) => Err(format!("installed {}", installed.installed)),
            Err(error) => Ok(error.into_message()),
        }
    }

    #[test]
    fn the_current_version_installs_when_this_runx_reads_it() -> Result<(), String> {
        let (result, fetched) = run(&[("3", Fake::Reads), ("2", Fake::Reads)], 2, false);
        let installed = result.map_err(ResolveError::into_message)?;
        assert_eq!(installed.installed, "3");
        assert_eq!(installed.newest_unread, None);
        assert!(fetched.is_empty());
        Ok(())
    }

    #[test]
    fn an_unread_version_gives_way_to_the_newest_older_one_across_pages() -> Result<(), String> {
        let published = [
            ("4", Fake::Unread),
            ("3", Fake::Unread),
            ("2", Fake::Reads),
            ("1", Fake::Reads),
        ];
        let (result, fetched) = run(&published, 2, false);
        let installed = result.map_err(ResolveError::into_message)?;
        assert_eq!(installed.installed, "2");
        assert_eq!(
            installed.newest_unread,
            Some(UnreadVersion {
                version: "4".to_owned(),
                reason: "colour is not supported".to_owned(),
            })
        );
        assert_eq!(fetched, ["3", "2"]);
        Ok(())
    }

    #[test]
    fn a_named_version_this_runx_does_not_read_refuses() -> Result<(), String> {
        let (result, fetched) = run(&[("2", Fake::Unread), ("1", Fake::Reads)], 2, true);
        assert_eq!(refusal(result)?, "unread");
        assert!(fetched.is_empty());
        Ok(())
    }

    #[test]
    fn any_other_failure_stops_at_once() -> Result<(), String> {
        let published = [("3", Fake::Unread), ("2", Fake::Broken), ("1", Fake::Reads)];
        let (result, fetched) = run(&published, 3, false);
        assert_eq!(refusal(result)?, "broken");
        assert_eq!(fetched, ["2"]);
        Ok(())
    }

    #[test]
    fn no_version_this_runx_reads_names_the_newest() -> Result<(), String> {
        let (result, fetched) = run(&[("2", Fake::Unread), ("1", Fake::Unread)], 1, false);
        assert_eq!(
            refusal(result)?,
            format!(
                "runx {} reads no published version of acme/echo; the newest, acme/echo@2: colour is not supported",
                crate::EXECUTION_RUNTIME_RELEASE
            )
        );
        assert_eq!(fetched, ["1"]);
        Ok(())
    }
}
