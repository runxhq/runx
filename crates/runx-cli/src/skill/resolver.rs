// Module rationale: skill resolution centralizes local, official, installed, and registry paths.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::PaidSkillListing;
use runx_contracts::sha256_prefixed;
use runx_runtime::registry::{
    InstallCandidate, InstallLocalSkillOptions, materialization_cache_path,
    materialization_digest_marker, parse_registry_ref, split_skill_id,
};

use crate::official_skills::official_skill_entry_by_name;
use crate::registry::{self, RegistryAction, RegistryPlan};

pub(super) struct SkillResolverOptions<'a> {
    pub(super) env: &'a BTreeMap<String, String>,
    pub(super) registry: Option<&'a str>,
    pub(super) expected_digest: Option<&'a str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SkillRefKind {
    ExplicitPath,
    ExportedShim,
    WorkspaceLocal,
    Installed,
    Official,
    Registry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RegistryTrustState {
    Trusted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedSkillRef {
    pub(crate) kind: SkillRefKind,
    pub(crate) skill_id: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) digest: Option<String>,
    pub(crate) profile_digest: Option<String>,
    pub(crate) package_digest: Option<String>,
    pub(crate) registry_source: Option<String>,
    pub(crate) registry_source_fingerprint: Option<String>,
    pub(crate) trust_state: Option<RegistryTrustState>,
    pub(crate) trust_tier: Option<String>,
    pub(crate) registry_key_id: Option<String>,
    pub(crate) paid_listing: Option<PaidSkillListing>,
    pub(crate) hosted_registry_url: Option<String>,
    pub(crate) runnable_path: PathBuf,
}

pub(crate) fn resolve_skill_ref_details(
    skill_ref: &Path,
    cwd: &Path,
    options: SkillResolverOptions<'_>,
) -> Result<ResolvedSkillRef, String> {
    if skill_ref.exists() {
        let path = resolve_exported_skill_shim(skill_ref)?;
        let kind = if path == skill_ref {
            SkillRefKind::ExplicitPath
        } else {
            SkillRefKind::ExportedShim
        };
        return Ok(local_resolved(kind, path));
    }

    let raw_ref = skill_ref.to_string_lossy();
    let parsed = parse_registry_ref(&raw_ref);
    if is_registry_prefixed_ref(&raw_ref) {
        if split_skill_id(&parsed.skill_id).is_err() {
            return Err(format!(
                "Registry ref '{}' is ambiguous. Use '<owner>/<name>' instead.",
                raw_ref
            ));
        }
        return resolve_registry_skill(&parsed.skill_id, parsed.version.as_deref(), cwd, options);
    }

    if is_bare_skill_ref(skill_ref) {
        return resolve_bare_skill_ref(skill_ref, &raw_ref, cwd, options);
    }

    if is_explicit_registry_ref(&raw_ref, &parsed.skill_id) {
        return resolve_registry_skill(&parsed.skill_id, parsed.version.as_deref(), cwd, options);
    }

    Ok(local_resolved(
        SkillRefKind::ExplicitPath,
        skill_ref.to_path_buf(),
    ))
}

fn resolve_bare_skill_ref(
    skill_ref: &Path,
    raw_ref: &str,
    cwd: &Path,
    options: SkillResolverOptions<'_>,
) -> Result<ResolvedSkillRef, String> {
    if let Some(path) = resolve_installed_or_workspace_skill(raw_ref, cwd, &options)? {
        return Ok(path);
    }
    if let Some(entry) = official_skill_entry_by_name(raw_ref) {
        return resolve_official_skill(entry.skill_id, entry.version, entry.digest, cwd, options);
    }
    Err(format!(
        "could not resolve skill ref '{}'; tried {} and {}. Search with `runx registry search {}` or run a registry ref directly with `runx skill <owner>/<name>@<version>`.",
        skill_ref.display(),
        cwd.join("skills").join(skill_ref).display(),
        runx_runtime::RunxPaths::resolve(options.env, cwd)
            .workspace()
            .to_path_buf()
            .join("skills")
            .join(skill_ref)
            .display(),
        raw_ref
    ))
}

