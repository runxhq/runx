use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::JsonValue;

use super::path::{
    declared_secret_prefixes, display_relative, invalid_skill_change, normalize_package_file,
    reject_secret_material, required_string, unique_stage_root,
};
use super::snapshot::{PackageSnapshot, package_snapshot};
use super::{MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};
use crate::RuntimeError;
use crate::filesystem::{TextBundle, TextFileWrite, apply_text_bundle};

pub(super) struct InlineCandidateStage {
    root: PathBuf,
    pub(super) skill_dir: PathBuf,
}

impl InlineCandidateStage {
    pub(super) fn prepare(
        repo_root: &Path,
        files: &[JsonValue],
        env: &BTreeMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        let root = unique_stage_root(repo_root, &repo_root.join(".runx-inline-skill"));
        let stage = Self {
            root: root.clone(),
            skill_dir: root,
        };
        fs::create_dir_all(&stage.skill_dir).map_err(|source| {
            RuntimeError::io(
                format!("creating inline candidate {}", stage.skill_dir.display()),
                source,
            )
        })?;
        let writes =
            admit_inline_candidate_files(files, &declared_secret_prefixes(repo_root, env)?)?;
        apply_text_bundle(
            "runx.skill.validate",
            &stage.skill_dir,
            &TextBundle {
                writes,
                deletes: Vec::new(),
            },
        )?;
        Ok(stage)
    }
}

fn admit_inline_candidate_files(
    files: &[JsonValue],
    secret_prefixes: &BTreeSet<String>,
) -> Result<Vec<TextFileWrite>, RuntimeError> {
    if files.is_empty() || files.len() > MAX_PACKAGE_FILES {
        return Err(invalid_skill_change(format!(
            "candidate files must contain 1-{MAX_PACKAGE_FILES} entries"
        )));
    }
    let mut writes = Vec::with_capacity(files.len());
    let mut total_bytes = 0usize;
    let mut seen = BTreeSet::new();
    for (index, file) in files.iter().enumerate() {
        let file = file.as_object().ok_or_else(|| {
            invalid_skill_change(format!("candidate files[{index}] must be an object"))
        })?;
        let path = normalize_package_file(required_string(file, "path")?)?;
        if !seen.insert(path.clone()) {
            return Err(invalid_skill_change(format!(
                "candidate contains duplicate file {path}"
            )));
        }
        let contents = file
            .get("contents")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                invalid_skill_change(format!(
                    "candidate files[{index}].contents must be a string"
                ))
            })?;
        total_bytes = total_bytes
            .checked_add(contents.len())
            .ok_or_else(|| invalid_skill_change("candidate byte count overflow"))?;
        if total_bytes > MAX_PACKAGE_BYTES {
            return Err(invalid_skill_change(format!(
                "candidate exceeds {MAX_PACKAGE_BYTES} bytes"
            )));
        }
        reject_secret_material(&path, contents, secret_prefixes)?;
        writes.push(TextFileWrite {
            path,
            contents: contents.to_owned(),
        });
    }
    if !seen.contains("SKILL.md") || !seen.contains(runx_parser::PROFILE_FILE) {
        return Err(invalid_skill_change(
            "candidate must contain SKILL.md and run.yaml",
        ));
    }
    Ok(writes)
}

impl Drop for InlineCandidateStage {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.root);
    }
}

pub(super) struct ValidationReceiptStage {
    root: PathBuf,
    pub(super) receipt_dir: PathBuf,
}

impl ValidationReceiptStage {
    pub(super) fn prepare(repo_root: &Path) -> Result<Self, RuntimeError> {
        let root = unique_stage_root(repo_root, &repo_root.join(".runx-validation"));
        let receipt_dir = root.join("receipts");
        fs::create_dir_all(&receipt_dir).map_err(|source| {
            RuntimeError::io(
                format!(
                    "creating isolated validation receipts {}",
                    receipt_dir.display()
                ),
                source,
            )
        })?;
        Ok(Self { root, receipt_dir })
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for ValidationReceiptStage {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.root);
    }
}

/// The environment a candidate's harness runs in: the repo as its workspace,
/// and a home and project directory of its own under `scratch`.
pub(super) fn isolated_harness_env(
    repo_root: &Path,
    env: &BTreeMap<String, String>,
    scratch: &Path,
) -> BTreeMap<String, String> {
    let mut isolated = BTreeMap::new();
    for key in [
        "PATH",
        "SystemRoot",
        "COMSPEC",
        "PATHEXT",
        "LANG",
        "LC_ALL",
        "SHELL",
    ] {
        if let Some(value) = env.get(key) {
            isolated.insert(key.to_owned(), value.clone());
        }
    }
    isolated.insert(
        crate::paths::RUNX_CWD_ENV.to_owned(),
        repo_root.to_string_lossy().into_owned(),
    );
    crate::paths::isolate(&mut isolated, scratch);
    let tools = repo_root.join("tools");
    if tools.is_dir()
        && let Ok(value) = std::env::join_paths([tools])
    {
        isolated.insert(
            crate::services::tool_roots::RUNX_TOOL_ROOTS_ENV.to_owned(),
            value.to_string_lossy().into_owned(),
        );
    }
    if let Some(registry_dir) = admitted_registry_dir(repo_root, env) {
        isolated.insert("RUNX_REGISTRY_DIR".to_owned(), registry_dir);
    }
    isolated
}

