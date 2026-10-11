use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::Receipt;

use super::receipts::ReceiptReadContext;
use crate::{ReceiptStoreError, RuntimeError};

/// Exact, proof-verifying access to the receipt store selected by the current
/// workspace. Consumers receive receipts only after content-address, signature,
/// and schema verification under the same policy as the native receipt tools.
pub struct VerifiedReceiptStore {
    context: ReceiptReadContext,
}

impl VerifiedReceiptStore {
    pub fn resolve(env: &BTreeMap<String, String>, cwd: &Path) -> Result<Self, RuntimeError> {
        Ok(Self {
            context: ReceiptReadContext::resolve(env, cwd)?,
        })
    }

    pub fn read_exact(&self, receipt_id: &str) -> Result<Receipt, ReceiptStoreError> {
        self.context
            .store()
            .read_exact(receipt_id, self.context.signature_policy())
    }

    pub fn list(&self) -> Result<Vec<Receipt>, ReceiptStoreError> {
        self.context.store().list(self.context.signature_policy())
    }

    pub(crate) fn write_all<'a>(
        &self,
        receipts: impl IntoIterator<Item = &'a Receipt>,
    ) -> Result<(), ReceiptStoreError> {
        self.context
            .store()
            .write_receipts(receipts, self.context.signature_policy())
    }
}
