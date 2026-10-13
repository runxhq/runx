use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use runx_contracts::{ContextEntry, JsonObject, JsonValue};

use crate::RuntimeError;
use crate::registry::{
    FileRegistryStore, RegistryResolveOptions, RegistrySkillResolution, resolve_registry_skill,
};

mod catalog;
mod entry;

use catalog::validate_context_manifest;
use entry::{SkillContextEntryInput, insert_string, skill_context_entry};

// Context is an explicit skill-chain dependency, not an inline convenience
// field. Keep one aggregate safety boundary large enough for substantive
// manuals and examples; never truncate an individual skill to make it fit.
const MAX_CONTEXT_SKILLS: usize = 128;
const MAX_CONTEXT_SKILLS_TOTAL_BYTES: usize = 4 * 1024 * 1024;

pub fn load_context_skills(
    step_id: &str,
    graph_dir: &Path,
    refs: &[String],
    env: &BTreeMap<String, String>,
    created_at: &str,
) -> Result<Vec<ContextEntry>, RuntimeError> {
    if refs.len() > MAX_CONTEXT_SKILLS {
        return Err(RuntimeError::InvalidRunStep {
            step_id: step_id.to_owned(),
            reason: format!(
                "context_skills declares {} skills; the maximum is {MAX_CONTEXT_SKILLS}",
                refs.len()
            ),
        });
    }

    let mut seen = BTreeSet::new();
    let mut total_bytes = 0usize;
    refs.iter()
        .map(|reference| {
            if !seen.insert(reference.as_str()) {
                return Err(RuntimeError::InvalidRunStep {
                    step_id: step_id.to_owned(),
                    reason: format!("context skill '{reference}' is declared more than once"),
                });
            }
            let entry = load_context_skill(step_id, graph_dir, reference, env, created_at)?;
            let entry_bytes =
                usize::try_from(entry.meta.size_bytes).map_err(|_| RuntimeError::InvalidRunStep {
                    step_id: step_id.to_owned(),
                    reason: format!(
                        "context skill '{reference}' size exceeds this platform's address space"
                    ),
                })?;
            total_bytes =
                total_bytes
                    .checked_add(entry_bytes)
                    .ok_or_else(|| RuntimeError::InvalidRunStep {
                        step_id: step_id.to_owned(),
                        reason: "context_skills total size overflowed".to_owned(),
                    })?;
            if total_bytes > MAX_CONTEXT_SKILLS_TOTAL_BYTES {
                return Err(RuntimeError::InvalidRunStep {
                    step_id: step_id.to_owned(),
                    reason: format!(
                        "context_skills resolved to more than {MAX_CONTEXT_SKILLS_TOTAL_BYTES} bytes"
                    ),
                });
            }
            Ok(entry)
        })
        .collect()
}

fn load_context_skill(
    step_id: &str,
    graph_dir: &Path,
    reference: &str,
    env: &BTreeMap<String, String>,
    created_at: &str,
) -> Result<ContextEntry, RuntimeError> {
    if runx_parser::registry_ref(reference).is_some() {
        return load_registry_context_skill(step_id, reference, env, created_at);
    }
    load_local_context_skill(step_id, graph_dir, reference, env, created_at)
}