fn resolve_installed_or_workspace_skill(
    name: &str,
    cwd: &Path,
    options: &SkillResolverOptions<'_>,
) -> Result<Option<ResolvedSkillRef>, String> {
    for root in installed_roots(cwd, options.env) {
        let candidate = root.join(name);
        if candidate.exists() {
            let path = resolve_exported_skill_shim(&candidate)?;
            let kind = if path == candidate {
                if root == cwd.join("skills") {
                    SkillRefKind::WorkspaceLocal
                } else {
                    SkillRefKind::Installed
                }
            } else {
                SkillRefKind::ExportedShim
            };
            return Ok(Some(local_resolved(kind, path)));
        }
    }
    Ok(None)
}

fn installed_roots(cwd: &Path, env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut roots = vec![cwd.join("skills")];
    let workspace_root = runx_runtime::RunxPaths::resolve(env, cwd)
        .workspace()
        .to_path_buf()
        .join("skills");
    if !roots.contains(&workspace_root) {
        roots.push(workspace_root);
    }
    roots
}

fn resolve_official_skill(
    skill_id: &str,
    version: &str,
    digest: &str,
    cwd: &Path,
    options: SkillResolverOptions<'_>,
) -> Result<ResolvedSkillRef, String> {
    let registry_override = official_registry_override(options.env, options.registry);
    let expected_digest = options.expected_digest.unwrap_or(digest);
    materialize_trusted_registry_skill(
        RegistryMaterializationRequest {
            skill_id,
            version: Some(version),
            cache_root: CacheRoot::Official,
            registry_override: Some(&registry_override),
            expected_digest: Some(expected_digest),
            kind: SkillRefKind::Official,
            cwd,
        },
        options,
    )
}

fn resolve_registry_skill(
    skill_id: &str,
    version: Option<&str>,
    cwd: &Path,
    options: SkillResolverOptions<'_>,
) -> Result<ResolvedSkillRef, String> {
    materialize_trusted_registry_skill(
        RegistryMaterializationRequest {
            skill_id,
            version,
            cache_root: CacheRoot::Registry,
            registry_override: None,
            expected_digest: options.expected_digest,
            kind: SkillRefKind::Registry,
            cwd,
        },
        options,
    )
}

// Function rationale: registry materialization is kept atomic around trust, cache, and digest checks.
fn materialize_trusted_registry_skill(
    request: RegistryMaterializationRequest<'_>,
    options: SkillResolverOptions<'_>,
) -> Result<ResolvedSkillRef, String> {
    let RegistryMaterializationRequest {
        skill_id,
        version,
        cache_root,
        registry_override,
        expected_digest,
        kind,
        cwd,
    } = request;
    let env = options.env;
    let registry = registry_override.or(options.registry);
    let mut plan = RegistryPlan {
        action: RegistryAction::Install,
        subject: skill_id.to_owned(),
        registry: registry.map(ToOwned::to_owned),
        registry_dir: None,
        version: version.map(ToOwned::to_owned),
        expected_digest: expected_digest.map(ToOwned::to_owned),
        destination: None,
        owner: None,
        profile: None,
        trust_tier: None,
        limit: None,
        upsert: false,
        json: true,
    };
    let target = registry::resolve_registry_target(&plan, env, cwd);
    let source_description = registry::registry_source_description(&target);
    let hosted_registry_url = match &target {
        registry::RegistryTarget::Remote { registry_url } => Some(registry_url.clone()),
        registry::RegistryTarget::Local { .. } => None,
    };
    let source_fingerprint = registry_source_fingerprint(&target);
    let source_authority = target.manifest_source_authority();
    let (mut candidate, acquisition) = registry::install_candidate(&plan, target, env, cwd)
        .map_err(|error| error.into_message())?;
    let paid_listing = acquisition
        .as_ref()
        .and_then(|acquired| acquired.paid_listing.clone());
    if cache_root == CacheRoot::Official {
        candidate.manifest_source_authority =
            Some(runx_runtime::registry::RegistryManifestSourceAuthority::OfficialRunx);
    }
    canonicalize_candidate_ref(&mut candidate);
    let identity = registry_cache_identity(&candidate)?;
    let destination_root =
        destination_root_for_cache(cache_root, env, cwd, &source_fingerprint, &identity)?;
    plan.expected_digest = expected_digest.map(ToOwned::to_owned);
    let install = runx_runtime::registry::install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: destination_root.to_path_buf(),
            expected_digest: plan.expected_digest,
            trusted_manifest_keys: registry::trusted_manifest_keys_from_env_for_source(
                env,
                candidate
                    .manifest_source_authority
                    .clone()
                    .unwrap_or(source_authority),
            )
            .map_err(|error| error.into_message())?,
        },
    )
    .map_err(crate::registry::RegistryCliError::from)
    .map_err(|error| error.into_message())?;
    let runnable_path = install
        .destination
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "registry install returned invalid path {}",
                install.destination.display()
            )
        })?;
    if cache_root == CacheRoot::Official {
        sync_packaged_official_skill_assets(&runnable_path, skill_id)?;
    }
    Ok(ResolvedSkillRef {
        kind,
        skill_id: identity.skill_id,
        version: identity.version,
        digest: Some(install.digest),
        profile_digest: install.profile_digest,
        // Registry package-file digests cover only auxiliary registry files.
        // Execution package digests cover the complete validated package and
        // are bound separately by `runx skill --package-digest`.
        package_digest: None,
        registry_source: Some(source_description),
        registry_source_fingerprint: Some(source_fingerprint),
        trust_state: Some(RegistryTrustState::Trusted),
        trust_tier: install.trust_tier.as_ref().map(trust_tier_string),
        registry_key_id: candidate
            .signed_manifest
            .as_ref()
            .map(|manifest| manifest.signer.key_id.clone()),
        paid_listing,
        hosted_registry_url,
        runnable_path,
    })
}

