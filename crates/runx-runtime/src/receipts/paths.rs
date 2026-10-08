use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::path_util::lexical_normalize;
use crate::paths::RunxPaths;

pub const RUNTIME_RECEIPTS_DIR_CONFIG_KEY: &str = "runtime.receipts.dir";

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
    pub paths: &'a RunxPaths,
}

/// The receipt root: an explicit directory, else the runtime config, else
/// `RUNX_RECEIPT_DIR`, else the project's `receipts`.
#[must_use]
pub fn resolve_receipt_path(inputs: ReceiptPathInputs<'_>) -> ResolvedReceiptPath {
    let paths = inputs.paths;
    let configured = inputs
        .runtime_config
        .and_then(|config| config.dir.as_deref());
    let (path, source) = match (inputs.explicit_dir, configured, paths.receipts_override()) {
        (Some(path), _, _) => (paths.in_workspace(path), ReceiptPathSource::ExplicitInput),
        (None, Some(path), _) => (paths.in_workspace(path), ReceiptPathSource::RuntimeConfig),
        (None, None, Some(path)) => (path.to_path_buf(), ReceiptPathSource::Environment),
        (None, None, None) => (paths.receipts(), ReceiptPathSource::ProjectDefault),
    };
    let label = safe_receipt_store_label(&path, paths);
    ResolvedReceiptPath {
        path,
        source,
        label,
    }
}

#[must_use]
pub fn safe_receipt_store_label(receipt_dir: &Path, paths: &RunxPaths) -> ReceiptStoreLabel {
    let receipt_dir = lexical_normalize(receipt_dir);
    if let Ok(relative_to_project) = receipt_dir.strip_prefix(paths.project()) {
        if let Ok(relative_to_workspace) = receipt_dir.strip_prefix(paths.workspace()) {
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
    paths: &RunxPaths,
) -> ReceiptStorePublicProjection {
    ReceiptStorePublicProjection {
        label: safe_receipt_store_label(receipt_dir, paths),
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