fn load_local_context_skill(
    step_id: &str,
    graph_dir: &Path,
    reference: &str,
    env: &BTreeMap<String, String>,
    created_at: &str,
) -> Result<ContextEntry, RuntimeError> {
    validate_local_context_ref(step_id, reference)?;
    let skill_dir = graph_dir.join(reference);
    let package = crate::load_validated_skill_package(&skill_dir)?;
    let skill_path = package.package_root.join("SKILL.md");
    let canonical_skill_path = skill_path.canonicalize().map_err(|source| {
        RuntimeError::io(
            format!("canonicalizing context skill {}", skill_path.display()),
            source,
        )
    })?;
    validate_context_manifest(step_id, reference, package.manifest())?;
    let mut data = JsonObject::new();
    insert_string(&mut data, "ref", reference);
    insert_string(&mut data, "source", "local-path");
    insert_string(&mut data, "security_boundary", "untrusted-agent-context");
    insert_string(&mut data, "name", &package.package.skill.name);
    if let Some(description) = &package.package.skill.description {
        insert_string(&mut data, "description", description);
    }
    let skill_path_display = canonical_skill_path.to_string_lossy();
    insert_string(&mut data, "path", skill_path_display.as_ref());
    if let Some(profile_relative) = package.profile_path.as_deref() {
        let profile = package
            .package
            .source
            .files
            .get(profile_relative)
            .ok_or_else(|| {
                runx_parser::SkillPackageError::invalid(
                    profile_relative,
                    "validated context profile source is missing",
                )
            })?;
        let profile_path = package.package_root.join(profile_relative);
        let profile_path = profile_path.canonicalize().map_err(|source| {
            RuntimeError::io(
                format!("canonicalizing context profile {}", profile_path.display()),
                source,
            )
        })?;
        insert_string(
            &mut data,
            "profile_path",
            profile_path.to_string_lossy().as_ref(),
        );
        insert_string(
            &mut data,
            "profile_sha256",
            &runx_contracts::sha256_prefixed(profile),
        );
    }
    insert_catalog_summary(&mut data, package.manifest())?;
    let entry = manual_context_entry(
        step_id,
        reference,
        env,
        created_at,
        &package.package.manual_digest,
        &package.package.manual_markdown,
        data,
    )?;
    super::prepared_skill::verify_prepared_artifact_at_use(env, &canonical_skill_path)?;
    Ok(entry)
}

fn load_registry_context_skill(
    step_id: &str,
    reference: &str,
    env: &BTreeMap<String, String>,
    created_at: &str,
) -> Result<ContextEntry, RuntimeError> {
    let (resolution, package) = resolve_registry_context_skill(step_id, reference, env)?;
    validate_context_manifest(step_id, reference, package.root_manifest())?;
    let mut data = JsonObject::new();
    insert_string(&mut data, "ref", reference);
    insert_string(&mut data, "source", &resolution.source);
    insert_string(&mut data, "security_boundary", "untrusted-agent-context");
    insert_string(&mut data, "source_label", &resolution.source_label);
    insert_string(&mut data, "skill_id", &resolution.skill_id);
    insert_string(&mut data, "name", &resolution.name);
    insert_string(&mut data, "version", &resolution.version);
    insert_string(&mut data, "digest", &resolution.digest);
    insert_string(&mut data, "trust_tier", resolution.trust_tier.as_str());
    if let Some(profile_digest) = &resolution.profile_digest {
        insert_string(
            &mut data,
            "profile_sha256",
            &prefixed_registry_digest(profile_digest),
        );
    }
    if let Some(package_digest) = &resolution.package_digest {
        insert_string(
            &mut data,
            "package_sha256",
            &prefixed_registry_digest(package_digest),
        );
    }
    if let Some(description) = &package.skill.description {
        insert_string(&mut data, "description", description);
    }
    insert_catalog_summary(&mut data, package.root_manifest())?;
    manual_context_entry(
        step_id,
        reference,
        env,
        created_at,
        &package.manual_digest,
        &package.manual_markdown,
        data,
    )
}

