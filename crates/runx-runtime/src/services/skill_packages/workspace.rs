use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use runx_contracts::{
    Digest, JsonObject, JsonValue, SkillAuthoringContext, SkillCatalogEntry, SkillPackageFile,
    SkillPackageMetrics,
};

use super::MAX_PACKAGE_FILES;
use super::path::{
    canonical_directory, display_relative, display_repo_path, ignored_package_entry,
    invalid_skill_change, normalize_child_path,
};
use super::snapshot::{absent_package_digest, package_metrics, package_snapshot};
use crate::filesystem::read_dir_sorted;
use crate::{RuntimeError, RunxListItem, RunxListOptions, RunxListRequestedKind};

pub(crate) fn inspect_skill_workspace(
    repo_root: &Path,
    target_dir: Option<&str>,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<SkillAuthoringContext, RuntimeError> {
    let repo_root = canonical_directory(repo_root, "skill workspace")?;
    let target_dir = target_dir.map(normalize_child_path).transpose()?;
    let target_root = target_dir.as_ref().map(|path| repo_root.join(path));
    let target_exists = target_root.as_ref().is_some_and(|path| path.is_dir());
    let (target_files, target_metrics, base_digest) =
        inspect_target(target_root.as_deref(), target_exists)?;
    let target_inspection =
        inspect_target_package(&repo_root, target_root.as_deref(), target_exists);
    let catalog_root = if repo_root.join("skills").is_dir() {
        "skills"
    } else {
        "."
    };
    let catalog_skills = catalog_inventory(&repo_root, RunxListRequestedKind::Skills)?;
    let core_tools = core_tool_inventory(&repo_root, effects)?;

    Ok(SkillAuthoringContext {
        repo_root: repo_root.to_string_lossy().into_owned().into(),
        target_dir: target_dir
            .as_deref()
            .map(|path| display_relative(path).into()),
        target_exists,
        base_digest,
        target_files,
        target_metrics,
        target_inspection,
        catalog_root: catalog_root.into(),
        catalog_skills,
        core_tools,
    })
}

fn inspect_target_package(
    repo_root: &Path,
    target_root: Option<&Path>,
    target_exists: bool,
) -> Option<JsonObject> {
    if !target_exists {
        return None;
    }
    let Some(target_root) = target_root else {
        return Some(invalid_target_inspection(
            "existing skill target has no resolved workspace path",
        ));
    };
    let env = std::collections::BTreeMap::from([(
        crate::paths::RUNX_CWD_ENV.to_owned(),
        repo_root.to_string_lossy().into_owned(),
    )]);
    Some(
        match crate::inspect_skill_package(target_root, None, Some(&env))
            .map_err(|error| error.to_string())
            .and_then(|inspection| {
                serde_json::to_value(&inspection)
                    .and_then(serde_json::from_value::<JsonObject>)
                    .map_err(|error| error.to_string())
            }) {
            Ok(inspection) => inspection,
            Err(error) => invalid_target_inspection(&error),
        },
    )
}

fn invalid_target_inspection(error: &str) -> JsonObject {
    JsonObject::from([
        ("status".to_owned(), JsonValue::String("invalid".to_owned())),
        ("error".to_owned(), JsonValue::String(error.to_owned())),
    ])
}

fn inspect_target(
    target_root: Option<&Path>,
    target_exists: bool,
) -> Result<(Vec<SkillPackageFile>, SkillPackageMetrics, Digest), RuntimeError> {
    let target_files = if target_exists {
        let target = target_root.ok_or_else(|| {
            invalid_skill_change("existing skill target has no resolved workspace path")
        })?;
        bounded_file_inventory(target)?
    } else {
        Vec::new()
    };
    let snapshot = target_root.map(package_snapshot).transpose()?;
    let target_metrics = snapshot
        .as_ref()
        .map_or_else(SkillPackageMetrics::default, package_metrics);
    let base_digest = snapshot.map_or_else(absent_package_digest, |snapshot| snapshot.digest);
    Ok((target_files, target_metrics, base_digest))
}

fn core_tool_inventory(
    repo_root: &Path,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<Vec<SkillCatalogEntry>, RuntimeError> {
    let mut core_tools = catalog_inventory(repo_root, RunxListRequestedKind::Tools)?;
    {
        let native_tools = crate::tool_catalogs::native::inventory(effects);
        let native_names = native_tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        core_tools.retain(|tool| !native_names.contains(&tool.name));
        core_tools.extend(native_tools);
    }
    core_tools.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(core_tools)
}

fn bounded_file_inventory(root: &Path) -> Result<Vec<SkillPackageFile>, RuntimeError> {
    let mut output = Vec::new();
    inventory_directory(root, root, &mut output)?;
    if output.len() > MAX_PACKAGE_FILES {
        return Err(invalid_skill_change(format!(
            "skill package inventory exceeds {MAX_PACKAGE_FILES} files"
        )));
    }
    Ok(output)
}

fn inventory_directory(
    root: &Path,
    current: &Path,
    output: &mut Vec<SkillPackageFile>,
) -> Result<(), RuntimeError> {
    for entry in read_dir_sorted(current)? {
        if ignored_package_entry(&entry.name) {
            continue;
        }
        if entry.is_dir {
            inventory_directory(root, &entry.path, output)?;
        } else if entry.is_file {
            let metadata = fs::metadata(&entry.path).map_err(|source| {
                RuntimeError::io(format!("reading {}", entry.path.display()), source)
            })?;
            output.push(SkillPackageFile {
                path: display_repo_path(root, &entry.path).into(),
                bytes: metadata.len(),
            });
            if output.len() > MAX_PACKAGE_FILES {
                break;
            }
        }
    }
    Ok(())
}

fn catalog_inventory(
    repo_root: &Path,
    requested_kind: RunxListRequestedKind,
) -> Result<Vec<SkillCatalogEntry>, RuntimeError> {
    let report = crate::list_authoring_primitives(&RunxListOptions {
        root: repo_root.to_path_buf(),
        requested_kind,
    })?;
    Ok(report.items.into_iter().map(catalog_entry).collect())
}

fn catalog_entry(item: RunxListItem) -> SkillCatalogEntry {
    SkillCatalogEntry {
        name: item.name.into(),
        kind: item.kind,
        description: item
            .description
            .and_then(runx_contracts::schema::NonEmptyString::new),
        path: item.path.into(),
        status: item.status,
        scopes: item
            .scopes
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect(),
        fixtures: item.fixtures,
        harness_cases: item.harness_cases,
    }
}
