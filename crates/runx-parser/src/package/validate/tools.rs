use std::collections::{BTreeMap, BTreeSet, VecDeque};

use runx_core::policy::admit_agent_tool_ref;

use crate::{SkillSource, ValidatedTool, parse_tool_manifest_json, validate_tool_manifest};

use super::super::javascript::process_module_imports;
use super::super::path::validate_package_path;
use super::super::{SkillPackageError, SkillPackageSource, ValidatedPackageTool};
use super::contract::{has_nested_manual_boundary, text_file};

pub(super) fn validate_package_tools(
    source: &SkillPackageSource,
) -> Result<BTreeMap<String, ValidatedPackageTool>, SkillPackageError> {
    owned_tool_manifest_paths(source)
        .into_iter()
        .map(|manifest_path| {
            let contents = source.files.get(&manifest_path).ok_or_else(|| {
                SkillPackageError::invalid(&manifest_path, "tool manifest source is missing")
            })?;
            let tool = validate_manifest(&manifest_path, text_file(&manifest_path, contents)?)?;
            validate_tool_identity(&manifest_path, &tool)?;
            let source_files = validate_tool_source_files(source, &manifest_path, &tool.source)?;
            Ok((
                manifest_path.clone(),
                ValidatedPackageTool {
                    manifest_path,
                    tool,
                    source_files,
                },
            ))
        })
        .collect()
}

fn owned_tool_manifest_paths(source: &SkillPackageSource) -> Vec<String> {
    source
        .files
        .keys()
        .filter(|path| is_tool_manifest_path(path))
        .filter(|path| !has_nested_manual_boundary(path, source))
        .cloned()
        .collect()
}

fn is_tool_manifest_path(path: &str) -> bool {
    path.ends_with("/manifest.json") && path.split('/').any(|segment| segment == "tools")
}

fn validate_manifest(path: &str, contents: &str) -> Result<ValidatedTool, SkillPackageError> {
    let raw = parse_tool_manifest_json(contents).map_err(|source| SkillPackageError::Parse {
        path: path.to_owned(),
        source,
    })?;
    validate_tool_manifest(raw).map_err(|source| SkillPackageError::Validation {
        path: path.to_owned(),
        source,
    })
}

fn validate_tool_identity(path: &str, tool: &ValidatedTool) -> Result<(), SkillPackageError> {
    let admission = admit_agent_tool_ref(&tool.name);
    if !admission.allowed {
        return Err(SkillPackageError::invalid(
            format!("{path}.name"),
            format!(
                "bundled tool name {:?} is invalid: {}",
                tool.name, admission.reason
            ),
        ));
    }
    let segments = path.split('/').collect::<Vec<_>>();
    let tools_index = segments
        .iter()
        .rposition(|segment| *segment == "tools")
        .ok_or_else(|| SkillPackageError::invalid(path, "bundled tool has no tools/ root"))?;
    let name_segments = &segments[tools_index + 1..segments.len().saturating_sub(1)];
    if name_segments.len() < 2 {
        return Err(SkillPackageError::invalid(
            path,
            "bundled tool paths must be tools/<namespace>/<tool>/manifest.json",
        ));
    }
    let expected = name_segments.join(".");
    if expected != tool.name {
        return Err(SkillPackageError::invalid(
            format!("{path}.name"),
            format!(
                "bundled tool name {:?} must match its catalog path {expected:?}",
                tool.name
            ),
        ));
    }
    Ok(())
}

