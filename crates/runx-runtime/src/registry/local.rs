// Module rationale: this untracked registry file is under
// active parallel work; keep the module stable while extracting blockers here.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::refs::parse_registry_ref;
use super::types::{
    PublishSkillMarkdownResult, PublishStatus, RegistryAttestation, RegistryLinkResolution,
    RegistryPublishHarnessReport, RegistryPublisher, RegistrySearchResult, RegistrySkill,
    RegistrySkillDetail, RegistrySkillResolution, RegistrySkillVersion, RegistrySourceMetadata,
    TrustTier,
};

#[derive(Clone, Debug)]
pub struct FileRegistryStore {
    root: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PutVersionOptions {
    pub upsert: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IngestSkillOptions {
    pub owner: Option<String>,
    pub version: Option<String>,
    pub created_at: Option<String>,
    pub profile_document: Option<String>,
    pub package_files: Vec<super::types::RegistryPackageFile>,
    pub publisher: Option<RegistryPublisher>,
    pub trust_tier: Option<TrustTier>,
    pub attestations: Vec<RegistryAttestation>,
    pub source_metadata: Option<RegistrySourceMetadata>,
    pub upsert: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct CreateRegistrySkillVersionResult {
    pub record: RegistrySkillVersion,
    pub created: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PublishSkillMarkdownOptions {
    pub ingest: IngestSkillOptions,
    pub registry_url: Option<String>,
    pub harness: RegistryPublishHarnessReport,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistrySearchOptions {
    pub limit: Option<usize>,
    pub registry_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistryResolveOptions {
    pub version: Option<String>,
    pub registry_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LocalRegistryError {
    #[error("{0}")]
    Package(#[from] runx_parser::SkillPackageError),
    #[error("{0}")]
    Parse(#[from] runx_parser::ParseError),
    #[error("{0}")]
    Validation(#[from] runx_parser::ValidationError),
    #[error("io error while {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("invalid registry JSON at {path}: {source}")]
    JsonRead {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to serialize registry JSON at {path}: {source}")]
    JsonWrite {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid registry version payload at {field}: {message}")]
    InvalidVersionPayload { field: String, message: String },
    #[error("invalid registry skill manifest at {field}: {message}")]
    InvalidSkillManifest { field: String, message: String },
    #[error("invalid registry skill id '{0}'. Expected '<owner>/<name>'.")]
    InvalidSkillId(String),
    #[error("registry slugs cannot be empty")]
    EmptySlug,
    #[error("registry path component '{0}' is not allowed")]
    UnsafePathComponent(String),
    #[error("registry version {skill_id}@{version} already exists with a different digest")]
    VersionConflict { skill_id: String, version: String },
    #[error("Registry ref '{0}' is ambiguous. Use '<owner>/<name>' instead.")]
    Ambiguous(String),
}

impl FileRegistryStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn put_version(
        &self,
        version: RegistrySkillVersion,
        options: PutVersionOptions,
    ) -> Result<RegistrySkillVersion, LocalRegistryError> {
        let version_path = self.version_path(&version.skill_id, &version.version)?;
        if let Some(parent) = version_path.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error("creating", parent, source))?;
        }

        if let Some(existing) = self.get_version(&version.skill_id, Some(&version.version))? {
            if existing.digest != version.digest
                || existing.profile_digest != version.profile_digest
                || existing.package_digest != version.package_digest
                || existing.paid_listing != version.paid_listing
            {
                if !options.upsert {
                    return Err(LocalRegistryError::VersionConflict {
                        skill_id: version.skill_id,
                        version: version.version,
                    });
                }
                let mut upserted = version;
                upserted.updated_at = now_iso8601();
                write_registry_json(&version_path, &upserted, false)?;
                return Ok(upserted);
            }

            let mut refreshed = version;
            refreshed.created_at = existing.created_at.clone();
            refreshed.updated_at = now_iso8601();
            if existing != refreshed {
                write_registry_json(&version_path, &refreshed, false)?;
            }
            return Ok(refreshed);
        }

        write_registry_json(&version_path, &version, true)?;
        Ok(version)
    }

    pub fn get_version(
        &self,
        skill_id: &str,
        version: Option<&str>,
    ) -> Result<Option<RegistrySkillVersion>, LocalRegistryError> {
        if let Some(version) = version {
            return self.read_version(skill_id, version);
        }

        let versions = self.list_versions(skill_id)?;
        Ok(versions.last().cloned())
    }

    pub fn list_versions(
        &self,
        skill_id: &str,
    ) -> Result<Vec<RegistrySkillVersion>, LocalRegistryError> {
        let skill_dir = self.skill_dir(skill_id)?;
        let mut files = safe_read_dir_names(&skill_dir)?;
        files.sort();

        let mut versions = Vec::new();
        for file in files.into_iter().filter(|file| file.ends_with(".json")) {
            let path = skill_dir.join(file);
            versions.push(read_registry_version(&path)?);
        }
        versions.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.version.cmp(&right.version))
        });
        Ok(versions)
    }

    pub fn list_skills(&self) -> Result<Vec<RegistrySkill>, LocalRegistryError> {
        let owners = safe_read_dir_names(&self.root)?;
        let mut skills = Vec::new();
        for owner in owners {
            let owner_dir = self.root.join(&owner);
            for name in safe_read_dir_names(&owner_dir)? {
                let skill_id = format!("{}/{}", decode_part(&owner)?, decode_part(&name)?);
                let versions = self.list_versions(&skill_id)?;
                let Some(latest) = versions.last() else {
                    continue;
                };
                skills.push(RegistrySkill {
                    skill_id,
                    owner: latest.owner.clone(),
                    name: latest.name.clone(),
                    description: latest.description.clone(),
                    category: latest.category.clone(),
                    source_category: latest.source_category.clone(),
                    latest_version: latest.version.clone(),
                    latest_digest: latest.digest.clone(),
                    versions,
                });
            }
        }
        skills.sort_by(|left, right| left.skill_id.cmp(&right.skill_id));
        Ok(skills)
    }

    fn version_path(&self, skill_id: &str, version: &str) -> Result<PathBuf, LocalRegistryError> {
        Ok(self
            .skill_dir(skill_id)?
            .join(format!("{}.json", encode_part(version))))
    }

    fn read_version(
        &self,
        skill_id: &str,
        version: &str,
    ) -> Result<Option<RegistrySkillVersion>, LocalRegistryError> {
        let path = self.version_path(skill_id, version)?;
        let record = match read_registry_version(&path) {
            Ok(record) => record,
            Err(LocalRegistryError::Io { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if record.skill_id != skill_id {
            return Err(LocalRegistryError::InvalidVersionPayload {
                field: "registry_version.skill_id".to_owned(),
                message: format!(
                    "{} does not match registry path {skill_id}",
                    record.skill_id
                ),
            });
        }
        if record.version != version {
            return Err(LocalRegistryError::InvalidVersionPayload {
                field: "registry_version.version".to_owned(),
                message: format!("{} does not match registry path {version}", record.version),
            });
        }
        Ok(Some(record))
    }

    fn skill_dir(&self, skill_id: &str) -> Result<PathBuf, LocalRegistryError> {
        let (owner, name) = split_skill_id(skill_id)?;
        Ok(self.root.join(encode_part(owner)).join(encode_part(name)))
    }
}

fn read_registry_version(path: &Path) -> Result<RegistrySkillVersion, LocalRegistryError> {
    let contents = fs::read_to_string(path).map_err(|source| io_error("reading", path, source))?;
    let payload =
        serde_json::from_str::<RegistrySkillVersionPayload>(&contents).map_err(|source| {
            LocalRegistryError::JsonRead {
                path: path.to_path_buf(),
                source,
            }
        })?;
    normalize_registry_skill_version(payload)
}

pub fn ingest_skill_markdown(
    store: &FileRegistryStore,
    markdown: &str,
    options: IngestSkillOptions,
) -> Result<RegistrySkillVersion, LocalRegistryError> {
    Ok(create_registry_skill_version(store, markdown, options)?.record)
}

fn create_registry_skill_version(
    store: &FileRegistryStore,
    markdown: &str,
    options: IngestSkillOptions,
) -> Result<CreateRegistrySkillVersionResult, LocalRegistryError> {
    let record = build_registry_skill_version(markdown, &options)?;
    let existing = store.get_version(&record.skill_id, Some(&record.version))?;
    if let Some(existing) = existing {
        if existing.digest != record.digest
            || existing.profile_digest != record.profile_digest
            || existing.package_digest != record.package_digest
            || existing.paid_listing != record.paid_listing
        {
            if !options.upsert {
                return Err(LocalRegistryError::VersionConflict {
                    skill_id: record.skill_id,
                    version: record.version,
                });
            }
            return Ok(CreateRegistrySkillVersionResult {
                record: store.put_version(record, PutVersionOptions { upsert: true })?,
                created: false,
            });
        }
        let mut refreshed = record;
        refreshed.created_at = existing.created_at;
        return Ok(CreateRegistrySkillVersionResult {
            record: store.put_version(refreshed, PutVersionOptions::default())?,
            created: false,
        });
    }

    Ok(CreateRegistrySkillVersionResult {
        record: store.put_version(record, PutVersionOptions::default())?,
        created: true,
    })
}

mod build;
mod trust;
mod util;

use build::{
    RegistrySkillVersionPayload, build_registry_skill_version, normalize_registry_skill_version,
};
use trust::{
    detail_for_version, normalize, resolve_by_name, search_result_for_version, searchable_text,
};
use util::{
    decode_part, encode_part, encode_uri_component, io_error, is_unsafe_path_component,
    now_iso8601, reject_unsafe_path_component, safe_read_dir_names, write_registry_json,
};

pub fn publish_skill_markdown(
    store: &FileRegistryStore,
    markdown: &str,
    options: PublishSkillMarkdownOptions,
) -> Result<PublishSkillMarkdownResult, LocalRegistryError> {
    let result = create_registry_skill_version(store, markdown, options.ingest)?;
    let link = runx_link_for_version(&result.record, options.registry_url.as_deref());
    Ok(PublishSkillMarkdownResult {
        status: if result.created {
            PublishStatus::Published
        } else {
            PublishStatus::Unchanged
        },
        skill_id: result.record.skill_id.clone(),
        name: result.record.name.clone(),
        version: result.record.version.clone(),
        digest: result.record.digest.clone(),
        signed_manifest: result.record.signed_manifest.clone(),
        profile_digest: result.record.profile_digest.clone(),
        runner_names: result.record.runner_names.clone(),
        source_type: result.record.source_type.clone(),
        registry_url: options.registry_url,
        harness: options.harness,
        link,
        record: result.record,
    })
}

pub fn search_registry_with_options(
    store: &FileRegistryStore,
    query: &str,
    options: RegistrySearchOptions,
) -> Result<Vec<RegistrySearchResult>, LocalRegistryError> {
    let normalized_query = normalize(query);
    let mut matches = store
        .list_skills()?
        .into_iter()
        .filter_map(|skill| skill.versions.last().cloned())
        .filter(registry_version_is_public)
        .filter(|version| {
            normalized_query.is_empty() || searchable_text(version).contains(&normalized_query)
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.skill_id.cmp(&right.skill_id));
    matches.truncate(options.limit.unwrap_or(20));
    Ok(matches
        .iter()
        .map(|version| search_result_for_version(version, options.registry_url.as_deref()))
        .collect())
}

fn registry_version_is_public(version: &RegistrySkillVersion) -> bool {
    version.catalog_visibility.as_deref() != Some("internal")
}

pub fn resolve_registry_skill(
    store: &FileRegistryStore,
    registry_ref: &str,
    options: RegistryResolveOptions,
) -> Result<Option<RegistrySkillResolution>, LocalRegistryError> {
    let parsed = parse_registry_ref(registry_ref);
    let version = options.version.as_deref().or(parsed.version.as_deref());
    let record = if parsed.skill_id.contains('/') {
        store.get_version(&parsed.skill_id, version)?
    } else {
        resolve_by_name(store, &parsed.skill_id, version)?
    };
    Ok(record.map(|record| {
        let link = runx_link_for_version(&record, options.registry_url.as_deref());
        RegistrySkillResolution {
            markdown: record.markdown,
            profile_document: record.profile_document,
            profile_digest: record.profile_digest,
            package_files: record.package_files,
            package_digest: record.package_digest,
            paid_listing: record.paid_listing,
            runner_names: record.runner_names,
            skill_id: record.skill_id,
            name: record.name,
            version: record.version,
            digest: record.digest,
            signed_manifest: record.signed_manifest,
            source: "runx-registry".to_owned(),
            source_label: "runx registry".to_owned(),
            source_type: record.source_type,
            trust_tier: record.trust_tier,
            registry_url: options.registry_url,
            install_command: link.install_command,
            run_command: link.run_command,
        }
    }))
}

pub fn read_registry_skill(
    store: &FileRegistryStore,
    skill_id: &str,
    version: Option<&str>,
    registry_url: Option<&str>,
) -> Result<Option<RegistrySkillDetail>, LocalRegistryError> {
    Ok(store
        .get_version(skill_id, version)?
        .map(|record| detail_for_version(&record, registry_url)))
}

pub fn resolve_runx_link(
    store: &FileRegistryStore,
    skill_id: &str,
    version: Option<&str>,
    registry_url: Option<&str>,
) -> Result<Option<RegistryLinkResolution>, LocalRegistryError> {
    Ok(store
        .get_version(skill_id, version)?
        .map(|record| runx_link_for_version(&record, registry_url)))
}

pub fn runx_link_for_version(
    record: &RegistrySkillVersion,
    registry_url: Option<&str>,
) -> RegistryLinkResolution {
    let registry_ref = format!("{}@{}", record.skill_id, record.version);
    let registry_flag = registry_url.map_or_else(String::new, |url| format!(" --registry {url}"));
    RegistryLinkResolution {
        link: format!(
            "runx://skill/{}@{}",
            encode_uri_component(&record.skill_id),
            encode_uri_component(&record.version)
        ),
        skill_id: record.skill_id.clone(),
        version: record.version.clone(),
        digest: record.digest.clone(),
        registry_url: registry_url.map(ToOwned::to_owned),
        install_command: format!("runx add {registry_ref}{registry_flag}"),
        run_command: format!("runx skill {registry_ref}{registry_flag}"),
    }
}

pub fn build_skill_id(owner: &str, name: &str) -> Result<String, LocalRegistryError> {
    Ok(format!("{}/{}", slugify(owner)?, slugify(name)?))
}

pub fn split_skill_id(skill_id: &str) -> Result<(&str, &str), LocalRegistryError> {
    let mut parts = skill_id.split('/');
    let Some(owner) = parts.next().filter(|part| !part.is_empty()) else {
        return Err(LocalRegistryError::InvalidSkillId(skill_id.to_owned()));
    };
    let Some(name) = parts.next().filter(|part| !part.is_empty()) else {
        return Err(LocalRegistryError::InvalidSkillId(skill_id.to_owned()));
    };
    if parts.next().is_some() {
        return Err(LocalRegistryError::InvalidSkillId(skill_id.to_owned()));
    }
    reject_unsafe_path_component(owner)?;
    reject_unsafe_path_component(name)?;
    Ok((owner, name))
}

pub fn slugify(value: &str) -> Result<String, LocalRegistryError> {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.trim().to_lowercase().chars() {
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-');
        if keep {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        Err(LocalRegistryError::EmptySlug)
    } else if is_unsafe_path_component(&slug) {
        Err(LocalRegistryError::UnsafePathComponent(slug))
    } else {
        Ok(slug)
    }
}
