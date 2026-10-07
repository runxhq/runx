use std::collections::{BTreeMap, BTreeSet};

use crate::harness_fixture::{HarnessFixture, HarnessFixtureKind};
use crate::{ExecutionGraph, SkillRunnerManifest, SkillSource, SourceKind};

use super::super::path::{normalize_context_ref, validate_package_path};
use super::super::{SkillPackageError, SkillPackageSource};
use super::contract::{text_file, validate_manual};

pub(super) fn collect_package_references(
    profiles: &BTreeMap<String, SkillRunnerManifest>,
    package: &SkillPackageSource,
) -> Result<PackageReferences, SkillPackageError> {
    let mut references = PackageReferences::default();
    for (profile_path, manifest) in profiles {
        let profile_dir = profile_directory(profile_path);
        for (name, runner) in &manifest.runners {
            collect_source_references(
                &format!("{profile_path}.runners.{name}.source"),
                profile_dir,
                &runner.source,
                package,
                &mut references,
            )?;
        }
    }
    Ok(references)
}

#[derive(Default)]
pub(super) struct PackageReferences {
    pub(super) module_roots: BTreeSet<String>,
    pub(super) execution_files: BTreeSet<String>,
    pub(super) context_refs: Vec<ContextReference>,
}

#[derive(Clone)]
pub(super) struct ContextReference {
    field: String,
    profile_dir: String,
    pub(super) reference: String,
}

pub(super) fn collect_harness_fixture_references(
    fixtures: &BTreeMap<String, HarnessFixture>,
    package: &SkillPackageSource,
    references: &mut PackageReferences,
) -> Result<BTreeSet<String>, SkillPackageError> {
    let mut files = BTreeSet::new();
    for (fixture_path, fixture) in fixtures {
        files.insert(fixture_path.clone());
        for receipt_path in &fixture.setup.receipts {
            if !package.files.contains_key(receipt_path) {
                return Err(SkillPackageError::invalid(
                    fixture_path,
                    format!("setup receipt {receipt_path:?} is missing from the skill package"),
                ));
            }
            files.insert(receipt_path.clone());
        }
        if fixture.kind != HarnessFixtureKind::Graph {
            continue;
        }
        let target = resolve_fixture_target(fixture_path, &fixture.target)?;
        let contents = package.files.get(&target).ok_or_else(|| {
            SkillPackageError::invalid(
                fixture_path,
                format!(
                    "graph target {:?} resolves to missing file {target}",
                    fixture.target
                ),
            )
        })?;
        let graph = crate::validate_graph(
            crate::parse_graph_yaml(text_file(&target, contents)?).map_err(|source| {
                SkillPackageError::Parse {
                    path: target.clone(),
                    source,
                }
            })?,
        )
        .map_err(|source| SkillPackageError::Validation {
            path: target.clone(),
            source,
        })?;
        collect_graph_references(
            &format!("{fixture_path}.target"),
            package_directory(&target),
            &graph,
            package,
            references,
        )?;
        files.insert(target);
    }
    Ok(files)
}

fn collect_source_references(
    field: &str,
    profile_dir: &str,
    source: &SkillSource,
    package: &SkillPackageSource,
    references: &mut PackageReferences,
) -> Result<(), SkillPackageError> {
    if let Some(module) = &source.module {
        references
            .module_roots
            .insert(package_relative(profile_dir, module));
    }
    if source.source_type == SourceKind::CliTool {
        collect_process_execution_files(
            profile_dir,
            source.command.iter().chain(&source.args),
            &mut references.execution_files,
        );
    }
    if let Some(graph) = &source.graph {
        collect_graph_references(field, profile_dir, graph, package, references)?;
    }
    Ok(())
}

fn collect_process_execution_files<'a>(
    profile_dir: &str,
    values: impl IntoIterator<Item = &'a String>,
    files: &mut BTreeSet<String>,
) {
    for value in values {
        if let Some(path) = process_script_path(profile_dir, value) {
            files.insert(path);
        }
    }
}

