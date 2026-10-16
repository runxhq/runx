use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::MAX_PACKAGE_FILES;
use super::path::{
    canonical_directory, display_relative, display_repo_path, ignored_package_entry,
    invalid_skill_change, json_string, normalize_child_path,
};
use super::snapshot::{
    absent_package_digest, package_metrics, package_metrics_json, package_snapshot,
};
use crate::filesystem::read_dir_sorted;
use crate::{RuntimeError, RunxListItem, RunxListItemKind, RunxListOptions, RunxListRequestedKind};

pub(crate) fn inspect_skill_workspace(
    repo_root: &Path,
    target_dir: Option<&str>,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<JsonObject, RuntimeError> {
    let repo_root = canonical_directory(repo_root, "skill workspace")?;
    let target_dir = target_dir.map(normalize_child_path).transpose()?;
    let target_root = target_dir.as_ref().map(|path| repo_root.join(path));
    let target_exists = target_root.as_ref().is_some_and(|path| path.is_dir());
    let (target_files, target_metrics, base_digest) =
        inspect_target(target_root.as_deref(), target_exists)?;
    let target_inspection =
        inspect_target_package(&repo_root, target_root.as_deref(), target_exists);
    let catalog_root = if repo_root.join("skills").is_dir() {
        repo_root.join("skills")
    } else {
        repo_root.clone()
    };
    let catalog_skills = catalog_inventory(&repo_root, RunxListRequestedKind::Skills)?;
    let core_tools = core_tool_inventory(&repo_root, effects)?;

    Ok(JsonObject::from([
        (
            "repo_root".to_owned(),
            JsonValue::String(repo_root.to_string_lossy().into_owned()),
        ),
        (
            "target_dir".to_owned(),
            target_dir
                .as_ref()
                .map(|path| JsonValue::String(display_relative(path)))
                .unwrap_or(JsonValue::Null),
        ),
        ("target_exists".to_owned(), JsonValue::Bool(target_exists)),
        ("base_digest".to_owned(), base_digest),
        ("target_files".to_owned(), JsonValue::Array(target_files)),
        ("target_metrics".to_owned(), target_metrics),
        ("target_inspection".to_owned(), target_inspection),
        (
            "catalog_root".to_owned(),
            JsonValue::String(display_repo_path(&repo_root, &catalog_root)),
        ),
        (
            "catalog_skills".to_owned(),
            JsonValue::Array(catalog_skills),
        ),
        ("core_tools".to_owned(), JsonValue::Array(core_tools)),
    ]))
}

fn inspect_target_package(
    repo_root: &Path,
    target_root: Option<&Path>,
    target_exists: bool,
) -> JsonValue {
    if !target_exists {
        return JsonValue::Null;
    }
    let Some(target_root) = target_root else {
        return invalid_target_inspection("existing skill target has no resolved workspace path");
    };
    let env = std::collections::BTreeMap::from([(
        crate::paths::RUNX_CWD_ENV.to_owned(),
        repo_root.to_string_lossy().into_owned(),
    )]);
    match crate::inspect_skill_package(target_root, None, Some(&env)) {
        Ok(inspection) => inspection,
        Err(error) => invalid_target_inspection(&error.to_string()),
    }
}

fn invalid_target_inspection(error: &str) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("status".to_owned(), JsonValue::String("invalid".to_owned())),
        ("error".to_owned(), JsonValue::String(error.to_owned())),
    ]))
}

fn inspect_target(
    target_root: Option<&Path>,
    target_exists: bool,
) -> Result<(Vec<JsonValue>, JsonValue, JsonValue), RuntimeError> {
    let target_files = if target_exists {
        let target = target_root.ok_or_else(|| {
            invalid_skill_change("existing skill target has no resolved workspace path")
        })?;
        bounded_file_inventory(target)?
    } else {
        Vec::new()
    };
    let snapshot = target_root.map(package_snapshot).transpose()?;
    let target_metrics = snapshot.as_ref().map_or_else(
        || package_metrics_json(&runx_contracts::SkillPackageMetrics::default()),
        |snapshot| package_metrics_json(&package_metrics(snapshot)),
    );
    let base_digest = snapshot
        .map(|snapshot| JsonValue::String(snapshot.digest))
        .unwrap_or_else(|| JsonValue::String(absent_package_digest()));
    Ok((target_files, target_metrics, base_digest))
}

fn core_tool_inventory(
    repo_root: &Path,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<Vec<JsonValue>, RuntimeError> {
    let mut core_tools = catalog_inventory(repo_root, RunxListRequestedKind::Tools)?;
    {
        let native_tools = crate::tool_catalogs::native::inventory(effects);
        let native_names = native_tools
            .iter()
            .map(|tool| json_string(tool, "name"))
            .collect::<BTreeSet<_>>();
        core_tools.retain(|tool| !native_names.contains(json_string(tool, "name")));
        core_tools.extend(native_tools);
    }
    core_tools.sort_by(|left, right| {
        json_string(left, "name")
            .cmp(json_string(right, "name"))
            .then_with(|| json_string(left, "path").cmp(json_string(right, "path")))
    });
    Ok(core_tools)
}

fn bounded_file_inventory(root: &Path) -> Result<Vec<JsonValue>, RuntimeError> {
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
    output: &mut Vec<JsonValue>,
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
            output.push(JsonValue::Object(JsonObject::from([
                (
                    "path".to_owned(),
                    JsonValue::String(display_repo_path(root, &entry.path)),
                ),
                (
                    "bytes".to_owned(),
                    JsonValue::Number(JsonNumber::U64(metadata.len())),
                ),
            ])));
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
) -> Result<Vec<JsonValue>, RuntimeError> {
    let report = crate::list_authoring_primitives(&RunxListOptions {
        root: repo_root.to_path_buf(),
        requested_kind,
    })?;
    Ok(report.items.into_iter().map(list_item_json).collect())
}

fn list_item_json(item: RunxListItem) -> JsonValue {
    let kind = match item.kind {
        RunxListItemKind::Tool => "tool",
        RunxListItemKind::Skill => "skill",
        RunxListItemKind::Graph => "graph",
        RunxListItemKind::Packet => "packet",
        RunxListItemKind::Overlay => "overlay",
    };
    let status = match item.status {
        crate::RunxListStatus::Ok => "ok",
        crate::RunxListStatus::Invalid => "invalid",
    };
    let fixtures = item
        .fixtures
        .map(|value| JsonValue::Number(JsonNumber::U64(value)))
        .unwrap_or(JsonValue::Null);
    let harness_cases = item
        .harness_cases
        .map(|value| JsonValue::Number(JsonNumber::U64(value)))
        .unwrap_or(JsonValue::Null);
    JsonValue::Object(JsonObject::from([
        ("name".to_owned(), JsonValue::String(item.name)),
        ("kind".to_owned(), JsonValue::String(kind.to_owned())),
        ("path".to_owned(), JsonValue::String(item.path)),
        ("status".to_owned(), JsonValue::String(status.to_owned())),
        (
            "scopes".to_owned(),
            JsonValue::Array(
                item.scopes
                    .unwrap_or_default()
                    .into_iter()
                    .map(JsonValue::String)
                    .collect(),
            ),
        ),
        ("fixtures".to_owned(), fixtures),
        ("harness_cases".to_owned(), harness_cases),
    ]))
}
