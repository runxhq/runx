//! Retain an immutable local execution binding so a paused native run can use
//! the same skill closure after its authored source is patched.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ring::rand::{SecureRandom, SystemRandom};
use runx_contracts::sha256_prefixed;

use super::inspect_skill_package;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SkillRetentionError(String);

pub fn inspect_local_skill_binding(
    source: &Path,
    env: Option<&BTreeMap<String, String>>,
) -> Result<String, SkillRetentionError> {
    let inspected = inspect_skill_package(source, None, env)
        .map_err(|error| SkillRetentionError(error.to_string()))?;
    skill_binding_digest(&inspected_skill_bindings(&inspected).map_err(SkillRetentionError)?)
        .map_err(SkillRetentionError)
}

pub fn retain_skill_binding(
    store_root: &Path,
    source: &Path,
    name: &str,
    expected_digest: &str,
) -> Result<PathBuf, SkillRetentionError> {
    pinned_skill_path_impl(store_root, source, name, expected_digest).map_err(SkillRetentionError)
}

pub fn resolve_retained_skill_binding(
    store_root: &Path,
    name: &str,
    digest: &str,
) -> Result<PathBuf, SkillRetentionError> {
    pinned_skill_path_by_digest_impl(store_root, name, digest).map_err(SkillRetentionError)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// One local skill binding includes every package reached by any declared
/// runner. A sibling patch gets a new binding while an already pinned run
/// keeps the exact earlier closure.
fn inspected_skill_bindings(
    inspected: &runx_contracts::JsonValue,
) -> Result<BTreeMap<String, (String, PathBuf)>, String> {
    let value = serde_json::to_value(inspected)
        .map_err(|error| format!("encoding skill inspection: {error}"))?;
    let name = value["name"]
        .as_str()
        .ok_or("skill inspection has no name")?;
    let digest = value["package_digest"]
        .as_str()
        .ok_or("skill inspection has no package digest")?;
    let path = value["skill_path"]
        .as_str()
        .ok_or("skill inspection has no path")?;
    let mut bindings =
        BTreeMap::from([(name.to_owned(), (digest.to_owned(), PathBuf::from(path)))]);
    for runner in value["runner_inspections"].as_array().into_iter().flatten() {
        let packages = runner["execution_closure"]["package_bindings"]
            .as_array()
            .ok_or("skill runner has no package bindings")?;
        for package in packages {
            if package["source_kind"] != "source_root" {
                return Err("skill closure must use local source packages".to_owned());
            }
            let package_name = package["skill"]
                .as_str()
                .ok_or("skill closure package has no name")?;
            let package_digest = package["package_digest"]
                .as_str()
                .ok_or("skill closure package has no digest")?;
            let package_path = package["source_path"]
                .as_str()
                .ok_or("skill closure package has no source path")?;
            let binding = (package_digest.to_owned(), PathBuf::from(package_path));
            if let Some(existing) = bindings.insert(package_name.to_owned(), binding.clone())
                && existing != binding
            {
                return Err("skill closure binds a skill to conflicting packages".to_owned());
            }
        }
    }
    Ok(bindings)
}

fn skill_binding_digest(bindings: &BTreeMap<String, (String, PathBuf)>) -> Result<String, String> {
    if bindings.len() == 1 {
        return Ok(bindings
            .values()
            .next()
            .ok_or("empty skill binding")?
            .0
            .clone());
    }
    let digests = bindings
        .iter()
        .map(|(name, (digest, _))| (name, digest))
        .collect::<BTreeMap<_, _>>();
    Ok(sha256_prefixed(&serde_json::to_vec(&digests).map_err(
        |error| format!("encoding skill closure binding: {error}"),
    )?))
}

fn pinned_skill_path_by_digest_impl(
    store_root: &Path,
    name: &str,
    digest: &str,
) -> Result<PathBuf, String> {
    if !valid_identifier(name) || !valid_digest(digest) {
        return Err("retained skill binding has an invalid name or digest".to_owned());
    }
    let path = store_root
        .join(name)
        .join(digest.trim_start_matches("sha256:"));
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("reading pinned {name} skill package: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "pinned {name} skill package is unavailable or changed"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("protecting pinned {name} skill package: {error}"))?;
    }
    resolve_pinned_skill(&path, name, digest)
}

fn package_digest(path: &Path) -> Result<String, String> {
    let inspected = inspect_skill_package(path, None, None)
        .map_err(|error| format!("inspecting skill package: {error}"))?;
    inspected
        .as_object()
        .and_then(|object| object.get("package_digest"))
        .and_then(runx_contracts::JsonValue::as_str)
        .map(str::to_owned)
        .ok_or("skill inspection has no package digest".to_owned())
}

fn resolve_pinned_skill(root: &Path, name: &str, digest: &str) -> Result<PathBuf, String> {
    if root.join("SKILL.md").is_file() {
        if package_digest(root)? != digest {
            return Err(format!("pinned {name} skill package has changed"));
        }
        return Ok(root.to_path_buf());
    }
    let bytes = fs::read(root.join(".assistant-bundle.json"))
        .map_err(|error| format!("reading pinned {name} closure binding: {error}"))?;
    let digests: BTreeMap<String, String> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("decoding pinned {name} closure binding: {error}"))?;
    if digests.len() < 2 || !digests.contains_key(name) {
        return Err(format!("pinned {name} closure is incomplete"));
    }
    let bindings = digests
        .iter()
        .map(|(skill, package_digest)| (skill.clone(), (package_digest.clone(), root.join(skill))))
        .collect::<BTreeMap<_, _>>();
    if skill_binding_digest(&bindings)? != digest {
        return Err(format!("pinned {name} closure binding has changed"));
    }
    for (skill, (expected, path)) in &bindings {
        if !valid_identifier(skill)
            || fs::symlink_metadata(path)
                .map_err(|error| format!("reading pinned {skill} package: {error}"))?
                .file_type()
                .is_symlink()
            || package_digest(path)? != *expected
        {
            return Err(format!("pinned {skill} closure package has changed"));
        }
    }
    Ok(root.join(name))
}

