// Module rationale: tool build validates canonical source manifests and emits
// derived digests in its report without rewriting author-owned files.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use runx_contracts::sha256_prefixed;
use runx_contracts::tools::{
    BuiltToolItem, ToolBuildReport, ToolBuildReportSchema, ToolBuildStatus, ToolInput,
};
use runx_parser::{ProcessTool, SkillArtifactContract};
use serde::Serialize;

use super::error::ToolCatalogError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolBuildOptions {
    pub root: PathBuf,
    pub tool_path: Option<PathBuf>,
    pub all: bool,
}

pub fn build_tool_catalogs(
    options: &ToolBuildOptions,
) -> Result<ToolBuildReport, ToolCatalogError> {
    let tool_dirs = if options.all {
        discover_tool_directories(&options.root)?
    } else {
        vec![resolve_tool_path(
            &options.root,
            options.tool_path.as_deref(),
        )?]
    };
    let mut built = Vec::new();
    let mut errors = Vec::new();
    for tool_dir in tool_dirs {
        match build_tool_manifest(&options.root, &tool_dir) {
            Ok(item) => built.push(item),
            Err(error) => errors.push(format!(
                "{}: {}",
                project_path(&options.root, &tool_dir),
                error.concise_message()
            )),
        }
    }
    Ok(ToolBuildReport {
        schema: ToolBuildReportSchema::V1,
        status: if errors.is_empty() {
            ToolBuildStatus::Success
        } else {
            ToolBuildStatus::Failure
        },
        built,
        errors,
    })
}

fn build_tool_manifest(root: &Path, tool_dir: &Path) -> Result<BuiltToolItem, ToolCatalogError> {
    let manifest_path = tool_dir.join("manifest.json");
    let validated = super::manifest::read(&manifest_path)?;
    let source_hash = hash_tool_source(tool_dir, &validated.source)?;
    let schema_hash = schema_hash(
        &manifest_path,
        &validated.inputs,
        validated.artifacts.as_ref(),
    )?;
    Ok(BuiltToolItem {
        path: project_path(root, tool_dir),
        manifest: project_path(root, &manifest_path),
        source_hash,
        schema_hash,
    })
}

#[derive(Serialize)]
struct ToolSchemaMaterial<'a> {
    inputs: &'a BTreeMap<String, ToolInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifacts: Option<&'a SkillArtifactContract>,
}

fn schema_hash(
    manifest_path: &Path,
    inputs: &BTreeMap<String, ToolInput>,
    artifacts: Option<&SkillArtifactContract>,
) -> Result<String, ToolCatalogError> {
    let bytes = serde_json::to_vec(&ToolSchemaMaterial { inputs, artifacts })
        .map_err(|source| ToolCatalogError::json("hashing tool schema", manifest_path, source))?;
    Ok(sha256_prefixed(&bytes))
}

pub(crate) fn hash_tool_source(
    tool_dir: &Path,
    source: &ProcessTool,
) -> Result<String, ToolCatalogError> {
    let roots = source_entrypoints(tool_dir, source);
    let files = tool_source_closure(&roots)?;
    reject_uncompiled_node_sources(source, &files)?;
    let mut bytes = Vec::new();
    let hash_root = fs::canonicalize(tool_dir).unwrap_or_else(|_| tool_dir.to_path_buf());
    for file_path in &files {
        bytes.extend(source_hash_path(&hash_root, file_path).as_bytes());
        bytes.push(0);
        bytes.extend(
            fs::read(file_path)
                .map_err(|error| ToolCatalogError::io("reading tool source", file_path, error))?,
        );
        bytes.push(0);
    }
    if files.is_empty() {
        bytes.extend(b"no-source");
    }
    Ok(sha256_prefixed(&bytes))
}

fn reject_uncompiled_node_sources(
    source: &ProcessTool,
    files: &[PathBuf],
) -> Result<(), ToolCatalogError> {
    let command = source.command.as_str();
    let command_name = Path::new(command)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(command);
    if !command_name.eq_ignore_ascii_case("node") {
        return Ok(());
    }
    let Some(source_path) = files.iter().find(|path| {
        matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("ts" | "tsx" | "mts" | "cts")
        )
    }) else {
        return Ok(());
    };
    Err(ToolCatalogError::InvalidManifest {
        path: source_path.clone(),
        message: format!(
            "node tool source imports uncompiled TypeScript at {}; declare an executable JavaScript entrypoint",
            source_path.display()
        ),
    })
}

fn source_entrypoints(tool_dir: &Path, source: &ProcessTool) -> Vec<PathBuf> {
    std::iter::once(&source.command)
        .chain(&source.args)
        .filter_map(|candidate| local_source_path(tool_dir, candidate))
        .collect()
}

