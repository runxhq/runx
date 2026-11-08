use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{Digest, JsonValue};

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
                expected: Default::default(),
            },
            runx_contracts::Reach::Inputs,
            None,
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

/// What selects the registry a candidate's closure is read from, and the key
/// it is trusted to sign with: the registry's URL, the hosted API base that is
/// the default registry when no URL is named, leave to reach a private network
/// address, and the operator's trust key. Reading a public registry takes no
/// credential, so none is forwarded.
const REGISTRY_SELECTION_ENV: [&str; 6] = [
    "RUNX_REGISTRY_URL",
    crate::HOSTED_API_BASE_URL_ENV,
    crate::hosted_api::HOSTED_API_ALLOW_PRIVATE_NETWORK_ENV,
    crate::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV,
    crate::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV,
    crate::registry::RUNX_REGISTRY_MANIFEST_TRUST_PUBLISHER_ENV,
];

/// The environment a candidate's harness runs in: the repo as its workspace,
/// a home and project directory of its own under `scratch`, and the registry
/// its operator selects.
pub(super) fn isolated_test_env(
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
    ]
    .into_iter()
    .chain(REGISTRY_SELECTION_ENV)
    {
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
    isolated
}

pub(super) struct CandidateStage {
    root: PathBuf,
    pub(super) skill_dir: PathBuf,
    pub(super) base_digest: Digest,
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
        apply_text_bundle(
            "runx.skill.apply",
            &stage.skill_dir,
            &staged,
            runx_contracts::Reach::Inputs,
            None,
        )?;
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
    Ok(TextBundle {
        writes,
        deletes,
        expected: Default::default(),
    })
}

impl Drop for CandidateStage {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::isolated_test_env;
    use std::collections::BTreeMap;

    /// A candidate's harness reads the registry its operator selects, and
    /// nothing else of the operator's crosses into it: no credential, home,
    /// project or other setting.
    #[test]
    fn the_isolated_test_carries_the_registry_selection_and_nothing_else()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let selection = [
            ("RUNX_REGISTRY_URL", "http://127.0.0.1:4100"),
            ("RUNX_PUBLIC_API_BASE_URL", "https://api.staging.runx.test"),
            ("RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK", "1"),
            ("RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64", "a2V5"),
            ("RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID", "operator-key"),
            ("RUNX_REGISTRY_MANIFEST_TRUST_PUBLISHER", "acme"),
        ];
        let operator = selection
            .iter()
            .chain(&[
                ("PATH", "/usr/bin"),
                ("HOME", "/home/operator"),
                ("RUNX_HOME", "/home/operator/.runx"),
                ("RUNX_PROJECT_DIR", "/work/.runx"),
                ("RUNX_PUBLIC_API_TOKEN", "rxk_operator"),
                ("RUNX_HOSTED_API_ADMIN_TOKEN", "admin-secret"),
                ("RUNX_AGENT_API_KEY", "agent-secret"),
                ("RUNX_INSTALLATION_ID", "operator-installation"),
            ])
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>();
        let scratch = workspace.path().join("scratch");

        let isolated = isolated_test_env(workspace.path(), &operator, &scratch);

        let mut expected = selection
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>();
        expected.insert("PATH".to_owned(), "/usr/bin".to_owned());
        expected.insert(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().to_string_lossy().into_owned(),
        );
        crate::paths::isolate(&mut expected, &scratch);
        assert_eq!(isolated, expected);
        Ok(())
    }
}