struct RegistryMaterializationRequest<'a> {
    skill_id: &'a str,
    version: Option<&'a str>,
    cache_root: CacheRoot,
    registry_override: Option<&'a str>,
    expected_digest: Option<&'a str>,
    kind: SkillRefKind,
    cwd: &'a Path,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheRoot {
    Official,
    Registry,
}

fn destination_root_for_cache(
    cache_root: CacheRoot,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    source_fingerprint: &str,
    identity: &RegistryCacheIdentity,
) -> Result<PathBuf, String> {
    let skill_id = identity
        .skill_id
        .as_deref()
        .ok_or_else(|| "registry skill is missing skill_id".to_owned())?;
    let (owner, name) = split_skill_id(skill_id).map_err(|error| error.to_string())?;
    let version = identity
        .version
        .as_deref()
        .ok_or_else(|| "registry skill is missing version".to_owned())?;
    let digest = identity
        .digest
        .as_deref()
        .ok_or_else(|| "registry skill is missing signed digest".to_owned())?;
    let paths = runx_runtime::RunxPaths::resolve(env, cwd);
    let root = match cache_root {
        CacheRoot::Official => registry::official_skills_cache_root(&paths, env),
        CacheRoot::Registry => paths.registry_skills().join(source_fingerprint),
    };
    Ok(materialization_cache_path(
        &root,
        owner,
        name,
        version,
        &cache_identity_digest(
            digest,
            identity.profile_digest.as_deref(),
            identity.package_digest.as_deref(),
        ),
    ))
}

