use std::cell::RefCell;

use runx_contracts::{JsonObject, JsonValue, Receipt};

use crate::receipts::RuntimeReceiptSignaturePolicy;
use crate::receipts::store::{LocalReceiptStore, ReceiptStoreError};

pub(crate) struct LoadedReceiptTree {
    pub(crate) root: Receipt,
    /// The one receipt of the tree the caller asked to keep, as the walk
    /// read it.
    pub(crate) kept: Option<Receipt>,
    pub(crate) receipt_count: usize,
    pub(crate) verification: runx_receipts::ReceiptVerification,
    pub(crate) findings: Vec<JsonValue>,
}

/// The shared native proof owner: read only this root's signed descendants,
/// each as the walk reaches it, and apply the normal tree and signature
/// policy once. The walk holds the path it is on, never the tree, and keeps
/// the receipt `keep` names when it reads it.
pub(crate) fn load_verified_tree(
    store: &LocalReceiptStore,
    receipt_id: &str,
    keep: Option<&str>,
    policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<LoadedReceiptTree, ReceiptStoreError> {
    let root = store.read_exact(receipt_id, policy)?;
    let findings = RefCell::new(Vec::new());
    let kept = RefCell::new(None);
    let read = crate::receipts::tree::verify_runtime_receipt_tree_reading(
        &root,
        |child_id| match store.read_exact(child_id, policy) {
            Ok(receipt) => {
                if keep == Some(child_id) {
                    *kept.borrow_mut() = Some(receipt.clone());
                }
                Some(receipt)
            }
            Err(error) => {
                finding(
                    &mut findings.borrow_mut(),
                    "receipt.tree.child_unreadable",
                    format!(
                        "child receipt {child_id} could not be verified: {}",
                        public_error(&error)
                    ),
                );
                None
            }
        },
        policy,
    );
    let mut findings = findings.into_inner();
    findings.extend(super::projection::tree_findings(
        &read.verification.findings,
    ));
    Ok(LoadedReceiptTree {
        root,
        kept: kept.into_inner(),
        receipt_count: read.receipt_count,
        verification: read.verification,
        findings,
    })
}

pub(super) fn store_finding(
    findings: &mut Vec<JsonValue>,
    receipt_id: &str,
    error: &ReceiptStoreError,
    label: &crate::ReceiptStoreLabel,
) {
    finding(
        findings,
        "receipt.store.unresolved",
        format!(
            "receipt {receipt_id} could not be resolved: {}",
            error.public_message(label)
        ),
    );
}

fn public_error(error: &ReceiptStoreError) -> &'static str {
    match error {
        ReceiptStoreError::MissingStore { .. } => "receipt store is missing",
        ReceiptStoreError::MissingReceipt { .. } => "receipt is missing",
        ReceiptStoreError::ReceiptProofInvalid { .. } => "receipt proof is invalid",
        ReceiptStoreError::InvalidReceiptId { .. } => "receipt id is invalid",
        _ => "receipt store evidence is invalid or unreadable",
    }
}

fn finding(findings: &mut Vec<JsonValue>, code: &str, message: impl Into<String>) {
    findings.push(JsonValue::Object(JsonObject::from([
        ("code".to_owned(), JsonValue::String(code.to_owned())),
        ("message".to_owned(), JsonValue::String(message.into())),
    ])));
}
