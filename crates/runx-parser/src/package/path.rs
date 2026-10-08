use std::collections::BTreeMap;

use super::SkillPackageError;

pub(super) fn validate_source_paths(
    files: &BTreeMap<String, Vec<u8>>,
    mut symlinks: impl Iterator<Item = String>,
) -> Result<(), SkillPackageError> {
    if let Some(path) = symlinks.next() {
        validate_package_path(&path)?;
        return Err(SkillPackageError::invalid(
            path,
            "symbolic links are not valid skill package sources",
        ));
    }
    for path in files.keys() {
        validate_package_path(path)?;
    }
    Ok(())
}

pub(super) fn validate_package_path(path: &str) -> Result<(), SkillPackageError> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return Err(SkillPackageError::invalid(
            path,
            "package paths must be non-empty relative POSIX paths",
        ));
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(SkillPackageError::invalid(
            path,
            "package paths must not contain empty, '.' or '..' segments",
        ));
    }
    Ok(())
}

pub(super) fn normalize_module_import(
    importer: &str,
    specifier: &str,
) -> Result<String, SkillPackageError> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return Err(SkillPackageError::invalid(
            importer,
            format!(
                "unsupported JavaScript import {specifier:?}; deterministic modules may import only relative package .js/.mjs files"
            ),
        ));
    }
    if specifier.contains('\\') {
        return Err(SkillPackageError::invalid(
            importer,
            format!("JavaScript import {specifier:?} must use POSIX separators"),
        ));
    }

    let mut segments = importer
        .rsplit_once('/')
        .map(|(parent, _)| parent.split('/').collect::<Vec<_>>())
        .unwrap_or_default();
    for segment in specifier.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(SkillPackageError::invalid(
                        importer,
                        format!("JavaScript import {specifier:?} escapes the skill package"),
                    ));
                }
            }
            value => segments.push(value),
        }
    }
    let resolved = segments.join("/");
    validate_package_path(&resolved)?;
    if !matches!(resolved.rsplit_once('.'), Some((_, "js" | "mjs"))) {
        return Err(SkillPackageError::invalid(
            importer,
            format!("JavaScript import {specifier:?} must resolve to an explicit .js or .mjs file"),
        ));
    }
    Ok(resolved)
}

pub(super) fn normalize_context_ref(
    profile_dir: &str,
    reference: &str,
) -> Result<Option<String>, SkillPackageError> {
    if reference.starts_with("registry:")
        || reference.starts_with("runx-registry:")
        || reference.starts_with("runx://skill/")
    {
        return Ok(None);
    }
    if reference.trim() != reference
        || reference.is_empty()
        || reference.starts_with('/')
        || reference.contains('\\')
    {
        return Err(SkillPackageError::invalid(
            super::PROFILE_FILE,
            format!(
                "context skill ref {reference:?} must be a relative POSIX path or registry ref"
            ),
        ));
    }
    let segments = reference
        .split('/')
        .filter(|segment| *segment != ".")
        .collect::<Vec<_>>();
    if segments.is_empty()
        || segments
            .iter()
            .any(|segment| segment.is_empty() || *segment == "..")
    {
        return Err(SkillPackageError::invalid(
            super::PROFILE_FILE,
            format!("context skill ref {reference:?} must not traverse the package"),
        ));
    }
    if segments.contains(&"graph") {
        return Err(SkillPackageError::invalid(
            super::PROFILE_FILE,
            format!("context skill ref {reference:?} must not target an internal graph stage"),
        ));
    }
    let relative = segments.join("/");
    Ok(Some(if profile_dir.is_empty() {
        format!("{relative}/SKILL.md")
    } else {
        format!("{profile_dir}/{relative}/SKILL.md")
    }))
}
