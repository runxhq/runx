use std::path::Path;

use runx_contracts::sha256_hex;
use runx_parser::{
    SkillPackageError, SkillPackageSource, ValidatedSkillPackage, validate_skill_package,
};
use serde::Serialize;

use super::types::RegistryPackageFile;

pub(crate) fn normalize_registry_package_files(
    files: Vec<RegistryPackageFile>,
) -> Result<Vec<RegistryPackageFile>, String> {
    for file in &files {
        validate_registry_package_file_path(&file.path)?;
    }
    let mut normalized = files;
    normalized.sort_by(|left, right| left.path.cmp(&right.path));
    if let Some(duplicate) = normalized
        .windows(2)
        .find(|pair| pair[0].path == pair[1].path)
    {
        return Err(format!("duplicate package file '{}'", duplicate[0].path));
    }
    Ok(normalized)
}

pub(crate) fn registry_package_digest(files: &[RegistryPackageFile]) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let mut sorted = files.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    let canonical = serde_json::to_vec(&RegistryPackageDigestDocument {
        files: sorted
            .into_iter()
            .map(|file| RegistryPackageDigestFile {
                content: &file.content,
                path: &file.path,
            })
            .collect(),
    })
    .ok()?;
    Some(sha256_hex(&canonical))
}

#[derive(Serialize)]
struct RegistryPackageDigestDocument<'a> {
    files: Vec<RegistryPackageDigestFile<'a>>,
}

#[derive(Serialize)]
struct RegistryPackageDigestFile<'a> {
    content: &'a str,
    path: &'a str,
}

pub(crate) fn validate_registry_package_file_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("package file path cannot be empty".to_owned());
    }
    if path.contains('\\') {
        return Err(format!("package file path '{path}' must use / separators"));
    }
    let parsed = Path::new(path);
    if parsed.is_absolute() {
        return Err(format!("package file path '{path}' must be relative"));
    }
    let mut depth = 0usize;
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment.starts_with('.') {
            return Err(format!(
                "package file path '{path}' contains an unsafe segment"
            ));
        }
        depth += 1;
    }
    if depth == 1 && matches!(path, "SKILL.md" | runx_parser::PROFILE_FILE) {
        return Err(format!("{path} is stored in its dedicated registry field"));
    }
    Ok(())
}

pub(crate) fn validate_registry_skill_package(
    markdown: &str,
    profile_document: Option<&str>,
    files: &[RegistryPackageFile],
) -> Result<ValidatedSkillPackage, SkillPackageError> {
    let mut source = SkillPackageSource::from_documents(
        markdown.to_owned(),
        profile_document.map(str::to_owned),
    );
    for file in files {
        if source
            .files
            .insert(file.path.clone(), file.content.as_bytes().to_vec())
            .is_some()
        {
            return Err(SkillPackageError::invalid(
                &file.path,
                "package source contains a duplicate path",
            ));
        }
    }
    validate_skill_package(source)
}

#[cfg(test)]
mod tests {
    use super::registry_package_digest;
    use crate::registry::types::RegistryPackageFile;

    #[test]
    fn package_digest_uses_locale_independent_path_ordering() {
        let files = vec![
            RegistryPackageFile {
                path: "graph/plan/run.yaml".to_owned(),
                content: "graph\n".to_owned(),
            },
            RegistryPackageFile {
                path: "graph/plan/SKILL.md".to_owned(),
                content: "manual\n".to_owned(),
            },
        ];

        assert_eq!(
            registry_package_digest(&files).as_deref(),
            Some("70f0608aac8193a5b10fdf5f9b9d5d3be4f836ef6a8d8ce58b803698cada3d90")
        );
    }
}
