use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use runx_contracts::{ContextEntry, JsonObject, JsonValue};

use crate::RuntimeError;

mod entry;

use entry::{SkillContextEntryInput, insert_string, skill_context_entry};

// Context is an explicit skill-chain dependency, not an inline convenience
// field. Keep one aggregate safety boundary large enough for substantive
// manuals and examples; never truncate an individual skill to make it fit.
const MAX_CONTEXT_SKILLS: usize = 128;
const MAX_CONTEXT_SKILLS_TOTAL_BYTES: usize = 4 * 1024 * 1024;

pub fn load_context_skills(
    step_id: &str,
    workflow_dir: &Path,
    refs: &[String],
    env: &BTreeMap<String, String>,
    created_at: &str,
) -> Result<Vec<ContextEntry>, RuntimeError> {
    if refs.len() > MAX_CONTEXT_SKILLS {
        return Err(RuntimeError::StepFailed {
            step_id: step_id.to_owned(),
            reason: format!(
                "context.skills declares {} skills; the maximum is {MAX_CONTEXT_SKILLS}",
                refs.len()
            ),
        });
    }

    let mut seen = BTreeSet::new();
    let mut total_bytes = 0usize;
    refs.iter()
        .map(|reference| {
            if !seen.insert(reference.as_str()) {
                return Err(RuntimeError::StepFailed {
                    step_id: step_id.to_owned(),
                    reason: format!("context skill '{reference}' is declared more than once"),
                });
            }
            let entry = load_context_skill(step_id, workflow_dir, reference, env, created_at)?;
            let entry_bytes =
                usize::try_from(entry.meta.size_bytes).map_err(|_| RuntimeError::StepFailed {
                    step_id: step_id.to_owned(),
                    reason: format!(
                        "context skill '{reference}' size exceeds this platform's address space"
                    ),
                })?;
            total_bytes =
                total_bytes
                    .checked_add(entry_bytes)
                    .ok_or_else(|| RuntimeError::StepFailed {
                        step_id: step_id.to_owned(),
                        reason: "context.skills total size overflowed".to_owned(),
                    })?;
            if total_bytes > MAX_CONTEXT_SKILLS_TOTAL_BYTES {
                return Err(RuntimeError::StepFailed {
                    step_id: step_id.to_owned(),
                    reason: format!(
                        "context.skills resolved to more than {MAX_CONTEXT_SKILLS_TOTAL_BYTES} bytes"
                    ),
                });
            }
            Ok(entry)
        })
        .collect()
}

fn load_context_skill(
    step_id: &str,
    workflow_dir: &Path,
    reference: &str,
    env: &BTreeMap<String, String>,
    created_at: &str,
) -> Result<ContextEntry, RuntimeError> {
    if runx_parser::registry_ref(reference).is_some() {
        let paths = crate::paths::RunxPaths::resolve(env, workflow_dir);
        if let Some(directory) =
            crate::execution::graph::installed_registry_skill(reference, &paths)?
        {
            return load_context_skill_directory(
                step_id, reference, &directory, true, env, created_at,
            );
        }
        return Err(RuntimeError::StepFailed {
            step_id: step_id.to_owned(),
            reason: format!(
                "context skill '{reference}' is not installed in this project; `runx skill` fetches it before the run"
            ),
        });
    }
    validate_local_context_ref(step_id, reference)?;
    load_context_skill_directory(
        step_id,
        reference,
        &workflow_dir.join(reference),
        false,
        env,
        created_at,
    )
}

/// Whether the project holds the context skill a registry ref names.
pub(crate) fn registry_holds_context_skill(
    reference: &str,
    env: &BTreeMap<String, String>,
) -> Result<bool, RuntimeError> {
    let paths = crate::paths::RunxPaths::resolve(env, Path::new("."));
    Ok(crate::execution::graph::installed_registry_skill(reference, &paths)?.is_some())
}

