use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::package_evals::{EvalQuery, PackageEval, PackageEvals};
use runx_contracts::sha256_prefixed;

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
    let eval_files = validate_eval_files(&source, &skill.name)?;
    consumed_files.extend(eval_files.paths);
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
        eval_queries: eval_files.queries,
        evals: eval_files.evals,
        source,
    })
}

/// A package's Agent Skills evaluation files, as it declares them.
struct EvalFiles {
    paths: BTreeSet<String>,
    queries: Vec<EvalQuery>,
    evals: Vec<PackageEval>,
}

const EVALS_FILE: &str = "evals/evals.json";
const EVAL_QUERIES_FILE: &str = "eval_queries.json";

/// The Agent Skills evaluation files: `evals/evals.json`, naming this skill
/// and holding its evals, and `eval_queries.json`, the queries a host should
/// or should not select this skill for, each validated against its contract.
/// They evaluate a package; they are never a requirement of its format.
fn validate_eval_files(
    source: &SkillPackageSource,
    skill: &str,
) -> Result<EvalFiles, SkillPackageError> {
    let mut files = EvalFiles {
        paths: BTreeSet::new(),
        queries: Vec::new(),
        evals: Vec::new(),
    };
    if let Some(contents) = source.files.get(EVALS_FILE) {
        files.evals = validate_evals(text_file(EVALS_FILE, contents)?, skill)?;
        files.paths.insert(EVALS_FILE.to_owned());
    }
    if let Some(contents) = source.files.get(EVAL_QUERIES_FILE) {
        files.queries = validate_eval_queries(text_file(EVAL_QUERIES_FILE, contents)?)?;
        files.paths.insert(EVAL_QUERIES_FILE.to_owned());
    }
    Ok(files)
}

/// The evals under this skill's name, each id listed once.
fn validate_evals(text: &str, skill: &str) -> Result<Vec<PackageEval>, SkillPackageError> {
    let file = serde_json::from_str::<PackageEvals>(text).map_err(|error| {
        SkillPackageError::invalid(
            EVALS_FILE,
            format!("is the Agent Skills evals format: {error}"),
        )
    })?;
    if file.skill_name.as_str() != skill {
        return Err(SkillPackageError::invalid(
            EVALS_FILE,
            format!("skill_name is the package's name, {skill}"),
        ));
    }
    let mut ids = BTreeSet::new();
    if let Some(eval) = file.evals.iter().find(|eval| !ids.insert(eval.id)) {
        return Err(SkillPackageError::invalid(
            EVALS_FILE,
            format!("eval {} is listed twice", eval.id),
        ));
    }
    Ok(file.evals)
}

/// Each trigger query is exactly a `query` with words in it and a boolean
/// `should_trigger`, and the file holds at least one.
fn validate_eval_queries(text: &str) -> Result<Vec<EvalQuery>, SkillPackageError> {
    match serde_json::from_str::<Vec<EvalQuery>>(text) {
        Ok(queries)
            if !queries.is_empty()
                && queries
                    .iter()
                    .all(|query| !query.query.as_str().trim().is_empty()) =>
        {
            Ok(queries)
        }
        _ => Err(SkillPackageError::invalid(
            EVAL_QUERIES_FILE,
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
