use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::{JsonObject, JsonValue};

use crate::RuntimeError;

pub(super) fn ignored_package_entry(name: &str) -> bool {
    crate::skill_package::ignored_package_entry(name)
}

pub(super) fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, RuntimeError> {
    let canonical = fs::canonicalize(path).map_err(|source| {
        RuntimeError::io(format!("resolving {label} {}", path.display()), source)
    })?;
    if !canonical.is_dir() {
        return Err(invalid_skill_change(format!("{label} must be a directory")));
    }
    Ok(canonical)
}

pub(super) fn normalize_child_path(value: &str) -> Result<PathBuf, RuntimeError> {
    if value.trim() != value || value.is_empty() || value.contains('\\') {
        return Err(invalid_skill_change(
            "target_dir must be a non-empty repo-relative POSIX path",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(invalid_skill_change(
                    "target_dir must stay inside repo_root",
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(invalid_skill_change("target_dir must name a child path"));
    }
    Ok(normalized)
}

pub(super) fn normalize_package_file(value: String) -> Result<String, RuntimeError> {
    let path = normalize_child_path(&value)?;
    Ok(display_relative(&path))
}

pub(super) fn assert_allowed_package_write_path(
    relative: &str,
    target: &Path,
    mode: &str,
) -> Result<(), RuntimeError> {
    let basename = relative.rsplit('/').next().unwrap_or(relative);
    if is_auxiliary_doc(basename) && !(mode == "improve" && target.join(relative).is_file()) {
        return Err(invalid_skill_change(format!(
            "{basename} is auxiliary package bloat; keep operating guidance in SKILL.md"
        )));
    }
    assert_harness_path(relative, mode)
}

pub(super) fn assert_allowed_package_delete_path(
    relative: &str,
    mode: &str,
) -> Result<(), RuntimeError> {
    let basename = relative.rsplit('/').next().unwrap_or(relative);
    if is_auxiliary_doc(basename) {
        return Err(invalid_skill_change(format!(
            "{basename} is an existing public package surface and cannot be deleted by skill-build"
        )));
    }
    assert_harness_path(relative, mode)
}

fn is_auxiliary_doc(basename: &str) -> bool {
    matches!(
        basename,
        "README.md" | "CHANGELOG.md" | "INSTALLATION_GUIDE.md" | "QUICK_REFERENCE.md"
    )
}

fn assert_harness_path(relative: &str, mode: &str) -> Result<(), RuntimeError> {
    if mode == "harness"
        && !(relative.starts_with("fixtures/")
            && (relative.ends_with(".yaml") || relative.ends_with(".yml")))
    {
        return Err(invalid_skill_change(format!(
            "harness mode may only change fixtures/*.yaml files: {relative}"
        )));
    }
    Ok(())
}

pub(super) fn reject_secret_material(relative: &str, contents: &str) -> Result<(), RuntimeError> {
    let contains_private_key = contents.lines().any(is_private_key_header);
    let contains_declared_secret = declared_secret_prefixes()?
        .iter()
        .any(|prefix| contents.contains(prefix.as_str()));
    if contains_private_key || contains_declared_secret {
        return Err(invalid_skill_change(format!(
            "refusing secret-like material in {relative}"
        )));
    }
    Ok(())
}

/// The prefixes official connectors declare for their secrets.
fn declared_secret_prefixes() -> Result<&'static [String], RuntimeError> {
    static PREFIXES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    if let Some(prefixes) = PREFIXES.get() {
        return Ok(prefixes);
    }
    let connectors = crate::kernel::connectors::ConnectorScope::official_connectors()
        .map_err(|error| invalid_skill_change(error.to_string()))?;
    let prefixes = connectors
        .iter()
        .flat_map(|connector| connector.manifest.auth.credentials.values())
        .flat_map(|declaration| declaration.fields.iter())
        .flat_map(|field| &field.prefixes)
        .map(|prefix| prefix.as_str().to_owned())
        .collect();
    Ok(PREFIXES.get_or_init(|| prefixes))
}

fn is_private_key_header(line: &str) -> bool {
    let Some(label) = line
        .trim()
        .strip_prefix("-----BEGIN ")
        .and_then(|value| value.strip_suffix("-----"))
    else {
        return false;
    };
    matches!(
        label,
        "PRIVATE KEY" | "RSA PRIVATE KEY" | "EC PRIVATE KEY" | "OPENSSH PRIVATE KEY"
    )
}

pub(super) fn is_executable_path(relative: &str) -> bool {
    ["cjs", "js", "mjs", "ts", "tsx", "py", "rb", "rs", "sh"]
        .iter()
        .any(|extension| relative.ends_with(&format!(".{extension}")))
}

pub(super) fn required_string(object: &JsonObject, field: &str) -> Result<String, RuntimeError> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid_skill_change(format!("{field} must be a non-empty string")))
}

pub(super) fn display_repo_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(display_relative)
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

pub(super) fn display_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            Component::CurDir => Some(".".to_owned()),
            Component::ParentDir => Some("..".to_owned()),
            Component::RootDir | Component::Prefix(_) => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn json_string<'a>(value: &'a JsonValue, field: &str) -> &'a str {
    value
        .as_object()
        .and_then(|object| object.get(field))
        .and_then(JsonValue::as_str)
        .unwrap_or("")
}

pub(super) fn unique_stage_root(repo_root: &Path, target: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let target_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("skill");
    let stage_name = format!(
        ".runx-candidate-{target_name}-{}-{nanos}",
        std::process::id()
    );
    // A candidate is staged beside its target so it validates in the same
    // place in the workspace; a target without a parent yet stages at the root.
    target
        .parent()
        .filter(|parent| parent.is_dir())
        .unwrap_or(repo_root)
        .join(stage_name)
}

pub(super) fn invalid_skill_change(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "runx.skill.apply".to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::reject_secret_material;

    #[test]
    fn secret_material_rejection_detects_private_key_headers_without_static_secrets() {
        let private_key = format!("-----BEGIN {}-----\nfixture", "PRIVATE KEY");
        assert!(reject_secret_material("tools/key.pem", &private_key).is_err());
        assert!(reject_secret_material("tools/readme.txt", "PRIVATE KEY").is_ok());
    }

    #[test]
    fn secret_material_rejection_detects_declared_key_prefixes() {
        let key = ["phx", "fixture"].join("_");
        assert!(reject_secret_material("tools/provider.mjs", &key).is_err());
        assert!(reject_secret_material("tools/provider.mjs", "phx-fixture").is_ok());
    }
}
