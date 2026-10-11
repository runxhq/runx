use std::collections::{BTreeMap, BTreeSet};

use crate::analyze_package_catalog_semantics;
use runx_contracts::sha256_prefixed;

use super::path::validate_source_paths;
use super::{SkillPackageError, SkillPackageSource, ValidatedSkillPackage};

mod contract;
mod modules;
mod references;
mod support;
mod tools;

use contract::{required_text_file, validate_manual, validate_package_identity, validate_profiles};
use modules::validate_modules;
use references::{
    collect_harness_fixture_references, collect_package_references, validate_context_skill_sources,
};
use support::{
    validate_execution_files, validate_harness_fixtures, validate_harness_support_files,
    validate_nested_package_consumed_files, validate_operator_reference_files,
};
use tools::validate_package_tools;

pub fn validate_skill_package(
    source: SkillPackageSource,
) -> Result<ValidatedSkillPackage, SkillPackageError> {
    validate_source_paths(&source.files, source.symlinks.iter().cloned())?;
    let manual_markdown = required_text_file(&source, "SKILL.md")?.to_owned();
    let skill = validate_manual(&manual_markdown)?;
    let profiles = validate_profiles(&source)?;
    validate_package_identity(&skill, profiles.get(super::PROFILE_FILE))?;
    let harness_fixtures = validate_harness_fixtures(&source)?;
    validate_catalog_semantics(
        &skill.name,
        profiles.get(super::PROFILE_FILE),
        &harness_fixtures,
    )?;
    let mut references = collect_package_references(&profiles)?;
    let harness_fixture_files =
        collect_harness_fixture_references(&harness_fixtures, &source, &mut references)?;
    let tools = validate_package_tools(&source)?;
    validate_context_skill_sources(&source, &references.context_refs)?;
    let mut execution_files = references.execution_files;
    for package_tool in tools.values() {
        execution_files.insert(package_tool.manifest_path.clone());
        execution_files.extend(package_tool.source_files.iter().cloned());
    }
    validate_execution_files(&source, &execution_files)?;
    let mut harness_files = validate_harness_support_files(&source, &profiles)?;
    harness_files.extend(harness_fixture_files);
    let mut context_skill_refs = references
        .context_refs
        .into_iter()
        .map(|reference| reference.reference)
        .collect::<Vec<_>>();
    context_skill_refs.sort();
    context_skill_refs.dedup();
    let javascript_modules = validate_modules(&source, references.module_roots, &execution_files)?;
    let mut consumed_files = BTreeSet::from(["SKILL.md".to_owned()]);
    consumed_files.extend(profiles.keys().cloned());
    consumed_files.extend(javascript_modules.keys().cloned());
    consumed_files.extend(execution_files.iter().cloned());
    consumed_files.extend(harness_files.iter().cloned());
    consumed_files.extend(validate_operator_reference_files(&source)?);
    consumed_files.extend(validate_nested_package_consumed_files(&source)?);
    if let Some(mark) = source.files.get(crate::mark::MARK_FILE) {
        crate::mark::validate_mark_svg(mark).map_err(|reason| {
            SkillPackageError::invalid(
                crate::mark::MARK_FILE,
                format!("{} {reason}", crate::mark::MARK_FILE),
            )
        })?;
        consumed_files.insert(crate::mark::MARK_FILE.to_owned());
    }
    let source_digests = source
        .files
        .iter()
        .map(|(path, contents)| (path.clone(), sha256_prefixed(contents)))
        .collect::<BTreeMap<_, _>>();
    let manual_digest = source_digests
        .get("SKILL.md")
        .cloned()
        .ok_or_else(|| SkillPackageError::invalid("SKILL.md", "manual digest is missing"))?;
    let package_digest = package_digest(&source.files);
    Ok(ValidatedSkillPackage {
        skill,
        profiles,
        manual_markdown,
        manual_digest,
        package_digest,
        source_digests,
        javascript_modules,
        tools,
        execution_files,
        harness_files,
        consumed_files,
        harness_fixtures,
        context_skill_refs,
        source,
    })
}

fn validate_catalog_semantics(
    skill_name: &str,
    manifest: Option<&crate::SkillRunnerManifest>,
    fixtures: &BTreeMap<String, crate::harness_fixture::HarnessFixture>,
) -> Result<(), SkillPackageError> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    if manifest
        .catalog
        .as_ref()
        .is_none_or(|catalog| catalog.visibility != crate::CatalogVisibility::Public)
    {
        return Ok(());
    }
    let report = analyze_package_catalog_semantics(skill_name, manifest, fixtures);
    if report.diagnostics.is_empty() {
        return Ok(());
    }
    let corrections = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{} on runner {:?}: {}",
                diagnostic.code.as_str(),
                diagnostic.runner,
                diagnostic.required_correction
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Err(SkillPackageError::invalid(
        "run.yaml.catalog",
        format!("catalog semantic enforcement rejected the package: {corrections}"),
    ))
}

fn package_digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    let capacity = files
        .iter()
        .map(|(path, contents)| path.len().saturating_add(contents.len()).saturating_add(16))
        .sum();
    let mut canonical = Vec::with_capacity(capacity);
    canonical.extend_from_slice(b"runx.skill-package.v1\0");
    for (path, contents) in files {
        append_digest_field(&mut canonical, path.as_bytes());
        append_digest_field(&mut canonical, contents);
    }
    sha256_prefixed(&canonical)
}

fn append_digest_field(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}
