use std::collections::{BTreeMap, BTreeSet};

use crate::test_fixture::TestFixture;
use crate::{SkillMethodManifest, StepKind, ToolTarget, Workflow};

use super::super::path::normalize_context_ref;
use super::super::{SkillPackageError, SkillPackageSource};
use super::contract::{text_file, validate_manual};

pub(super) fn collect_package_references(
    profiles: &BTreeMap<String, SkillMethodManifest>,
) -> Result<PackageReferences, SkillPackageError> {
    let mut references = PackageReferences::default();
    for (profile_path, manifest) in profiles {
        let profile_dir = profile_directory(profile_path);
        for (name, method) in &manifest.methods {
            collect_workflow_references(
                &format!("{profile_path}.methods.{name}"),
                profile_dir,
                &method.workflow,
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

/// The files a package's tests read: each test, the receipts its setup
/// stages, and the fixture package it runs, which must be a package of its
/// own under `fixtures/`.
pub(super) fn collect_test_fixture_references(
    fixtures: &BTreeMap<String, TestFixture>,
    package: &SkillPackageSource,
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
        if let Some(fixture_package) = &fixture.package
            && !(package
                .files
                .contains_key(&format!("{fixture_package}/SKILL.md"))
                && package
                    .files
                    .contains_key(&format!("{fixture_package}/run.yaml")))
        {
            return Err(SkillPackageError::invalid(
                fixture_path,
                format!(
                    "package {fixture_package:?} is not a fixture package: it needs its own SKILL.md and run.yaml"
                ),
            ));
        }
    }
    Ok(files)
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

fn collect_workflow_references(
    field: &str,
    profile_dir: &str,
    workflow: &Workflow,
    references: &mut PackageReferences,
) -> Result<(), SkillPackageError> {
    for (index, step) in workflow.steps.iter().enumerate() {
        let step_field = format!("{field}.steps[{index}]");
        collect_step_context_refs(
            &step_field,
            profile_dir,
            &step.context_skills,
            &mut references.context_refs,
        )?;
        match &step.kind {
            StepKind::Tool(ToolTarget::Worker(worker)) => {
                references
                    .module_roots
                    .insert(package_relative(profile_dir, &worker.module));
            }
            StepKind::Script(script) => {
                references
                    .module_roots
                    .insert(package_relative(profile_dir, &script.module));
            }
            StepKind::Tool(ToolTarget::Process(process)) => collect_process_execution_files(
                profile_dir,
                std::iter::once(&process.command).chain(&process.args),
                &mut references.execution_files,
            ),
            _ => {}
        }
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
                format!("{step_field}.context.skills"),
                format!("context skill ref {reference:?} is declared more than once"),
            ));
        }
        context_refs.push(ContextReference {
            field: format!("{step_field}.context.skills"),
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
    crate::package::profile_directory(path).unwrap_or("")
}

fn package_relative(directory: &str, path: &str) -> String {
    if directory.is_empty() {
        path.to_owned()
    } else {
        format!("{directory}/{path}")
    }
}
