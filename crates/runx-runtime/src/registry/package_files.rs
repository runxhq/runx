use std::path::Path;

use runx_contracts::sha256_prefixed;
use runx_parser::{
    SkillPackageError, SkillPackageSource, ValidatedSkillPackage, validate_skill_package,
};
use serde::Serialize;

use super::types::RegistryPackageFile;

/// Script permissions follow the signed source bytes, not the publisher's
/// filesystem. This is shared by installation and publication staging.
pub(crate) fn restore_script_permissions(path: &Path, content: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    if content.starts_with("#!") {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = (path, content);
    Ok(())
}

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

/// The manual at the root of every skill package.
pub(crate) const MANUAL_FILE: &str = "SKILL.md";

/// A skill version's content digest, as the registry signs it: its manual,
/// its profile when it has one, and every package file.
pub fn skill_content_digest(
    markdown: &str,
    profile_document: Option<&str>,
    files: &[RegistryPackageFile],
) -> Result<String, runx_contracts::RegistryContentDigestError> {
    let manual = runx_contracts::RegistryContentFile {
        path: MANUAL_FILE,
        content: markdown,
    };
    let profile = profile_document.map(|content| runx_contracts::RegistryContentFile {
        path: runx_parser::PROFILE_FILE,
        content,
    });
    runx_contracts::registry_content_digest(
        std::iter::once(manual)
            .chain(profile)
            .chain(
                files
                    .iter()
                    .map(|file| runx_contracts::RegistryContentFile {
                        path: &file.path,
                        content: &file.content,
                    }),
            ),
    )
}

pub(crate) fn registry_bundle_digest(files: &[RegistryPackageFile]) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let mut sorted = files.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    let canonical = serde_json::to_vec(&RegistryBundleDigestDocument {
        files: sorted
            .into_iter()
            .map(|file| RegistryBundleDigestFile {
                content: &file.content,
                path: &file.path,
            })
            .collect(),
    })
    .ok()?;
    Some(sha256_prefixed(&canonical))
}

#[derive(Serialize)]
struct RegistryBundleDigestDocument<'a> {
    files: Vec<RegistryBundleDigestFile<'a>>,
}

#[derive(Serialize)]
struct RegistryBundleDigestFile<'a> {
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
    use super::registry_bundle_digest;
    use crate::registry::types::RegistryPackageFile;

    #[test]
    fn bundle_digest_uses_locale_independent_path_ordering() {
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
            registry_bundle_digest(&files).as_deref(),
            Some("sha256:70f0608aac8193a5b10fdf5f9b9d5d3be4f836ef6a8d8ce58b803698cada3d90")
        );
    }
}