fn resolve_registry_context_skill(
    step_id: &str,
    reference: &str,
    env: &BTreeMap<String, String>,
) -> Result<(RegistrySkillResolution, runx_parser::ValidatedSkillPackage), RuntimeError> {
    let paths = crate::paths::RunxPaths::resolve(env, Path::new("."));
    let Some(registry_dir) = paths.registry_dir() else {
        return Err(RuntimeError::InvalidRunStep {
            step_id: step_id.to_owned(),
            reason: format!(
                "context skill '{reference}' is a registry ref, but RUNX_REGISTRY_DIR is not configured"
            ),
        });
    };
    let store = FileRegistryStore::new(registry_dir);
    let registry_url = env.get("RUNX_REGISTRY_URL").cloned();
    let resolution = resolve_registry_skill(
        &store,
        reference,
        RegistryResolveOptions {
            version: None,
            registry_url,
        },
    )
    .map_err(|error| RuntimeError::InvalidRunStep {
        step_id: step_id.to_owned(),
        reason: format!("context skill registry ref '{reference}' could not be resolved: {error}"),
    })?
    .ok_or_else(|| RuntimeError::InvalidRunStep {
        step_id: step_id.to_owned(),
        reason: format!("context skill registry ref '{reference}' was not found"),
    })?;

    let mut source = runx_parser::SkillPackageSource::from_documents(
        resolution.markdown.clone(),
        resolution.profile_document.clone(),
    );
    for file in &resolution.package_files {
        source
            .files
            .insert(file.path.clone(), file.content.as_bytes().to_vec());
    }
    let package = runx_parser::validate_skill_package(source)?;
    Ok((resolution, package))
}

fn insert_catalog_summary(
    data: &mut JsonObject,
    manifest: Option<&runx_parser::SkillRunnerManifest>,
) -> Result<(), RuntimeError> {
    let Some(catalog) = manifest.and_then(|manifest| manifest.catalog.as_ref()) else {
        return Ok(());
    };
    let catalog = serde_json::to_value(catalog)
        .and_then(serde_json::from_value)
        .map_err(|source| RuntimeError::json("serializing skill context catalog", source))?;
    data.insert("catalog".to_owned(), catalog);
    Ok(())
}

fn manual_context_entry(
    step_id: &str,
    reference: &str,
    env: &BTreeMap<String, String>,
    created_at: &str,
    manual_digest: &str,
    manual_markdown: &str,
    mut data: JsonObject,
) -> Result<ContextEntry, RuntimeError> {
    insert_string(&mut data, "content_kind", "skill-manual");
    insert_string(&mut data, "manual_sha256", manual_digest);
    insert_string(&mut data, "content", manual_markdown);
    let canonical = runx_contracts::canonical_stable_json(&JsonValue::Object(data.clone()))
        .map_err(|error| RuntimeError::ReceiptInvalid {
            message: format!("skill context artifact could not be canonicalized: {error}"),
        })?;
    let digest = runx_contracts::sha256_prefixed(canonical.as_bytes());
    skill_context_entry(SkillContextEntryInput {
        step_id,
        reference,
        env,
        created_at,
        digest: &digest,
        size_bytes: canonical.len() as u64,
        data,
    })
}

fn validate_local_context_ref(step_id: &str, reference: &str) -> Result<(), RuntimeError> {
    if reference.trim().is_empty() {
        return Err(RuntimeError::InvalidRunStep {
            step_id: step_id.to_owned(),
            reason: "context skill ref must not be empty".to_owned(),
        });
    }
    let path = Path::new(reference);
    if path.is_absolute() {
        return Err(RuntimeError::InvalidRunStep {
            step_id: step_id.to_owned(),
            reason: format!("context skill '{reference}' must be a relative path or registry ref"),
        });
    }
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(RuntimeError::InvalidRunStep {
                    step_id: step_id.to_owned(),
                    reason: format!("context skill '{reference}' must not contain '..'"),
                });
            }
            Component::Normal(name) if name == "graph" => {
                return Err(RuntimeError::InvalidRunStep {
                    step_id: step_id.to_owned(),
                    reason: format!("context skill '{reference}' must not target graph stages"),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(RuntimeError::InvalidRunStep {
                    step_id: step_id.to_owned(),
                    reason: format!("context skill '{reference}' must be a relative path"),
                });
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

fn prefixed_registry_digest(digest: &str) -> String {
    if digest.starts_with("sha256:") {
        digest.to_owned()
    } else {
        format!("sha256:{digest}")
    }
}
