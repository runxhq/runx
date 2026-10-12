use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

mod packet;

use super::RegistryPublishPackageError;
use crate::LoadedSkillPackage;
use crate::registry::{RegistryConnectorMetadata, RegistryPackageFile};
use crate::skill_package::{MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};

/// One package's files, its plugin connectors among them, and the connectors
/// it declares.
pub(super) struct CollectedPackage {
    pub(super) files: Vec<RegistryPackageFile>,
    pub(super) connectors: Vec<RegistryConnectorMetadata>,
}

pub(super) fn collect_publish_package_files(
    loaded: &LoadedSkillPackage,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    packet_ids: &BTreeSet<String>,
) -> Result<CollectedPackage, RegistryPublishPackageError> {
    let mut files = BTreeMap::new();
    for relative in &loaded.package.consumed_files {
        if relative == "SKILL.md" || loaded.profile_path.as_deref() == Some(relative) {
            continue;
        }
        let contents = loaded.package.source.files.get(relative).ok_or_else(|| {
            RegistryPublishPackageError::invalid(format!(
                "parser-owned package file {relative} is missing from validated package source"
            ))
        })?;
        insert_source_file(&mut files, relative, contents)?;
    }
    packet::append_declared_packet_schemas(&mut files, loaded, env, cwd, packet_ids)?;
    let connectors = super::connectors::package_connectors(
        loaded,
        &mut files,
        crate::RunxPaths::resolve(env, cwd).installed_connectors(),
    )?;
    validate_package_limits(&files)?;
    Ok(CollectedPackage {
        files: files.into_values().collect(),
        connectors,
    })
}

pub(super) fn insert_source_file(
    files: &mut BTreeMap<String, RegistryPackageFile>,
    relative: &str,
    contents: &[u8],
) -> Result<(), RegistryPublishPackageError> {
    if should_reject_publish_file(relative) {
        return Err(RegistryPublishPackageError::invalid(format!(
            "publish package file {relative} looks like a secret or local credential"
        )));
    }
    let content = std::str::from_utf8(contents).map_err(|error| {
        RegistryPublishPackageError::invalid(format!(
            "publish package file {relative} must be UTF-8 text: {error}"
        ))
    })?;
    let file = RegistryPackageFile {
        path: relative.to_owned(),
        content: content.to_owned(),
    };
    if let Some(existing) = files.insert(relative.to_owned(), file)
        && existing.content != content
    {
        return Err(RegistryPublishPackageError::invalid(format!(
            "publish package path {relative} resolves to conflicting content"
        )));
    }
    Ok(())
}

pub(super) fn validate_package_limits(
    files: &BTreeMap<String, RegistryPackageFile>,
) -> Result<(), RegistryPublishPackageError> {
    let total_bytes = files.values().try_fold(0usize, |total, file| {
        total.checked_add(file.content.len()).ok_or_else(|| {
            RegistryPublishPackageError::invalid("publish package byte count overflow")
        })
    })?;
    if files.len() > MAX_PACKAGE_FILES || total_bytes > MAX_PACKAGE_BYTES {
        return Err(RegistryPublishPackageError::invalid(format!(
            "publish package exceeds {MAX_PACKAGE_FILES} files or {MAX_PACKAGE_BYTES} total bytes"
        )));
    }
    Ok(())
}

fn should_reject_publish_file(relative: &str) -> bool {
    let file_name = relative.rsplit('/').next().unwrap_or(relative);
    let lower = file_name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || matches!(
            lower.as_str(),
            ".npmrc"
                | ".pypirc"
                | ".netrc"
                | "credentials.json"
                | "credential.json"
                | "secrets.json"
                | "secret.json"
                | "id_rsa"
                | "id_ed25519"
        )
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
}

#[cfg(test)]
mod tests {
    use super::should_reject_publish_file;

    #[test]
    fn secret_like_names_are_rejected() {
        for path in [
            ".env",
            ".env.local",
            ".npmrc",
            "credentials.json",
            "nested/secrets.json",
            "private.pem",
            "tls/client.key",
            "id_ed25519",
        ] {
            assert!(should_reject_publish_file(path), "{path}");
        }
    }
}
