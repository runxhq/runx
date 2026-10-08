use std::collections::BTreeMap;

use crate::{
    SkillRunnerManifest, ValidatedSkill, parse_runner_manifest_yaml, parse_skill_markdown,
    validate_runner_manifest, validate_skill,
};

use super::{SkillPackageError, SkillPackageSource};

pub(super) fn required_text_file<'a>(
    source: &'a SkillPackageSource,
    path: &str,
) -> Result<&'a str, SkillPackageError> {
    let bytes = source
        .files
        .get(path)
        .ok_or_else(|| SkillPackageError::invalid(path, "required package file is missing"))?;
    text_file(path, bytes)
}

pub(super) fn text_file<'a>(path: &str, bytes: &'a [u8]) -> Result<&'a str, SkillPackageError> {
    std::str::from_utf8(bytes).map_err(|error| {
        SkillPackageError::invalid(path, format!("parser input must be UTF-8: {error}"))
    })
}

pub(super) fn validate_manual(markdown: &str) -> Result<ValidatedSkill, SkillPackageError> {
    let parsed = parse_skill_markdown(markdown).map_err(|source| SkillPackageError::Parse {
        path: "SKILL.md".to_owned(),
        source,
    })?;
    validate_skill(parsed).map_err(|source| SkillPackageError::Validation {
        path: "SKILL.md".to_owned(),
        source,
    })
}

pub(super) fn validate_profiles(
    source: &SkillPackageSource,
) -> Result<BTreeMap<String, SkillRunnerManifest>, SkillPackageError> {
    owned_profile_paths(source)
        .into_iter()
        .map(|path| {
            let contents = source
                .files
                .get(&path)
                .ok_or_else(|| SkillPackageError::invalid(&path, "profile source is missing"))?;
            let manifest = validate_manifest(&path, text_file(&path, contents)?)?;
            Ok((path, manifest))
        })
        .collect()
}

fn owned_profile_paths(source: &SkillPackageSource) -> Vec<String> {
    source
        .files
        .keys()
        .filter(|path| path.as_str() == super::super::PROFILE_FILE || path.ends_with("/run.yaml"))
        .filter(|path| !has_nested_manual_boundary(path, source))
        .cloned()
        .collect()
}

pub(super) fn has_nested_manual_boundary(path: &str, source: &SkillPackageSource) -> bool {
    let Some((directory, _)) = path.rsplit_once('/') else {
        return false;
    };
    let mut prefix = String::new();
    for segment in directory.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(segment);
        if source.files.contains_key(&format!("{prefix}/SKILL.md")) {
            return true;
        }
    }
    false
}

fn validate_manifest(path: &str, contents: &str) -> Result<SkillRunnerManifest, SkillPackageError> {
    let parsed =
        parse_runner_manifest_yaml(contents).map_err(|source| SkillPackageError::Parse {
            path: path.to_owned(),
            source,
        })?;
    let manifest =
        validate_runner_manifest(parsed).map_err(|source| SkillPackageError::Validation {
            path: path.to_owned(),
            source,
        })?;
    let defaults = manifest
        .runners
        .values()
        .filter(|runner| runner.default)
        .count();
    if defaults > 1 {
        return Err(SkillPackageError::invalid(
            format!("{path}.runners"),
            "runner selection is ambiguous: declare at most one default runner",
        ));
    }
    Ok(manifest)
}

pub(super) fn validate_package_identity(
    skill: &ValidatedSkill,
    manifest: Option<&SkillRunnerManifest>,
) -> Result<(), SkillPackageError> {
    let Some(manifest_name) = manifest.map(|manifest| manifest.skill.as_str()) else {
        return Ok(());
    };
    if manifest_name == skill.name {
        return Ok(());
    }
    Err(SkillPackageError::invalid(
        "run.yaml.skill",
        format!(
            "manifest skill {manifest_name:?} does not match SKILL.md name {:?}",
            skill.name
        ),
    ))
}