fn local_source_path(tool_dir: &Path, candidate: &str) -> Option<PathBuf> {
    let path = Path::new(candidate);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::CurDir | Component::Normal(_)))
    {
        return None;
    }
    let resolved = tool_dir.join(path);
    resolved.is_file().then_some(resolved)
}

fn tool_source_closure(roots: &[PathBuf]) -> Result<Vec<PathBuf>, ToolCatalogError> {
    let mut pending = roots.to_vec();
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < pending.len() {
        let source_path = pending[index].clone();
        index += 1;
        if !source_path.exists() {
            continue;
        }
        let source_path = fs::canonicalize(&source_path)
            .map_err(|error| ToolCatalogError::io("resolving tool source", &source_path, error))?;
        if !seen.insert(source_path.clone()) {
            continue;
        }
        let source = fs::read_to_string(&source_path)
            .map_err(|error| ToolCatalogError::io("reading tool source", &source_path, error))?;
        if !is_javascript_source(&source_path) {
            continue;
        }
        let source_label = source_path.to_string_lossy();
        let dependencies = runx_parser::javascript_process_module_imports(&source_label, &source)
            .map_err(|error| ToolCatalogError::InvalidManifest {
            path: source_path.clone(),
            message: error.to_string(),
        })?;
        for specifier in dependencies
            .into_iter()
            .filter(|specifier| specifier.starts_with("./") || specifier.starts_with("../"))
        {
            if let Some(dependency) = resolve_local_source_import(&source_path, &specifier)? {
                pending.push(dependency);
            } else {
                return Err(ToolCatalogError::InvalidManifest {
                    path: source_path.clone(),
                    message: format!(
                        "process-backed tool import {specifier:?} does not resolve to a local source file"
                    ),
                });
            }
        }
    }
    Ok(seen.into_iter().collect())
}

fn is_javascript_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "mjs" | "cjs")
    )
}

fn resolve_local_source_import(
    from_file: &Path,
    specifier: &str,
) -> Result<Option<PathBuf>, ToolCatalogError> {
    let clean_specifier = specifier
        .split(['?', '#'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(specifier);
    let base = from_file
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(clean_specifier);
    for candidate in source_import_candidates(&base) {
        if candidate.exists() {
            return fs::canonicalize(&candidate)
                .map(Some)
                .map_err(|error| ToolCatalogError::io("resolving tool source", &candidate, error));
        }
    }
    Ok(None)
}

fn source_import_candidates(base: &Path) -> Vec<PathBuf> {
    if base.extension().is_none() {
        let extensions = [".ts", ".tsx", ".mts", ".cts", ".js", ".mjs", ".cjs"];
        return extensions
            .iter()
            .map(|extension| PathBuf::from(format!("{}{}", base.display(), extension)))
            .chain(
                extensions
                    .iter()
                    .map(|extension| base.join(format!("index{extension}"))),
            )
            .collect();
    }
    vec![base.to_path_buf()]
}

fn source_hash_path(root: &Path, file_path: &Path) -> String {
    let root_components = path_component_strings(root);
    let file_components = path_component_strings(file_path);
    let common_len = root_components
        .iter()
        .zip(&file_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common_len == 0 {
        return file_path.to_string_lossy().replace('\\', "/");
    }
    let mut parts = Vec::new();
    for _ in common_len..root_components.len() {
        parts.push("..".to_owned());
    }
    parts.extend(file_components[common_len..].iter().cloned());
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

fn path_component_strings(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

fn discover_tool_directories(root: &Path) -> Result<Vec<PathBuf>, ToolCatalogError> {
    let tools_root = root.join("tools");
    let mut directories = Vec::new();
    for namespace in read_dirs(&tools_root)? {
        for tool in read_dirs(&namespace)? {
            if tool.join("manifest.json").exists() {
                directories.push(tool);
            }
        }
    }
    directories.sort();
    Ok(directories)
}

fn read_dirs(path: &Path) -> Result<Vec<PathBuf>, ToolCatalogError> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ToolCatalogError::io("reading directory", path, error)),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| ToolCatalogError::io("reading directory", path, error))?;
        let file_type = entry.file_type().map_err(|error| {
            ToolCatalogError::io("reading directory entry", entry.path(), error)
        })?;
        if file_type.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn resolve_tool_path(root: &Path, tool_path: Option<&Path>) -> Result<PathBuf, ToolCatalogError> {
    let Some(tool_path) = tool_path else {
        return Err(ToolCatalogError::InvalidRequest(
            "runx tool build requires a tool directory or --all".to_owned(),
        ));
    };
    if tool_path.is_absolute() {
        Ok(tool_path.to_path_buf())
    } else {
        Ok(root.join(tool_path))
    }
}

pub(crate) fn project_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map_or(path, |path| path)
        .to_string_lossy()
        .replace('\\', "/")
}
