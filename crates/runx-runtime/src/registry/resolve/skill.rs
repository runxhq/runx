// Module rationale: skill resolution centralizes explicit, bundled, installed, and verified registry packages.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::PaidSkillListing;
use runx_contracts::sha256_prefixed;

use super::{RegistryTarget, ResolveError, registry_source_description, resolve_registry_target};
use crate::registry::{
    InstallCandidate, InstallLocalSkillOptions, parse_registry_ref, split_skill_id,
};

#[derive(Clone, Copy)]
pub struct SkillResolverOptions<'a> {
    pub env: &'a BTreeMap<String, String>,
    /// The registry `--registry` names, if one does.
    pub registry: Option<&'a str>,
    pub expected_digest: Option<&'a str>,
    pub absent_pins: AbsentPins,
}

/// What a nested registry pin the registry does not hold means. A run cannot
/// proceed without it. An inspection reports it unresolved, so a caller that
/// supplies registry records, as the hosted registry closure does, can supply
/// it and inspect again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbsentPins {
    Fail,
    Report,
}

/// Why a registry skill did not resolve: the registry holds no such version,
/// or finding or installing the one it holds failed.
enum RegistryResolveError {
    NotHeld(String),
    Failed(String),
}

impl From<String> for RegistryResolveError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<ResolveError> for RegistryResolveError {
    fn from(error: ResolveError) -> Self {
        Self::Failed(error.into_message())
    }
}

