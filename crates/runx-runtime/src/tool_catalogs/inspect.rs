// Module rationale: tool inspection keeps local manifest
// resolution, provenance, and JSON projection in one read-only diagnostic
// surface.
use std::path::{Component, Path, PathBuf};

use runx_contracts::tools::{
    ToolBuildStatus, ToolInspectOrigin, ToolInspectProvenance, ToolInspectReport, ToolInspectResult,
};

use super::error::ToolCatalogError;
use crate::path_util::display_path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInspectOptions {
    pub root: PathBuf,
    pub tool: String,
    pub source: Option<String>,
    pub search_from_directory: PathBuf,
    pub tool_roots: Vec<PathBuf>,
    pub allow_explicit_manifest_path: bool,
}

#[derive(Clone, Debug)]
pub struct LocalToolResolution {
    pub manifest_path: PathBuf,
    pub manifest_source: String,
    pub tool: runx_parser::ValidatedTool,
}

pub fn inspect_tool(options: &ToolInspectOptions) -> Result<ToolInspectReport, ToolCatalogError> {
    inspect_tool_with_effects(options, &crate::RuntimeEffectRegistry::default())
}

pub fn inspect_tool_with_effects(
    options: &ToolInspectOptions,
    effects: &crate::RuntimeEffectRegistry,
) -> Result<ToolInspectReport, ToolCatalogError> {
    if native_source_allowed(options.source.as_deref(), &options.tool, effects)
        && let Some(report) = super::native::inspect(&options.tool, &options.root, effects)
    {
        return Ok(report);
    }
    let manifest_path = resolve_local_manifest(options)?;
    let tool = read_local_tool_manifest(&manifest_path)?;
    Ok(ToolInspectReport {
        status: ToolBuildStatus::Success,
        tool: inspect_local_tool(&manifest_path, tool)?,
    })
}

fn native_source_allowed(
    source: Option<&str>,
    tool: &str,
    effects: &crate::RuntimeEffectRegistry,
) -> bool {
    let source = source.map(str::trim).map(str::to_ascii_lowercase);
    match source.as_deref() {
        None | Some("") | Some("all") | Some("native") => true,
        Some("runx-runtime") => super::native::is_core_tool(tool),
        Some(source) => effects
            .capability(tool)
            .is_some_and(|capability| capability.definition().source.eq_ignore_ascii_case(source)),
    }
}

pub fn resolve_local_tool(
    options: &ToolInspectOptions,
) -> Result<LocalToolResolution, ToolCatalogError> {
    let manifest_path = resolve_local_manifest(options)?;
    let document = super::manifest::read_document(&manifest_path)?;
    Ok(LocalToolResolution {
        manifest_path,
        manifest_source: document.source,
        tool: document.tool,
    })
}

fn read_local_tool_manifest(
    manifest_path: &Path,
) -> Result<runx_parser::ValidatedTool, ToolCatalogError> {
    super::manifest::read(manifest_path)
}

fn inspect_local_tool(
    manifest_path: &Path,
    tool: runx_parser::ValidatedTool,
) -> Result<ToolInspectResult, ToolCatalogError> {
    Ok(ToolInspectResult {
        name: tool.name,
        description: tool.description,
        execution: "process".to_owned(),
        inputs: tool.inputs,
        scopes: tool.scopes,
        runtime: Some(super::projection::runtime_command(&tool.source)),
        reference_path: display_path(manifest_path),
        skill_directory: manifest_path
            .parent()
            .map(display_path)
            .unwrap_or_else(|| ".".to_owned()),
        provenance: local_provenance(),
    })
}

fn local_provenance() -> ToolInspectProvenance {
    ToolInspectProvenance {
        origin: ToolInspectOrigin::Local,
        source: None,
        source_label: None,
        namespace: None,
        external_name: None,
        catalog_id: None,
        tool_id: None,
        tags: None,
    }
}

fn resolve_local_manifest(options: &ToolInspectOptions) -> Result<PathBuf, ToolCatalogError> {
    if options.allow_explicit_manifest_path
        && let Some(path) = explicit_manifest_path(&options.tool, &options.search_from_directory)?
    {
        return Ok(path);
    }

    let segments = tool_segments(&options.tool)?;
    for root in resolve_tool_roots(options) {
        let manifest = root
            .join(segments.iter().collect::<PathBuf>())
            .join("manifest.json");
        if manifest.exists() {
            return Ok(manifest);
        }
    }

    Err(ToolCatalogError::NotFound(format!(
        "Tool '{}' was not found in configured tool roots.",
        options.tool
    )))
}

fn explicit_manifest_path(
    tool: &str,
    search_from_directory: &Path,
) -> Result<Option<PathBuf>, ToolCatalogError> {
    let candidate = Path::new(tool);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ToolCatalogError::InvalidRequest(
            "Explicit tool manifest paths must be relative and must not contain '..'.".to_owned(),
        ));
    }
    let resolved = search_from_directory.join(candidate);
    if resolved.is_file() {
        return Ok(Some(resolved));
    }
    let manifest = resolved.join("manifest.json");
    if manifest.is_file() {
        return Ok(Some(manifest));
    }
    Ok(None)
}

fn tool_segments(tool: &str) -> Result<Vec<&str>, ToolCatalogError> {
    let segments = tool
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() < 2 {
        return Err(ToolCatalogError::Unqualified(format!(
            "Tool '{tool}' must include a namespace, for example fs.read."
        )));
    }
    Ok(segments)
}

fn resolve_tool_roots(options: &ToolInspectOptions) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_existing_dirs(&mut roots, options.tool_roots.iter().cloned());
    push_existing_dirs(&mut roots, [options.root.join("tools")]);
    let root = options.root.as_path();
    let mut current = options.search_from_directory.clone();
    if !current.starts_with(root) {
        return roots;
    }
    loop {
        push_existing_dirs(&mut roots, [crate::paths::tools_in(&current)]);
        if current == root {
            break;
        }
        let Some(parent) = current.parent().map(Path::to_path_buf) else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent;
    }
    roots
}

fn push_existing_dirs(roots: &mut Vec<PathBuf>, candidates: impl IntoIterator<Item = PathBuf>) {
    for candidate in candidates {
        if candidate.is_dir() && !roots.iter().any(|root| root == &candidate) {
            roots.push(candidate);
        }
    }
}
