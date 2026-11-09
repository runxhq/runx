use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{JsonValue, sha256_prefixed};

use super::path::validate_source_paths;
use super::{SkillPackageError, SkillPackageSource, ValidatedSkillPackage};

mod contract;
mod modules;
mod packets;
mod references;
mod support;
mod tools;

use contract::{
    required_text_file, text_file, validate_agent_task_sections, validate_manual, validate_profiles,
};
use modules::validate_modules;
use references::{
    collect_package_references, collect_test_fixture_references, validate_context_skill_sources,
};
use support::{
    validate_execution_files, validate_nested_package_consumed_files,
    validate_operator_reference_files, validate_test_fixtures, validate_test_methods,
    validate_test_support_files,
};
use tools::validate_package_tools;

pub fn validate_skill_package(
    source: SkillPackageSource,
) -> Result<ValidatedSkillPackage, SkillPackageError> {
    validate_source_paths(&source.files, source.symlinks.iter().cloned())?;
    let manual_markdown = required_text_file(&source, "SKILL.md")?.to_owned();
    let skill = validate_manual(&manual_markdown)?;
    let profiles = validate_profiles(&source, &skill)?;
    validate_agent_task_sections(&skill, profiles.get(super::PROFILE_FILE))?;
    let test_fixtures = validate_test_fixtures(&source)?;
    validate_test_methods(&test_fixtures, profiles.get(super::PROFILE_FILE))?;
    let references = collect_package_references(&profiles)?;
    let test_fixture_files = collect_test_fixture_references(&test_fixtures, &source)?;
    let tools = validate_package_tools(&source)?;
    validate_context_skill_sources(&source, &references.context_refs)?;
    let mut execution_files = references.execution_files;
    for package_tool in tools.values() {
        execution_files.insert(package_tool.manifest_path.clone());
        execution_files.extend(package_tool.source_files.iter().cloned());
    }
    validate_execution_files(&source, &execution_files)?;
    let mut test_files = validate_test_support_files(&source)?;
    test_files.extend(test_fixture_files);
    let mut context_skills = references
        .context_refs
        .into_iter()
        .map(|reference| reference.reference)
        .collect::<Vec<_>>();
    context_skills.sort();
    context_skills.dedup();
    let javascript_modules = validate_modules(&source, references.module_roots, &execution_files)?;
    let packets = packets::validate_package_packets(&source, &skill.name)?;
    let mut consumed_files = BTreeSet::from(["SKILL.md".to_owned()]);
    consumed_files.extend(packets.values().map(|packet| packet.path.clone()));
    consumed_files.extend(profiles.keys().cloned());
    consumed_files.extend(javascript_modules.keys().cloned());
    consumed_files.extend(execution_files.iter().cloned());
    consumed_files.extend(test_files.iter().cloned());
    consumed_files.extend(validate_operator_reference_files(&source)?);
    consumed_files.extend(validate_eval_files(&source, &skill.name)?);
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
        test_files,
        consumed_files,
        test_fixtures,
        context_skills,
        packets,
        source,
    })
}

/// The Agent Skills evaluation files: `evals/evals.json`, naming this skill
/// and holding its evals, and `eval_queries.json`, the queries a host should
/// or should not select this skill for. They evaluate a package; they are
/// never a requirement of its format.
fn validate_eval_files(
    source: &SkillPackageSource,
    skill: &str,
) -> Result<BTreeSet<String>, SkillPackageError> {
    let mut files = BTreeSet::new();
    for path in ["evals/evals.json", "eval_queries.json"] {
        let Some(contents) = source.files.get(path) else {
            continue;
        };
        let value = serde_json::from_str::<JsonValue>(text_file(path, contents)?)
            .map_err(|error| SkillPackageError::invalid(path, format!("is not JSON: {error}")))?;
        if path == "evals/evals.json" {
            validate_evals(path, &value, skill)?;
        } else {
            validate_eval_queries(path, &value)?;
        }
        files.insert(path.to_owned());
    }
    Ok(files)
}

fn validate_evals(path: &str, value: &JsonValue, skill: &str) -> Result<(), SkillPackageError> {
    let JsonValue::Object(evals) = value else {
        return Err(SkillPackageError::invalid(path, "is an object"));
    };
    if evals.get("skill_name").and_then(JsonValue::as_str) != Some(skill) {
        return Err(SkillPackageError::invalid(
            path,
            format!("skill_name is the package's name, {skill}"),
        ));
    }
    if !matches!(evals.get("evals"), Some(JsonValue::Array(_))) {
        return Err(SkillPackageError::invalid(path, "evals is a list"));
    }
    Ok(())
}

/// Each trigger query is exactly a non-empty `query` and a boolean
/// `should_trigger`, and the file holds at least one.
fn validate_eval_queries(path: &str, value: &JsonValue) -> Result<(), SkillPackageError> {
    let is_query = |entry: &JsonValue| {
        entry.as_object().is_some_and(|entry| {
            entry.len() == 2
                && entry
                    .get("query")
                    .and_then(JsonValue::as_str)
                    .is_some_and(|query| !query.trim().is_empty())
                && entry
                    .get("should_trigger")
                    .and_then(JsonValue::as_bool)
                    .is_some()
        })
    };
    match value {
        JsonValue::Array(queries) if !queries.is_empty() && queries.iter().all(is_query) => Ok(()),
        _ => Err(SkillPackageError::invalid(
            path,
            "is a list of { query, should_trigger }",
        )),
    }
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