/// A context skill read from a package directory: a local path, or the
/// installed version of a registry ref, which reads only once its signature
/// verifies at the version the ref pins, and keeps the provenance it states.
fn load_context_skill_directory(
    step_id: &str,
    reference: &str,
    skill_dir: &Path,
    installed: bool,
    env: &BTreeMap<String, String>,
    created_at: &str,
) -> Result<ContextEntry, RuntimeError> {
    let package = crate::load_validated_skill_package(skill_dir)?;
    let skill_path = package.package_root.join("SKILL.md");
    let canonical_skill_path = skill_path.canonicalize().map_err(|source| {
        RuntimeError::io(
            format!("canonicalizing context skill {}", skill_path.display()),
            source,
        )
    })?;
    let signed = installed
        .then(|| crate::identity::registry_copy_identity(reference, &package, env))
        .transpose()
        .map_err(|reason| RuntimeError::StepFailed {
            step_id: step_id.to_owned(),
            reason,
        })?;
    let mut data = JsonObject::new();
    insert_string(&mut data, "skill", reference);
    match &signed {
        Some(signed) => {
            insert_string(&mut data, "source", "runx-registry");
            insert_string(&mut data, "source_label", "runx registry");
            insert_string(&mut data, "skill_id", &signed.name);
            insert_string(&mut data, "version", &signed.version);
            insert_string(&mut data, "digest", &package.package.manual_digest);
            if let Some(bundle_digest) = crate::execution::graph::package_files_digest(&package) {
                insert_string(&mut data, "bundle_digest", &bundle_digest);
            }
        }
        None => insert_string(&mut data, "source", "local-path"),
    }
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
            "profile_digest",
            &runx_contracts::sha256_prefixed(profile),
        );
    }
    insert_catalog_summary(&mut data, package.manifest())?;
    let entry = manual_context_entry(step_id, reference, env, created_at, &package.package, data)?;
    super::prepared_skill::verify_prepared_artifact_at_use(env, &canonical_skill_path)?;
    Ok(entry)
}

fn insert_catalog_summary(
    data: &mut JsonObject,
    manifest: Option<&runx_parser::SkillMethodManifest>,
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

/// The context entry of a context skill: its manual, with what describes it,
/// summarised for an index as the skill's name and description.
fn manual_context_entry(
    step_id: &str,
    reference: &str,
    env: &BTreeMap<String, String>,
    created_at: &str,
    package: &runx_parser::ValidatedSkillPackage,
    mut data: JsonObject,
) -> Result<ContextEntry, RuntimeError> {
    insert_string(&mut data, "content_kind", "skill-manual");
    insert_string(&mut data, "manual_digest", &package.manual_digest);
    insert_string(&mut data, "content", &package.manual_markdown);
    let skill = &package.skill;
    let summary = match &skill.description {
        Some(description) => format!("Context skill {}: {description}", skill.name),
        None => format!("Context skill {}", skill.name),
    };
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
        summary,
        data,
    })
}

fn validate_local_context_ref(step_id: &str, reference: &str) -> Result<(), RuntimeError> {
    if reference.trim().is_empty() {
        return Err(RuntimeError::StepFailed {
            step_id: step_id.to_owned(),
            reason: "context skill ref must not be empty".to_owned(),
        });
    }
    let path = Path::new(reference);
    if path.is_absolute() {
        return Err(RuntimeError::StepFailed {
            step_id: step_id.to_owned(),
            reason: format!("context skill '{reference}' must be a relative path or registry ref"),
        });
    }
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(RuntimeError::StepFailed {
                    step_id: step_id.to_owned(),
                    reason: format!("context skill '{reference}' must not contain '..'"),
                });
            }
            Component::Normal(name) if name == "graph" => {
                return Err(RuntimeError::StepFailed {
                    step_id: step_id.to_owned(),
                    reason: format!("context skill '{reference}' must not target workflow stages"),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(RuntimeError::StepFailed {
                    step_id: step_id.to_owned(),
                    reason: format!("context skill '{reference}' must be a relative path"),
                });
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}
