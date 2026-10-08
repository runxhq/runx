use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::Receipt;

use crate::RuntimeError;
use crate::receipts::paths::{
    ReceiptPathInputs, ResolvedReceiptPath, RuntimeReceiptConfig, resolve_receipt_path,
};
use crate::receipts::store::{LocalReceiptStore, ReceiptStoreError};
use crate::receipts::{
    Ed25519ReceiptVerifier, RuntimeReceiptSignatureConfig, RuntimeReceiptSigningError,
    receipt_verifier_from_env,
};
use crate::services::WorkspaceEnv;

#[derive(Clone, Debug)]
pub(crate) struct ReceiptServices {
    signature_config: RuntimeReceiptSignatureConfig,
}

pub(crate) struct ReceiptReadContext {
    store: LocalReceiptStore,
    resolved: ResolvedReceiptPath,
    verifier: Option<Ed25519ReceiptVerifier>,
}

impl ReceiptReadContext {
    pub(crate) fn resolve(
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, RuntimeError> {
        let verifier = production_receipt_verifier(env)?;
        let resolved = resolve_receipt_path(ReceiptPathInputs {
            explicit_dir: None,
            runtime_config: Some(&RuntimeReceiptConfig::default()),
            env,
            cwd,
        });
        let store = LocalReceiptStore::new(&resolved.path);
        Ok(Self {
            store,
            resolved,
            verifier,
        })
    }

    pub(crate) const fn store(&self) -> &LocalReceiptStore {
        &self.store
    }

    pub(crate) const fn resolved(&self) -> &ResolvedReceiptPath {
        &self.resolved
    }

    pub(crate) const fn signature_mode(&self) -> &'static str {
        if self.verifier.is_some() {
            "production"
        } else {
            "local-development"
        }
    }

    pub(crate) fn signature_policy(&self) -> crate::receipts::RuntimeReceiptSignaturePolicy<'_> {
        self.verifier.as_ref().map_or_else(
            crate::receipts::RuntimeReceiptSignaturePolicy::local_development,
            |verifier| crate::receipts::RuntimeReceiptSignaturePolicy::production(verifier),
        )
    }
}

impl ReceiptServices {
    pub(crate) fn from_env(
        env: &BTreeMap<String, String>,
    ) -> Result<Self, RuntimeReceiptSigningError> {
        Ok(Self {
            signature_config: RuntimeReceiptSignatureConfig::from_env(env)?,
        })
    }

    pub(crate) fn from_env_or_local_development(
        env: &BTreeMap<String, String>,
    ) -> Result<Self, RuntimeReceiptSigningError> {
        match RuntimeReceiptSignatureConfig::from_env(env) {
            Ok(signature_config) => Ok(Self { signature_config }),
            Err(RuntimeReceiptSigningError::MissingSigningEnv) => Ok(Self {
                signature_config: RuntimeReceiptSignatureConfig::local_development(),
            }),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn signature_config(&self) -> &RuntimeReceiptSignatureConfig {
        &self.signature_config
    }

    #[cfg(any(feature = "process-tool", test))]
    pub(crate) fn from_signature_config(signature_config: RuntimeReceiptSignatureConfig) -> Self {
        Self { signature_config }
    }

    pub(crate) fn resolve_path(
        &self,
        workspace: &WorkspaceEnv,
        explicit_dir: Option<&Path>,
        runtime_config: Option<&RuntimeReceiptConfig>,
    ) -> ResolvedReceiptPath {
        let _ = self;
        resolve_receipt_path(ReceiptPathInputs {
            explicit_dir,
            runtime_config,
            env: workspace.env(),
            cwd: workspace.cwd(),
        })
    }

    pub(crate) fn write_local_receipt(
        &self,
        receipt: &Receipt,
        path: &ResolvedReceiptPath,
    ) -> Result<(), ReceiptStoreError> {
        LocalReceiptStore::new(&path.path)
            .write_receipt_with_policy(receipt, self.signature_config.signature_policy())
    }

    #[cfg(feature = "process-tool")]
    pub(crate) fn write_local_receipts<'a>(
        &self,
        receipts: impl IntoIterator<Item = &'a Receipt>,
        receipt_dir: &Path,
    ) -> Result<(), ReceiptStoreError> {
        LocalReceiptStore::new(receipt_dir)
            .write_receipts_with_policy(receipts, self.signature_config.signature_policy())
    }

    #[cfg(any(feature = "process-tool", test))]
    pub(crate) fn list_local_receipts(
        &self,
        receipt_dir: &Path,
    ) -> Result<Vec<Receipt>, ReceiptStoreError> {
        LocalReceiptStore::new(receipt_dir)
            .list_with_policy(self.signature_config.signature_policy())
    }

    pub(crate) fn read_local_receipt(
        &self,
        receipt_id: &str,
        receipt_dir: &Path,
    ) -> Result<Receipt, ReceiptStoreError> {
        LocalReceiptStore::new(receipt_dir)
            .read_exact_with_policy(receipt_id, self.signature_config.signature_policy())
    }
}

fn production_receipt_verifier(
    env: &BTreeMap<String, String>,
) -> Result<Option<Ed25519ReceiptVerifier>, RuntimeError> {
    receipt_verifier_from_env(env)
        .map(|resolved| resolved.map(|verifier| verifier.into_verifier()))
        .map_err(|error| receipt_read_error(error.to_string()))
}

fn receipt_read_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: "receipt.read".to_owned(),
        message: message.into(),
    }
}