fn admitted_registry_dir(repo_root: &Path, env: &BTreeMap<String, String>) -> Option<String> {
    let paths = crate::paths::RunxPaths::resolve(env, repo_root);
    let configured = paths.registry_dir()?;
    let repo_root = fs::canonicalize(repo_root).ok()?;
    let registry_dir = fs::canonicalize(configured).ok()?;
    if !registry_dir.is_dir()
        || (registry_dir != repo_root && !registry_dir.starts_with(&repo_root))
    {
        return None;
    }
    Some(registry_dir.to_string_lossy().into_owned())
}

pub(super) struct CandidateStage {
    root: PathBuf,
    pub(super) skill_dir: PathBuf,
    pub(super) base_digest: String,
}

impl CandidateStage {
    pub(super) fn prepare(
        repo_root: &Path,
        target: &Path,
        bundle: &TextBundle,
    ) -> Result<Self, RuntimeError> {
        let root = unique_stage_root(repo_root, target);
        let skill_dir = root.clone();
        let snapshot = package_snapshot(target)?;
        let stage = Self {
            root,
            skill_dir,
            base_digest: snapshot.digest.clone(),
        };
        fs::create_dir_all(&stage.skill_dir).map_err(|source| {
            RuntimeError::io(
                format!("creating candidate {}", stage.skill_dir.display()),
                source,
            )
        })?;
        copy_snapshot(&stage.skill_dir, snapshot)?;
        let staged = rebase_bundle(repo_root, target, bundle)?;
        apply_text_bundle("runx.skill.apply", &stage.skill_dir, &staged)?;
        Ok(stage)
    }
}

fn copy_snapshot(skill_dir: &Path, snapshot: PackageSnapshot) -> Result<(), RuntimeError> {
    for file in snapshot.files {
        let destination = skill_dir.join(&file.relative);
        let parent = destination
            .parent()
            .ok_or_else(|| invalid_skill_change("candidate file has no parent directory"))?;
        fs::create_dir_all(parent).map_err(|source| {
            RuntimeError::io(
                format!("creating candidate directory {}", parent.display()),
                source,
            )
        })?;
        fs::write(&destination, &file.contents).map_err(|source| {
            RuntimeError::io(
                format!("writing candidate file {}", destination.display()),
                source,
            )
        })?;
        fs::set_permissions(&destination, file.permissions).map_err(|source| {
            RuntimeError::io(
                format!("preserving candidate permissions {}", destination.display()),
                source,
            )
        })?;
    }
    Ok(())
}

fn rebase_bundle(
    repo_root: &Path,
    target: &Path,
    bundle: &TextBundle,
) -> Result<TextBundle, RuntimeError> {
    let target_prefix = target
        .strip_prefix(repo_root)
        .map_err(|_| invalid_skill_change("target directory escaped repo root"))?;
    let prefix = format!("{}/", display_relative(target_prefix));
    let writes = bundle
        .writes
        .iter()
        .map(|write| {
            let relative = write.path.strip_prefix(&prefix).ok_or_else(|| {
                invalid_skill_change(format!(
                    "bundle write is outside target directory: {}",
                    write.path
                ))
            })?;
            Ok(TextFileWrite {
                path: relative.to_owned(),
                contents: write.contents.clone(),
            })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    let deletes = bundle
        .deletes
        .iter()
        .map(|delete| {
            delete
                .strip_prefix(&prefix)
                .map(str::to_owned)
                .ok_or_else(|| {
                    invalid_skill_change(format!(
                        "bundle delete is outside target directory: {delete}"
                    ))
                })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    Ok(TextBundle { writes, deletes })
}

impl Drop for CandidateStage {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::isolated_harness_env;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn isolated_harness_admits_only_an_in_workspace_registry()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let registry = workspace.path().join(".runx/authoring-registry");
        std::fs::create_dir_all(&registry)?;
        let env = BTreeMap::from([(
            "RUNX_REGISTRY_DIR".to_owned(),
            registry.to_string_lossy().into_owned(),
        )]);

        let scratch = workspace.path().join("scratch");
        let isolated = isolated_harness_env(workspace.path(), &env, &scratch);
        let paths = crate::paths::RunxPaths::resolve(&isolated, workspace.path());

        assert_eq!(
            isolated.get("RUNX_REGISTRY_DIR").map(String::as_str),
            Some(registry.canonicalize()?.to_string_lossy().as_ref())
        );
        assert_eq!(paths.home(), scratch.join("home"));
        assert_eq!(paths.project(), scratch.join("project"));
        Ok(())
    }

    #[test]
    fn isolated_harness_drops_an_out_of_workspace_registry()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let registry = tempdir()?;
        let env = BTreeMap::from([(
            "RUNX_REGISTRY_DIR".to_owned(),
            registry.path().to_string_lossy().into_owned(),
        )]);

        let isolated = isolated_harness_env(workspace.path(), &env, &workspace.path().join("s"));

        assert!(!isolated.contains_key("RUNX_REGISTRY_DIR"));
        Ok(())
    }
}