impl From<RegistryResolveError> for String {
    fn from(error: RegistryResolveError) -> Self {
        match error {
            RegistryResolveError::NotHeld(message) | RegistryResolveError::Failed(message) => {
                message
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkillRefKind {
    ExplicitPath,
    ExportedShim,
    WorkspaceLocal,
    Installed,
    Bundled,
    Registry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryTrustState {
    Trusted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSkillRef {
    pub kind: SkillRefKind,
    pub skill_id: Option<String>,
    pub version: Option<String>,
    pub digest: Option<String>,
    pub profile_digest: Option<String>,
    pub package_digest: Option<String>,
    pub registry_source: Option<String>,
    pub registry_source_fingerprint: Option<String>,
    pub trust_state: Option<RegistryTrustState>,
    pub trust_tier: Option<String>,
    pub registry_key_id: Option<String>,
    pub paid_listing: Option<PaidSkillListing>,
    pub hosted_registry_url: Option<String>,
    pub runnable_path: PathBuf,
}

/// A graph nests skills at most this deep; deeper is a cycle, not a graph.
const MAX_NESTED_FETCH_DEPTH: usize = 16;

/// The package `skill_ref` names, with every registry skill it names
/// fetched into the project.
///
/// # Errors
///
/// Returns why the ref, or a registry skill its package names, did not
/// resolve.
pub fn resolve_skill_ref_details(
    skill_ref: &Path,
    cwd: &Path,
    options: SkillResolverOptions<'_>,
) -> Result<ResolvedSkillRef, String> {
    let resolved = resolve_skill_ref(skill_ref, cwd, options)?;
    fetch_unheld_registry_refs(&resolved.runnable_path, cwd, options)?;
    Ok(resolved)
}

/// A skill arrives when run, and so does every registry skill its runners
/// name: each one the project does not hold is fetched exactly as a top-level
/// ref is, into the project's skills directory, until the closure holds them
/// all, since a fetched skill can name its own. With `AbsentPins::Report`, a
/// pin the registry does not hold stays unfetched for the inspection to report.
fn fetch_unheld_registry_refs(
    skill_path: &Path,
    cwd: &Path,
    options: SkillResolverOptions<'_>,
) -> Result<(), String> {
    let mut absent = BTreeSet::new();
    for _ in 0..=MAX_NESTED_FETCH_DEPTH {
        let unheld = crate::unheld_registry_refs_at(skill_path, options.env)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|reference| !absent.contains(reference))
            .collect::<Vec<_>>();
        if unheld.is_empty() {
            return Ok(());
        }
        for reference in unheld {
            let parsed =
                parse_registry_ref(runx_parser::registry_ref(&reference).unwrap_or(&reference));
            // A package binds an exact closure, so it names the canonical id;
            // a root name may move and is never one.
            split_skill_id(&parsed.skill_id).map_err(|_| {
                format!(
                    "fetching {reference}: a package names a registry skill as '<owner>/<name>'"
                )
            })?;
            match resolve_registry_skill(
                &parsed.skill_id,
                parsed.version.as_deref(),
                cwd,
                SkillResolverOptions {
                    expected_digest: None,
                    ..options
                },
            ) {
                Ok(_) => {}
                Err(RegistryResolveError::NotHeld(_))
                    if options.absent_pins == AbsentPins::Report =>
                {
                    absent.insert(reference);
                }
                Err(error) => {
                    return Err(format!("fetching {reference}: {}", String::from(error)));
                }
            }
        }
    }
    Err(format!(
        "{} names registry skills more than {MAX_NESTED_FETCH_DEPTH} levels deep",
        skill_path.display()
    ))
}

fn resolve_skill_ref(
    skill_ref: &Path,
    cwd: &Path,
    options: SkillResolverOptions<'_>,
) -> Result<ResolvedSkillRef, String> {
    let local_path = crate::RunxPaths::resolve(options.env, cwd).existing_input(skill_ref);
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
                "Registry ref '{raw_ref}' names no publisher. A registry ref is '<owner>/<name>'; run a root name as `runx skill {}`.",
                parsed.skill_id
            ));
        }
        return resolve_registry_skill(&parsed.skill_id, parsed.version.as_deref(), cwd, options)
            .map_err(String::from);
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
        return resolve_registry_skill(&parsed.skill_id, parsed.version.as_deref(), cwd, options)
            .map_err(String::from);
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
    // Then the registry, which a bare name reaches only as a root name.
    if let Some(skill_id) = super::resolve_root_name(
        &parsed.skill_id,
        parsed.version.as_deref(),
        options.registry,
        options.env,
        cwd,
    )
    .map_err(ResolveError::into_message)?
    {
        return resolve_registry_skill(&skill_id, parsed.version.as_deref(), cwd, options)
            .map_err(String::from);
    }
    Err(format!(
        "could not resolve skill ref '{}'; tried {}, {} and the registry's root names. Search with `runx registry search {}` or run a registry ref directly with `runx skill <owner>/<name>@<version>`.",
        skill_ref.display(),
        cwd.join("skills").join(skill_ref).display(),
        crate::RunxPaths::resolve(options.env, cwd)
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
    let paths = crate::RunxPaths::resolve(options.env, cwd);
    let reference = version.map_or_else(
        || skill_id.to_owned(),
        |version| format!("{skill_id}@{version}"),
    );
    let Some(installed) =
        crate::registry::materialize_bundled_skill(&reference, &paths, options.expected_digest)?
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
    let paths = crate::RunxPaths::resolve(options.env, cwd);
    let Some(candidate) = crate::resolve_local_skill_reference(
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
) -> Result<ResolvedSkillRef, RegistryResolveError> {
    let env = options.env;
    let requested = version.map_or_else(
        || skill_id.to_owned(),
        |version| format!("{skill_id}@{version}"),
    );
    let target = resolve_registry_target(options.registry, env, cwd);
    let source_description = registry_source_description(&target);
    let hosted_registry_url = match &target {
        RegistryTarget::Remote { registry_url } => Some(registry_url.clone()),
        RegistryTarget::Local { .. } => None,
    };
    let source_fingerprint = registry_source_fingerprint(&target);
    let source_authority = target.manifest_source_authority();
    let (mut candidate, acquisition) =
        super::install_candidate(skill_id, version, target, env, cwd)?.ok_or_else(|| {
            RegistryResolveError::NotHeld(format!(
                "registry skill not found: {requested} in {source_description}"
            ))
        })?;
    let paid_listing = acquisition
        .as_ref()
        .and_then(|acquired| acquired.paid_listing.clone());
    canonicalize_candidate_ref(&mut candidate);
    let identity = registry_identity(&candidate)?;
    // A fetched skill is added to the project's skills directory, once per
    // version, like any other.
    let destination_root = crate::RunxPaths::resolve(env, cwd).skills();
    let install = crate::registry::install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root: destination_root.to_path_buf(),
            connectors_root: crate::RunxPaths::resolve(env, cwd).connectors(),
            install_records: crate::RunxPaths::resolve(env, cwd).install_records(),
            expected_digest: options.expected_digest.map(ToOwned::to_owned),
            trusted_manifest_keys: super::trusted_manifest_keys_for_source(
                env,
                candidate
                    .manifest_source_authority
                    .clone()
                    .unwrap_or(source_authority),
            )?,
        },
    )
    .map_err(ResolveError::from)?;
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
        trust_tier: install.trust_tier.map(|tier| tier.as_str().to_owned()),
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

fn registry_source_fingerprint(target: &RegistryTarget) -> String {
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
                absent_pins: AbsentPins::Fail,
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
