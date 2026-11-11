use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::Receipt;

use crate::RuntimeError;
use crate::paths::RunxPaths;
use crate::receipts::paths::{
    ReceiptPathInputs, ResolvedReceiptPath, RuntimeReceiptConfig, resolve_receipt_path,
};
use crate::receipts::store::{LocalReceiptStore, ReceiptStoreError};
use crate::receipts::{
    Ed25519ReceiptVerifier, ReceiptTrust, RuntimeReceiptSignatureConfig,
    RuntimeReceiptSignaturePolicy, RuntimeReceiptSigningError, receipt_verifier,
};
use crate::services::WorkspaceEnv;

#[derive(Clone, Debug)]
pub(crate) struct ReceiptServices {
    signature_config: RuntimeReceiptSignatureConfig,
}

/// A receipt store and the keys its receipts are checked against: the
/// selected home's own key and every configured key.
pub(crate) struct ReceiptReadContext {
    store: LocalReceiptStore,
    paths: RunxPaths,
    resolved: ResolvedReceiptPath,
    verifier: Ed25519ReceiptVerifier,
}

impl ReceiptReadContext {
    pub(crate) fn resolve(
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, RuntimeError> {
        let paths = RunxPaths::resolve(env, cwd);
        let verifier = receipt_verifier(env, &paths, ReceiptTrust::HomeAndConfigured)
            .map_err(|error| receipt_store_error(error.to_string()))?;
        let resolved = resolve_receipt_path(ReceiptPathInputs {
            explicit_dir: None,
            runtime_config: Some(&RuntimeReceiptConfig::default()),
            paths: &paths,
        });
        let store = LocalReceiptStore::of_project(&resolved.path, &paths);
        Ok(Self {
            store,
            paths,
            resolved,
            verifier,
        })
    }

    pub(crate) const fn paths(&self) -> &RunxPaths {
        &self.paths
    }

    pub(crate) const fn store(&self) -> &LocalReceiptStore {
        &self.store
    }

    pub(crate) const fn resolved(&self) -> &ResolvedReceiptPath {
        &self.resolved
    }

    pub(crate) fn signature_policy(&self) -> RuntimeReceiptSignaturePolicy<'_> {
        RuntimeReceiptSignaturePolicy::verifying(&self.verifier)
    }
}

impl ReceiptServices {
    /// The identity a run in `workspace` seals under: a configured signer,
    /// else the selected home's own key.
    pub(crate) fn resolve(workspace: &WorkspaceEnv) -> Result<Self, RuntimeReceiptSigningError> {
        Ok(Self {
            signature_config: RuntimeReceiptSignatureConfig::resolve(
                workspace.env(),
                workspace.paths(),
            )?,
        })
    }

    pub(crate) fn signature_config(&self) -> &RuntimeReceiptSignatureConfig {
        &self.signature_config
    }

    pub(crate) fn from_signature_config(signature_config: RuntimeReceiptSignatureConfig) -> Self {
        Self { signature_config }
    }

    pub(crate) fn write_local_receipt(
        &self,
        receipt: &Receipt,
        path: &ResolvedReceiptPath,
    ) -> Result<(), ReceiptStoreError> {
        LocalReceiptStore::new(&path.path)
            .write_receipt(receipt, self.signature_config.signature_policy())
    }

    /// Write the receipts a run sealed, a copy of `record`'s, into the
    /// receipt directory at `path`.
    pub(crate) fn write_sealed_receipts<'a>(
        &self,
        receipts: impl IntoIterator<Item = &'a Receipt>,
        path: &ResolvedReceiptPath,
        record: &std::sync::Arc<crate::kernel::store::KernelStore>,
    ) -> Result<(), ReceiptStoreError> {
        LocalReceiptStore::new(&path.path)
            .with_record(std::sync::Arc::clone(record))
            .write_sealed(receipts, self.signature_config.signature_policy())
    }

    pub(crate) fn write_local_receipts<'a>(
        &self,
        receipts: impl IntoIterator<Item = &'a Receipt>,
        receipt_dir: &Path,
    ) -> Result<(), ReceiptStoreError> {
        LocalReceiptStore::new(receipt_dir)
            .write_receipts(receipts, self.signature_config.signature_policy())
    }
}

fn receipt_store_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "receipt store".to_owned(),
        message: message.into(),
    }
}
