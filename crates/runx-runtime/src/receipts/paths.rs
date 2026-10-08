use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::path_util::lexical_normalize;

pub const RUNTIME_RECEIPTS_DIR_CONFIG_KEY: &str = "runtime.receipts.dir";
pub const RUNX_RECEIPT_DIR_ENV: &str = "RUNX_RECEIPT_DIR";
pub const RUNX_PROJECT_DIR_ENV: &str = "RUNX_PROJECT_DIR";
pub const RUNX_CWD_ENV: &str = "RUNX_CWD";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeReceiptConfig {
    pub dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptPathSource {
    ExplicitInput,
    RuntimeConfig,
    Environment,
    ProjectDefault,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptStoreLabel(String);

impl ReceiptStoreLabel {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReceiptStoreLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptStorePublicProjection {
    label: ReceiptStoreLabel,
}

impl ReceiptStorePublicProjection {
    #[must_use]
    pub fn label(&self) -> &ReceiptStoreLabel {
        &self.label
    }

    #[must_use]
    pub fn summary(&self) -> String {
        format!("receipt store: {}", self.label)
    }
}

impl fmt::Display for ReceiptStorePublicProjection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "receipt store: {}", self.label)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedReceiptPath {
    pub path: PathBuf,
    pub source: ReceiptPathSource,
    pub label: ReceiptStoreLabel,
    pub project_runx_dir: PathBuf,
    pub workspace_base: PathBuf,
}

impl ResolvedReceiptPath {
    #[must_use]
    pub fn public_projection(&self) -> ReceiptStorePublicProjection {
        ReceiptStorePublicProjection {
            label: self.label.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReceiptPathInputs<'a> {
    pub explicit_dir: Option<&'a Path>,
    pub runtime_config: Option<&'a RuntimeReceiptConfig>,
    pub env: &'a BTreeMap<String, String>,
    pub cwd: &'a Path,
}

#[must_use]
pub fn resolve_receipt_path(inputs: ReceiptPathInputs<'_>) -> ResolvedReceiptPath {
    let workspace_base = resolve_workspace_base(inputs.env, inputs.cwd);
    let project_runx_dir = resolve_project_runx_dir(inputs.env, &workspace_base);
    let (path, source) = match inputs.explicit_dir {
        Some(path) => (
            resolve_from_workspace_base(path, &workspace_base),
            ReceiptPathSource::ExplicitInput,
        ),
        None => match inputs
            .runtime_config
            .and_then(|config| config.dir.as_deref())
        {
            Some(path) => (
                resolve_from_workspace_base(path, &workspace_base),
                ReceiptPathSource::RuntimeConfig,
            ),
            None => match env_path(inputs.env, RUNX_RECEIPT_DIR_ENV) {
                Some(path) => (
                    resolve_from_workspace_base(path, &workspace_base),
                    ReceiptPathSource::Environment,
                ),
                None => (
                    project_runx_dir.join("receipts"),
                    ReceiptPathSource::ProjectDefault,
                ),
            },
        },
    };
    let path = lexical_normalize(&path);
    let label = safe_receipt_store_label(&path, &workspace_base, &project_runx_dir);
    ResolvedReceiptPath {
        path,
        source,
        label,
        project_runx_dir,
        workspace_base,
    }
}

#[must_use]
pub fn resolve_workspace_base(env: &BTreeMap<String, String>, cwd: &Path) -> PathBuf {
    crate::config::resolve_runx_workspace_base(env, cwd)
}

#[must_use]
pub fn resolve_project_runx_dir(env: &BTreeMap<String, String>, workspace_base: &Path) -> PathBuf {
    env_path(env, RUNX_PROJECT_DIR_ENV).map_or_else(
        || lexical_normalize(&workspace_base.join(".runx")),
        |path| resolve_from_workspace_base(path, workspace_base),
    )
}

#[must_use]
pub fn safe_receipt_store_label(
    receipt_dir: &Path,
    workspace_base: &Path,
    project_runx_dir: &Path,
) -> ReceiptStoreLabel {
    let receipt_dir = lexical_normalize(receipt_dir);
    let workspace_base = lexical_normalize(workspace_base);
    let project_runx_dir = lexical_normalize(project_runx_dir);

    if let Ok(relative_to_project) = receipt_dir.strip_prefix(&project_runx_dir) {
        if let Ok(relative_to_workspace) = receipt_dir.strip_prefix(&workspace_base) {
            return ReceiptStoreLabel(path_label(relative_to_workspace));
        }
        return ReceiptStoreLabel(format!("runx-project:{}", path_label(relative_to_project)));
    }

    ReceiptStoreLabel(format!(
        "external-receipt-store:{}",
        stable_path_hash(&receipt_dir)
    ))
}

#[must_use]
pub fn safe_receipt_store_projection(
    receipt_dir: &Path,
    workspace_base: &Path,
    project_runx_dir: &Path,
) -> ReceiptStorePublicProjection {
    ReceiptStorePublicProjection {
        label: safe_receipt_store_label(receipt_dir, workspace_base, project_runx_dir),
    }
}

fn env_path<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a Path> {
    env.get(key)
        .filter(|value| !value.trim().is_empty())
        .map(Path::new)
}

fn resolve_from_workspace_base(path: &Path, workspace_base: &Path) -> PathBuf {
    if path.is_absolute() {
        lexical_normalize(path)
    } else {
        lexical_normalize(&workspace_base.join(path))
    }
}

fn path_label(path: &Path) -> String {
    let label = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(segment) => Some(segment.to_string_lossy().into_owned()),
            Component::CurDir => Some(".".to_owned()),
            Component::ParentDir => Some("..".to_owned()),
            Component::Prefix(_) | Component::RootDir => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if label.is_empty() {
        ".".to_owned()
    } else {
        label
    }
}

fn stable_path_hash(path: &Path) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
