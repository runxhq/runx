// Module rationale: skill resolution centralizes explicit, bundled, installed, and verified registry packages.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::PaidSkillListing;
use runx_contracts::sha256_prefixed;
use runx_runtime::registry::{
    InstallCandidate, InstallLocalSkillOptions, parse_registry_ref, split_skill_id,
};

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
    Bundled,
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
    let local_path = runx_runtime::RunxPaths::resolve(options.env, cwd).existing_input(skill_ref);
    if local_path.exists() {
        let path = resolve_exported_skill_shim(&local_path)?;
        let kind = if path == local_path {
            SkillRefKind::ExplicitPath
        } else {
            SkillRefKind::ExportedShim
        };
        return Ok(local_resolved(kind, path));
    }

    let raw_ref = skill_ref.to_string_lossy();
    let parsed = parse_registry_ref(&raw_ref);
    if runx_parser::registry_ref(&raw_ref).is_some() {
        if split_skill_id(&parsed.skill_id).is_err() {
            return Err(format!(
                "Registry ref '{}' is ambiguous. Use '<owner>/<name>' instead.",
                raw_ref
            ));
        }
        return resolve_registry_skill(&parsed.skill_id, parsed.version.as_deref(), cwd, options);
    }

    if let Some(resolved) = resolve_installed_or_workspace_skill(&raw_ref, cwd, &options)? {
        return Ok(resolved);
    }

    if is_bare_skill_ref(skill_ref) {
        return resolve_bare_skill_ref(skill_ref, &raw_ref, cwd, options);
    }

    if is_explicit_registry_ref(&raw_ref, &parsed.skill_id) {
        if let Some(resolved) =
            resolve_bundled_skill(&parsed.skill_id, parsed.version.as_deref(), cwd, &options)?
        {
            return Ok(resolved);
        }
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
    let parsed = parse_registry_ref(raw_ref);
    if let Some(resolved) =
        resolve_bundled_skill(&parsed.skill_id, parsed.version.as_deref(), cwd, &options)?
    {
        return Ok(resolved);
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

fn resolve_bundled_skill(
    skill_id: &str,
    version: Option<&str>,
    cwd: &Path,
    options: &SkillResolverOptions<'_>,
) -> Result<Option<ResolvedSkillRef>, String> {
    let paths = runx_runtime::RunxPaths::resolve(options.env, cwd);
    let reference = version.map_or_else(
        || skill_id.to_owned(),
        |version| format!("{skill_id}@{version}"),
    );
    let Some(installed) = runx_runtime::registry::materialize_bundled_skill(
        &reference,
        &paths,
        options.expected_digest,
    )?
    else {
        return Ok(None);
    };
    let runnable_path = installed
        .destination
        .parent()
        .ok_or_else(|| {
            format!(
                "bundled skill has invalid destination {}",
                installed.destination.display()
            )
        })?
        .to_path_buf();
    let mut resolved = local_resolved(SkillRefKind::Bundled, runnable_path);
    resolved.skill_id = installed.skill_id;
    resolved.version = installed.version;
    resolved.digest = Some(installed.digest);
    resolved.profile_digest = installed.profile_digest;
    Ok(Some(resolved))
}

fn resolve_installed_or_workspace_skill(
    name: &str,
    cwd: &Path,
    options: &SkillResolverOptions<'_>,
) -> Result<Option<ResolvedSkillRef>, String> {
    let paths = runx_runtime::RunxPaths::resolve(options.env, cwd);
    let Some(candidate) = runx_runtime::resolve_local_skill_reference(
        &paths,
        Path::new(name),
        &paths.skill_roots(options.env),
    )
    .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let path = resolve_exported_skill_shim(&candidate)?;
    let kind = if path != candidate {
        SkillRefKind::ExportedShim
    } else if candidate.starts_with(cwd.join("skills")) {
        SkillRefKind::WorkspaceLocal
    } else {
        SkillRefKind::Installed
    };
    Ok(Some(local_resolved(kind, path)))
}

// Function rationale: registry materialization is kept atomic around trust, cache, and digest checks.
fn resolve_registry_skill(
    skill_id: &str,
    version: Option<&str>,
    cwd: &Path,
    options: SkillResolverOptions<'_>,
) -> Result<ResolvedSkillRef, String> {
    let env = options.env;
    let plan = RegistryPlan {
        action: RegistryAction::Install,
        subject: skill_id.to_owned(),
        registry: options.registry.map(ToOwned::to_owned),
        registry_dir: None,
        version: version.map(ToOwned::to_owned),
        expected_digest: options.expected_digest.map(ToOwned::to_owned),
        publisher: None,
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
    canonicalize_candidate_ref(&mut candidate);
    let identity = registry_identity(&candidate)?;
    // A fetched skill is added to the project's skills directory, once per
    // version, like any other.
    let destination_root = runx_runtime::RunxPaths::resolve(env, cwd).skills();
    let install = runx_runtime::registry::install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: destination_root.to_path_buf(),
            connectors_root: runx_runtime::RunxPaths::resolve(env, cwd).connectors(),
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
    Ok(ResolvedSkillRef {
        kind: SkillRefKind::Registry,
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

fn registry_identity(candidate: &InstallCandidate) -> Result<RegistryIdentity, String> {
    let manifest = candidate.signed_manifest.as_ref().ok_or_else(|| {
        format!(
            "registry signed manifest is required for {}",
            candidate.r#ref
        )
    })?;
    Ok(RegistryIdentity {
        skill_id: candidate
            .skill_id
            .clone()
            .or_else(|| Some(manifest.subject.subject_ref.clone())),
        version: candidate
            .version
            .clone()
            .or_else(|| Some(manifest.version.clone())),
    })
}

/// The skill and version a registry candidate's signed manifest names.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistryIdentity {
    skill_id: Option<String>,
    version: Option<String>,
}

fn canonicalize_candidate_ref(candidate: &mut InstallCandidate) {
    if let (Some(skill_id), Some(version)) = (&candidate.skill_id, &candidate.version) {
        candidate.r#ref = format!("{skill_id}@{version}");
    }
}

fn registry_source_fingerprint(target: &registry::RegistryTarget) -> String {
    let source = target.fingerprint_source();
    sha256_prefixed(source.as_bytes())
        .trim_start_matches("sha256:")
        .chars()
        .take(16)
        .collect()
}

fn is_bare_skill_ref(skill_ref: &Path) -> bool {
    skill_ref.components().count() == 1
}

fn is_explicit_registry_ref(raw_ref: &str, skill_id: &str) -> bool {
    runx_parser::registry_ref(raw_ref).is_some() || split_skill_id(skill_id).is_ok()
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