fn validate_tool_source_files(
    package: &SkillPackageSource,
    manifest_path: &str,
    source: &SkillSource,
) -> Result<BTreeSet<String>, SkillPackageError> {
    let tool_directory = manifest_path
        .strip_suffix("/manifest.json")
        .ok_or_else(|| SkillPackageError::invalid(manifest_path, "invalid tool manifest path"))?;
    let mut files = BTreeSet::new();
    let mut modules = VecDeque::new();

    for entry in source_entries(source) {
        let Some(path) = resolve_source_entry(package, tool_directory, &entry)? else {
            continue;
        };
        if is_javascript_source(&path) {
            modules.push_back(path.clone());
        }
        files.insert(path);
    }

    while let Some(module_path) = modules.pop_front() {
        let bytes = package.files.get(&module_path).ok_or_else(|| {
            SkillPackageError::invalid(&module_path, "tool source dependency is missing")
        })?;
        let source_text = text_file(&module_path, bytes)?;
        for specifier in process_module_imports(&module_path, source_text)? {
            if specifier.starts_with("node:") {
                continue;
            }
            if !(specifier.starts_with("./") || specifier.starts_with("../")) {
                return Err(SkillPackageError::invalid(
                    &module_path,
                    format!(
                        "process-backed tool import {specifier:?} must be a node: builtin or a relative package file"
                    ),
                ));
            }
            let directory = module_path
                .rsplit_once('/')
                .map_or("", |(directory, _)| directory);
            let dependency = normalize_relative_path(directory, &specifier, &module_path)?;
            if !package.files.contains_key(&dependency) {
                return Err(SkillPackageError::invalid(
                    &module_path,
                    format!(
                        "process-backed tool import {specifier:?} resolves to missing file {dependency}"
                    ),
                ));
            }
            if files.insert(dependency.clone()) && is_javascript_source(&dependency) {
                modules.push_back(dependency);
            }
        }
    }

    if node_command(source)
        && let Some(path) = files.iter().find(|path| is_typescript_source(path))
    {
        return Err(SkillPackageError::invalid(
            path,
            "node tool source imports uncompiled TypeScript; declare an executable JavaScript entrypoint",
        ));
    }

    Ok(files)
}

struct SourceEntry<'a> {
    field: String,
    value: &'a str,
    required_local: bool,
}

fn source_entries(source: &SkillSource) -> Vec<SourceEntry<'_>> {
    let mut entries = Vec::new();
    if let Some(command) = source.command.as_deref() {
        entries.push(SourceEntry {
            field: "source.command".to_owned(),
            value: command,
            required_local: command.starts_with('.'),
        });
    }
    for (index, argument) in source.args.iter().enumerate() {
        entries.push(SourceEntry {
            field: format!("source.args[{index}]"),
            value: argument,
            required_local: looks_like_local_source(argument),
        });
    }
    if let Some(module) = source.module.as_deref() {
        entries.push(SourceEntry {
            field: "source.module".to_owned(),
            value: module,
            required_local: true,
        });
    }
    entries
}

fn resolve_source_entry(
    package: &SkillPackageSource,
    tool_directory: &str,
    entry: &SourceEntry<'_>,
) -> Result<Option<String>, SkillPackageError> {
    let candidate = normalize_relative_path(tool_directory, entry.value, &entry.field)?;
    if package.files.contains_key(&candidate) {
        return Ok(Some(candidate));
    }
    if entry.required_local {
        return Err(SkillPackageError::invalid(
            &entry.field,
            format!("declared tool source file {candidate:?} is missing from the package"),
        ));
    }
    Ok(None)
}

fn normalize_relative_path(
    directory: &str,
    value: &str,
    field: &str,
) -> Result<String, SkillPackageError> {
    if value.trim() != value
        || value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains('?')
        || value.contains('#')
    {
        return Err(SkillPackageError::invalid(
            field,
            format!("tool source path {value:?} must be a normalized relative POSIX path"),
        ));
    }
    let mut segments = if directory.is_empty() {
        Vec::new()
    } else {
        directory.split('/').collect::<Vec<_>>()
    };
    for segment in value.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(SkillPackageError::invalid(
                        field,
                        format!("tool source path {value:?} escapes the skill package"),
                    ));
                }
            }
            segment => segments.push(segment),
        }
    }
    let resolved = segments.join("/");
    validate_package_path(&resolved)?;
    Ok(resolved)
}

fn looks_like_local_source(value: &str) -> bool {
    value.starts_with('.')
        || matches!(
            value.rsplit_once('.').map(|(_, extension)| extension),
            Some("js" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "py" | "rb" | "sh" | "wasm")
        )
}

fn is_javascript_source(path: &str) -> bool {
    matches!(
        path.rsplit_once('.').map(|(_, extension)| extension),
        Some("js" | "mjs" | "cjs")
    )
}

fn is_typescript_source(path: &str) -> bool {
    matches!(
        path.rsplit_once('.').map(|(_, extension)| extension),
        Some("ts" | "tsx" | "mts" | "cts")
    )
}

fn node_command(source: &SkillSource) -> bool {
    source.command.as_deref().is_some_and(|command| {
        let executable = command.rsplit(['/', '\\']).next().unwrap_or(command);
        executable
            .strip_suffix(".exe")
            .unwrap_or(executable)
            .eq_ignore_ascii_case("node")
    })
}