fn registry_cache_identity(candidate: &InstallCandidate) -> Result<RegistryCacheIdentity, String> {
    let manifest = candidate.signed_manifest.as_ref().ok_or_else(|| {
        format!(
            "registry signed manifest is required for {}",
            candidate.r#ref
        )
    })?;
    Ok(RegistryCacheIdentity {
        skill_id: candidate
            .skill_id
            .clone()
            .or_else(|| Some(manifest.skill_id.clone())),
        version: candidate
            .version
            .clone()
            .or_else(|| Some(manifest.version.clone())),
        digest: Some(manifest.digest.clone()),
        profile_digest: manifest.profile_digest.clone(),
        package_digest: candidate.package_digest.clone(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistryCacheIdentity {
    skill_id: Option<String>,
    version: Option<String>,
    digest: Option<String>,
    profile_digest: Option<String>,
    package_digest: Option<String>,
}

fn canonicalize_candidate_ref(candidate: &mut InstallCandidate) {
    if let (Some(skill_id), Some(version)) = (&candidate.skill_id, &candidate.version) {
        candidate.r#ref = format!("{skill_id}@{version}");
    }
}

fn cache_identity_digest(
    digest: &str,
    profile_digest: Option<&str>,
    package_digest: Option<&str>,
) -> String {
    sha256_prefixed(
        materialization_digest_marker(digest, profile_digest, package_digest).as_bytes(),
    )
}

fn registry_source_fingerprint(target: &registry::RegistryTarget) -> String {
    let source = target.fingerprint_source();
    sha256_prefixed(source.as_bytes())
        .trim_start_matches("sha256:")
        .chars()
        .take(16)
        .collect()
}

fn official_registry_override(
    env: &BTreeMap<String, String>,
    override_value: Option<&str>,
) -> String {
    override_value
        .or_else(|| env.get("RUNX_REGISTRY_URL").map(String::as_str))
        .or_else(|| env.get("RUNX_REGISTRY_DIR").map(String::as_str))
        .unwrap_or("https://runx.ai")
        .to_owned()
}

// Function rationale: asset sync preserves packaged official-skill cache invariants in one pass.
fn sync_packaged_official_skill_assets(
    target_skill_dir: &Path,
    skill_id: &str,
) -> Result<(), String> {
    let Some(packaged_skill_dir) = packaged_official_skill_dir(skill_id)? else {
        return Ok(());
    };
    for entry in fs::read_dir(&packaged_skill_dir).map_err(|error| {
        format!(
            "failed to read packaged official skill {}: {error}",
            packaged_skill_dir.display()
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "failed to read packaged official skill entry {}: {error}",
                packaged_skill_dir.display()
            )
        })?;
        let entry_name = entry.file_name();
        if entry_name == "SKILL.md" {
            continue;
        }
        let source_path = entry.path();
        let target_path = target_skill_dir.join(&entry_name);
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "failed to stat packaged official skill entry {}: {error}",
                source_path.display()
            )
        })?;
        if file_type.is_dir() {
            if target_path.exists() {
                fs::remove_dir_all(&target_path).map_err(|error| {
                    format!(
                        "failed to replace official skill asset directory {}: {error}",
                        target_path.display()
                    )
                })?;
            }
            copy_dir_all(&source_path, &target_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "failed to create official skill asset parent {}: {error}",
                        parent.display()
                    )
                })?;
            }
            fs::copy(&source_path, &target_path).map_err(|error| {
                format!(
                    "failed to copy official skill asset {} to {}: {error}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn packaged_official_skill_dir(skill_id: &str) -> Result<Option<PathBuf>, String> {
    let (_owner, name) = split_skill_id(skill_id).map_err(|error| error.to_string())?;
    Ok(packaged_official_skill_roots()
        .into_iter()
        .map(|root| root.join(name))
        .find(|candidate| candidate.exists()))
}

fn packaged_official_skill_roots() -> Vec<PathBuf> {
    vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills")]
}

fn copy_dir_all(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|error| {
        format!(
            "failed to create official skill asset directory {}: {error}",
            target.display()
        )
    })?;
    for entry in fs::read_dir(source).map_err(|error| {
        format!(
            "failed to read official skill asset directory {}: {error}",
            source.display()
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "failed to read official skill asset entry {}: {error}",
                source.display()
            )
        })?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "failed to stat official skill asset {}: {error}",
                source_path.display()
            )
        })?;
        if file_type.is_dir() {
            copy_dir_all(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path).map_err(|error| {
                format!(
                    "failed to copy official skill asset {} to {}: {error}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn is_bare_skill_ref(skill_ref: &Path) -> bool {
    skill_ref.components().count() == 1
}

fn is_explicit_registry_ref(raw_ref: &str, skill_id: &str) -> bool {
    is_registry_prefixed_ref(raw_ref) || split_skill_id(skill_id).is_ok()
}

fn is_registry_prefixed_ref(raw_ref: &str) -> bool {
    raw_ref.starts_with("registry:")
        || raw_ref.starts_with("runx-registry:")
        || raw_ref.starts_with("runx://skill/")
}

fn local_resolved(kind: SkillRefKind, runnable_path: PathBuf) -> ResolvedSkillRef {
    ResolvedSkillRef {
        kind,
        skill_id: None,
        version: None,
        digest: None,
        profile_digest: None,
        package_digest: None,
        registry_source: None,
        registry_source_fingerprint: None,
        trust_state: None,
        trust_tier: None,
        registry_key_id: None,
        paid_listing: None,
        hosted_registry_url: None,
        runnable_path,
    }
}

fn trust_tier_string(value: &runx_runtime::registry::TrustTier) -> String {
    match value {
        runx_runtime::registry::TrustTier::FirstParty => "first_party",
        runx_runtime::registry::TrustTier::Verified => "verified",
        runx_runtime::registry::TrustTier::Community => "community",
    }
    .to_owned()
}

fn resolve_exported_skill_shim(skill_ref: &Path) -> Result<PathBuf, String> {
    let skill_dir = if skill_ref.is_file() {
        skill_ref.parent().unwrap_or(skill_ref)
    } else {
        skill_ref
    };
    if skill_dir.join(runx_parser::PROFILE_FILE).exists() {
        return Ok(skill_ref.to_path_buf());
    }

    let skill_md = if skill_ref.is_file() {
        skill_ref.to_path_buf()
    } else {
        skill_dir.join("SKILL.md")
    };
    if !skill_md.exists() {
        return Ok(skill_ref.to_path_buf());
    }

    let source = fs::read_to_string(&skill_md)
        .map_err(|error| format!("failed to read {}: {error}", skill_md.display()))?;
    let Some(source_path) = exported_source_path(&source) else {
        return Ok(skill_ref.to_path_buf());
    };
    if source_path.join(runx_parser::PROFILE_FILE).exists() {
        return Ok(source_path);
    }
    Err(format!(
        "exported skill shim {} points at missing or invalid source {}; rerun `runx export`",
        skill_md.display(),
        source_path.display()
    ))
}

fn exported_source_path(source: &str) -> Option<PathBuf> {
    source
        .lines()
        .find(|line| line.contains("runx-export:") && line.contains(" source="))
        .and_then(|line| line.split_once(" source=").map(|(_prefix, value)| value))
        .map(|value| {
            let raw = value.trim().trim_end_matches("-->").trim();
            let raw = raw
                .strip_suffix("- generated, do not edit")
                .unwrap_or(raw)
                .trim();
            PathBuf::from(raw)
        })
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_registry_ref_requires_owner_and_name() {
        assert!(is_explicit_registry_ref("acme/echo", "acme/echo"));
        assert!(is_explicit_registry_ref("registry:acme/echo", "acme/echo"));
        assert!(!is_explicit_registry_ref("./echo", "./echo"));
        assert!(!is_explicit_registry_ref("echo", "echo"));
    }

    #[test]
    fn cache_identity_includes_profile_digest() {
        let without_profile = cache_identity_digest("sha256:abc", None, None);
        let with_profile = cache_identity_digest("sha256:abc", Some("sha256:def"), None);
        assert_ne!(without_profile, with_profile);
        assert!(without_profile.starts_with("sha256:"));
        assert!(with_profile.starts_with("sha256:"));
    }

    #[test]
    fn official_packaged_skill_assets_do_not_use_workspace_shadow_roots() {
        let roots = packaged_official_skill_roots();
        let untrusted_project = Path::new("/tmp/untrusted-project");

        assert!(
            !roots.contains(&PathBuf::from("/tmp/untrusted-project/skills")),
            "official skill companion assets must not be copied from the caller workspace"
        );
        assert!(
            !roots.iter().any(|root| root.starts_with(untrusted_project)),
            "official skill companion assets must not be copied from caller-controlled roots"
        );
    }

    #[test]
    fn unresolved_bare_skill_points_to_search_and_direct_registry_run() -> Result<(), String> {
        let env = BTreeMap::new();
        let error = match resolve_skill_ref_details(
            Path::new("missing-skill"),
            Path::new("/tmp/runx-cli-resolver-test"),
            SkillResolverOptions {
                env: &env,
                registry: None,
                expected_digest: None,
            },
        ) {
            Ok(_) => return Err("missing bare skill should fail".to_owned()),
            Err(error) => error,
        };

        assert!(error.contains("runx registry search missing-skill"));
        assert!(error.contains("runx skill <owner>/<name>@<version>"));
        Ok(())
    }
}
