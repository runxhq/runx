use std::collections::{BTreeMap, BTreeSet};

use crate::{
    SkillMethodManifest,
    test_fixture::{TestFixture, parse_test_fixture},
};

use super::contract::{has_nested_manual_boundary, text_file};
use super::{SkillPackageError, SkillPackageSource, validate_skill_package};

pub(super) fn validate_nested_package_consumed_files(
    source: &SkillPackageSource,
) -> Result<BTreeSet<String>, SkillPackageError> {
    let mut consumed = BTreeSet::new();
    for prefix in immediate_nested_manual_prefixes(source) {
        let nested = validate_skill_package(nested_package_source(source, &prefix))
            .map_err(|error| error.with_path_prefix(&prefix))?;
        consumed.extend(
            nested
                .consumed_files
                .into_iter()
                .map(|path| format!("{prefix}/{path}")),
        );
    }
    Ok(consumed)
}

pub(super) fn validate_operator_reference_files(
    source: &SkillPackageSource,
) -> Result<BTreeSet<String>, SkillPackageError> {
    source
        .files
        .iter()
        .filter(|(path, _)| {
            (path.starts_with("references/") || path.contains("/references/"))
                && path.ends_with(".md")
                && !has_nested_manual_boundary(path, source)
        })
        .map(|(path, contents)| {
            text_file(path, contents)?;
            Ok(path.clone())
        })
        .collect()
}

fn immediate_nested_manual_prefixes(source: &SkillPackageSource) -> Vec<String> {
    source
        .files
        .keys()
        .filter_map(|path| path.strip_suffix("/SKILL.md"))
        .filter(|prefix| {
            let segments = prefix.split('/').collect::<Vec<_>>();
            (1..segments.len()).all(|length| {
                !source
                    .files
                    .contains_key(&format!("{}/SKILL.md", segments[..length].join("/")))
            })
        })
        .map(str::to_owned)
        .collect()
}

fn nested_package_source(source: &SkillPackageSource, prefix: &str) -> SkillPackageSource {
    let prefix = format!("{prefix}/");
    SkillPackageSource {
        files: source
            .files
            .iter()
            .filter_map(|(path, contents)| {
                path.strip_prefix(&prefix)
                    .map(|relative| (relative.to_owned(), contents.clone()))
            })
            .collect(),
        symlinks: source
            .symlinks
            .iter()
            .filter_map(|path| path.strip_prefix(&prefix).map(str::to_owned))
            .collect(),
    }
}

pub(super) fn validate_test_support_files(
    source: &SkillPackageSource,
    profiles: &BTreeMap<String, SkillMethodManifest>,
) -> Result<BTreeSet<String>, SkillPackageError> {
    let mut files = BTreeSet::new();
    for (profile_path, manifest) in profiles {
        let Some(harness) = &manifest.harness else {
            continue;
        };
        let profile_directory = profile_path
            .rsplit_once('/')
            .map_or("", |(directory, _)| directory);
        for (index, declared) in harness.files.iter().enumerate() {
            let field = format!("{profile_path}.harness.files[{index}]");
            if declared.trim() != declared
                || declared.is_empty()
                || declared.starts_with('/')
                || declared.contains('\\')
                || declared
                    .split('/')
                    .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
                || !declared.starts_with("fixtures/")
            {
                return Err(SkillPackageError::invalid(
                    field,
                    "harness files must be normalized profile-relative paths under fixtures/",
                ));
            }
            let resolved = if profile_directory.is_empty() {
                declared.clone()
            } else {
                format!("{profile_directory}/{declared}")
            };
            if !source.files.contains_key(&resolved) {
                return Err(SkillPackageError::invalid(
                    field,
                    format!("declared harness file {resolved:?} is missing from the package"),
                ));
            }
            files.insert(resolved);
        }
    }
    Ok(files)
}

pub(super) fn validate_execution_files(
    source: &SkillPackageSource,
    paths: &BTreeSet<String>,
) -> Result<(), SkillPackageError> {
    for path in paths {
        if !source.files.contains_key(path) {
            return Err(SkillPackageError::invalid(
                path,
                "declared execution sidecar is missing from the skill package",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_test_fixtures(
    source: &SkillPackageSource,
) -> Result<BTreeMap<String, TestFixture>, SkillPackageError> {
    source
        .files
        .iter()
        .filter(|(path, _)| {
            path.starts_with("fixtures/")
                && (path.ends_with(".yaml") || path.ends_with(".yml"))
                && !has_nested_manual_boundary(path, source)
        })
        .map(|(path, contents)| {
            let fixture = parse_test_fixture(text_file(path, contents)?).map_err(|error| {
                SkillPackageError::invalid(path, format!("invalid harness fixture: {error}"))
            })?;
            Ok((path.clone(), fixture))
        })
        .collect()
}