fn process_script_path(profile_dir: &str, value: &str) -> Option<String> {
    let path = value
        .trim()
        .strip_prefix("./")
        .unwrap_or_else(|| value.trim());
    let valid = (path.ends_with(".js") || path.ends_with(".mjs"))
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".."));
    valid.then(|| package_relative(profile_dir, path))
}

fn collect_graph_references(
    field: &str,
    profile_dir: &str,
    graph: &ExecutionGraph,
    package: &SkillPackageSource,
    references: &mut PackageReferences,
) -> Result<(), SkillPackageError> {
    for (index, step) in graph.steps.iter().enumerate() {
        let step_field = format!("{field}.graph.steps[{index}]");
        collect_step_context_refs(
            &step_field,
            profile_dir,
            &step.context_skills,
            &mut references.context_refs,
        )?;
        let Some(run) = &step.run else {
            continue;
        };
        let Some(nested) = run.source() else {
            continue;
        };
        collect_source_references(
            &format!("{step_field}.run"),
            profile_dir,
            nested,
            package,
            references,
        )?;
    }
    Ok(())
}

fn collect_step_context_refs(
    step_field: &str,
    profile_dir: &str,
    references: &[String],
    context_refs: &mut Vec<ContextReference>,
) -> Result<(), SkillPackageError> {
    let mut seen = BTreeSet::new();
    for reference in references {
        if !seen.insert(reference) {
            return Err(SkillPackageError::invalid(
                format!("{step_field}.context_skills"),
                format!("context skill ref {reference:?} is declared more than once"),
            ));
        }
        context_refs.push(ContextReference {
            field: format!("{step_field}.context_skills"),
            profile_dir: profile_dir.to_owned(),
            reference: reference.clone(),
        });
    }
    Ok(())
}

pub(super) fn validate_context_skill_sources(
    source: &SkillPackageSource,
    references: &[ContextReference],
) -> Result<(), SkillPackageError> {
    for reference in references {
        let Some(manual_path) =
            normalize_context_ref(&reference.profile_dir, &reference.reference)?
        else {
            continue;
        };
        let markdown = source.files.get(&manual_path).ok_or_else(|| {
            SkillPackageError::invalid(
                &reference.field,
                format!(
                    "context skill ref {:?} does not resolve to {manual_path}",
                    reference.reference
                ),
            )
        })?;
        validate_manual(text_file(&manual_path, markdown)?).map_err(|error| {
            SkillPackageError::invalid(
                manual_path,
                format!("context skill manual is invalid: {error}"),
            )
        })?;
    }
    Ok(())
}

fn profile_directory(path: &str) -> &str {
    path.strip_suffix("/X.yaml").unwrap_or("")
}

fn package_directory(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn resolve_fixture_target(
    fixture_path: &str,
    reference: &str,
) -> Result<String, SkillPackageError> {
    if reference.trim() != reference
        || reference.is_empty()
        || reference.starts_with('/')
        || reference.contains('\\')
    {
        return Err(SkillPackageError::invalid(
            fixture_path,
            format!("graph target {reference:?} must be a relative POSIX path"),
        ));
    }
    let mut segments = package_directory(fixture_path)
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    for segment in reference.split('/') {
        match segment {
            "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(SkillPackageError::invalid(
                        fixture_path,
                        format!("graph target {reference:?} escapes the skill package"),
                    ));
                }
            }
            "" => {
                return Err(SkillPackageError::invalid(
                    fixture_path,
                    format!("graph target {reference:?} contains an empty path segment"),
                ));
            }
            value => segments.push(value),
        }
    }
    let resolved = segments.join("/");
    validate_package_path(&resolved)?;
    Ok(resolved)
}

fn package_relative(directory: &str, path: &str) -> String {
    if directory.is_empty() {
        path.to_owned()
    } else {
        format!("{directory}/{path}")
    }
}