/// A native checkpoint resumes from this exact content-addressed directory.
/// Patch installs can replace the source package without changing a running
/// checkpoint's code. The digest is checked again before every invocation.
fn pinned_skill_path_impl(
    store_root: &Path,
    source: &Path,
    name: &str,
    expected_digest: &str,
) -> Result<PathBuf, String> {
    if !valid_identifier(name) || !valid_digest(expected_digest) {
        return Err("retained skill binding has an invalid name or digest".to_owned());
    }
    let inspected = inspect_skill_package(source, None, None)
        .map_err(|error| format!("inspecting skill package: {error}"))?;
    let bindings = inspected_skill_bindings(&inspected)?;
    if !bindings.contains_key(name) {
        return Err(format!("retained binding does not contain skill {name}"));
    }
    let digest = skill_binding_digest(&bindings)?;
    if digest != expected_digest {
        return Err(format!(
            "{name} skill changed during this turn; retry with fresh bindings"
        ));
    }
    let root = store_root.join(name);
    let target = root.join(digest.trim_start_matches("sha256:"));
    fs::create_dir_all(&root).map_err(|error| format!("creating skill package store: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for directory in [
            root.parent().and_then(Path::parent),
            root.parent(),
            Some(root.as_path()),
        ]
        .into_iter()
        .flatten()
        {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("protecting skill package store: {error}"))?;
        }
    }
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))
                    .map_err(|error| format!("protecting pinned skill package: {error}"))?;
            }
            return resolve_pinned_skill(&target, name, &digest);
        }
        Ok(_) => {
            return Err(format!(
                "pinned {name} skill package is not a regular directory"
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("checking pinned skill package: {error}")),
    }
    let mut nonce = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "creating skill package nonce failed".to_owned())?;
    let stage = root.join(format!(".stage-{:032x}", u128::from_le_bytes(nonce)));
    fs::create_dir(&stage).map_err(|error| format!("staging skill package: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("protecting staged skill package: {error}"))?;
    }
    let staged = (|| {
        if bindings.len() == 1 {
            copy_skill_files(source, &stage)?;
        } else {
            let parent = source
                .parent()
                .ok_or("skill has no package parent")?
                .canonicalize()
                .map_err(|error| format!("resolving skill root: {error}"))?;
            let mut manifest = BTreeMap::new();
            for (skill, (package_digest, path)) in &bindings {
                let canonical = path
                    .canonicalize()
                    .map_err(|error| format!("resolving {skill} closure package: {error}"))?;
                if !valid_identifier(skill)
                    || canonical.parent() != Some(parent.as_path())
                    || canonical.file_name().and_then(|name| name.to_str()) != Some(skill)
                {
                    return Err("skill closure leaves its local sibling root".to_owned());
                }
                let destination = stage.join(skill);
                fs::create_dir(&destination)
                    .map_err(|error| format!("staging {skill} closure package: {error}"))?;
                copy_skill_files(&canonical, &destination)?;
                manifest.insert(skill.clone(), package_digest.clone());
            }
            fs::write(
                stage.join(".assistant-bundle.json"),
                serde_json::to_vec(&manifest)
                    .map_err(|error| format!("encoding closure binding: {error}"))?,
            )
            .map_err(|error| format!("writing closure binding: {error}"))?;
        }
        if resolve_pinned_skill(&stage, name, &digest).is_err() {
            return Err("skill changed while its package was being pinned".to_owned());
        }
        match fs::rename(&stage, &target) {
            Ok(()) => Ok(()),
            Err(_) if target.exists() && resolve_pinned_skill(&target, name, &digest).is_ok() => {
                Ok(())
            }
            Err(error) => Err(format!("committing skill package: {error}")),
        }
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    staged?;
    resolve_pinned_skill(&target, name, &digest)
}

fn copy_skill_files(source: &Path, target: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|error| format!("reading skill package: {error}"))? {
        let entry = entry.map_err(|error| format!("reading skill package entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("checking skill package entry: {error}"))?;
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir(&destination)
                .map_err(|error| format!("creating skill package directory: {error}"))?;
            copy_skill_files(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination)
                .map_err(|error| format!("copying skill package file: {error}"))?;
        } else {
            return Err("skill packages cannot contain symbolic links or special files".to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{resolve_retained_skill_binding, retain_skill_binding};
    use std::path::Path;

    #[test]
    fn retained_binding_paths_reject_untrusted_segments() {
        let store = Path::new("/tmp/runx-retained-binding-test");
        let source = Path::new("/tmp/runx-retained-binding-test-source");
        let digest = format!("sha256:{}", "a".repeat(64));
        assert!(resolve_retained_skill_binding(store, "../other", &digest).is_err());
        assert!(resolve_retained_skill_binding(store, "skill", "../other").is_err());
        assert!(retain_skill_binding(store, source, "../other", &digest).is_err());
        assert!(retain_skill_binding(store, source, "skill", "sha256:xyz").is_err());
    }
}
