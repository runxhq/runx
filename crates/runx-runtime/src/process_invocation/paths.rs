use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::RuntimeError;
use crate::receipts::paths::RUNX_CWD_ENV;

pub(super) fn resolve_cwd(
    source_cwd: Option<&str>,
    skill_directory: &Path,
    workspace_cwd: Option<&Path>,
) -> Result<PathBuf, RuntimeError> {
    let cwd = match source_cwd {
        Some("{{env.RUNX_CWD}}") => workspace_cwd.map(Path::to_path_buf).ok_or_else(|| {
            invocation_error(format!(
                "process cwd requests {{env.RUNX_CWD}} but {RUNX_CWD_ENV} is not available"
            ))
        })?,
        Some(cwd) => resolve_path(skill_directory, cwd),
        None => skill_directory.to_path_buf(),
    };
    Ok(normalize_path(&cwd))
}

pub(super) fn workspace_cwd(
    env: &BTreeMap<String, String>,
) -> Result<Option<PathBuf>, RuntimeError> {
    let Some(value) = env.get(RUNX_CWD_ENV) else {
        return Ok(None);
    };
    let name = RUNX_CWD_ENV;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(invocation_error(format!(
            "{name} must be an absolute path, got '{}'",
            path.display()
        )));
    }
    Ok(Some(normalize_path(&path)))
}

pub(super) fn resolved_skill_directory(
    skill_directory: &Path,
    workspace_cwd: Option<&Path>,
) -> Result<PathBuf, RuntimeError> {
    let path = if skill_directory.is_absolute() {
        skill_directory.to_path_buf()
    } else {
        workspace_cwd
            .map(|workspace| workspace.join(skill_directory))
            .ok_or_else(|| {
                invocation_error(format!(
                    "relative skill directory '{}' requires {RUNX_CWD_ENV}",
                    skill_directory.display()
                ))
            })?
    };
    resolve_existing_path(&path, "resolving process skill directory")
}

pub(super) fn execution_workspace_root(
    workspace_cwd: Option<&Path>,
    skill_directory: &Path,
) -> PathBuf {
    normalize_path(workspace_cwd.unwrap_or(skill_directory))
}

pub(super) fn resolve_path(base: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        base.join(candidate)
    }
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    crate::path_util::lexical_normalize(path)
}

fn resolve_existing_path(path: &Path, operation: &'static str) -> Result<PathBuf, RuntimeError> {
    if path.exists() {
        return fs::canonicalize(path).map_err(|source| RuntimeError::io(operation, source));
    }
    let normalized = normalize_path(path);
    let mut ancestor = normalized.as_path();
    let mut missing_tail = Vec::new();

    loop {
        if ancestor.exists() {
            let mut resolved =
                fs::canonicalize(ancestor).map_err(|source| RuntimeError::io(operation, source))?;
            for component in missing_tail.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }

        let Some(file_name) = ancestor.file_name() else {
            return Ok(normalized);
        };
        missing_tail.push(PathBuf::from(file_name));

        let Some(parent) = ancestor
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return Ok(normalized);
        };
        ancestor = parent;
    }
}

pub(super) fn invocation_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidProcessInvocation {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{normalize_path, resolve_cwd};

    #[test]
    fn normalize_path_preserves_current_directory() {
        assert_eq!(normalize_path(Path::new(".")), PathBuf::from("."));
        assert_eq!(normalize_path(Path::new("skill/..")), PathBuf::from("."));
    }

    #[test]
    fn explicit_runtime_workspace_cwd_preserves_package_relative_default()
    -> Result<(), Box<dyn std::error::Error>> {
        let skill = Path::new("/package/skill");
        let workspace = Path::new("/operator/workspace");

        assert_eq!(resolve_cwd(None, skill, Some(workspace))?, skill);
        assert_eq!(
            resolve_cwd(Some("tools"), skill, Some(workspace))?,
            skill.join("tools")
        );
        assert_eq!(
            resolve_cwd(Some("{{env.RUNX_CWD}}"), skill, Some(workspace))?,
            workspace
        );
        assert!(resolve_cwd(Some("{{env.RUNX_CWD}}"), skill, None).is_err());
        Ok(())
    }
}
