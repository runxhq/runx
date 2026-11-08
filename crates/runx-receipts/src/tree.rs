mod findings;
mod groups;
mod proof;
mod traversal;

#[cfg(test)]
mod proof_tests;
#[cfg(test)]
mod structural_tests;
#[cfg(test)]
mod test_support;

use std::collections::BTreeSet;

use runx_contracts::{Receipt, Reference};

use crate::{ReceiptProofContext, ReceiptVerification, verify_receipt_proof};
use findings::{duplicate_child_findings, orphan_child_findings};
use proof::{ChildProofPolicy, child_receipt_proof_findings};
use traversal::TreeTraversal;

pub use groups::{ReceiptTreeGroup, group_receipt_trees};

/// How many levels a receipt tree may hold below its root.
pub const MAX_RECEIPT_TREE_DEPTH: usize = 64;
/// How many receipts a receipt tree may hold, its root included.
pub const MAX_RECEIPT_TREE_RECEIPTS: usize = 1024;

/// The one rule every verifier of a runx receipt tree applies: a root and
/// its descendants, each committed once by its parent's signed lineage, at
/// most [`MAX_RECEIPT_TREE_RECEIPTS`] receipts and [`MAX_RECEIPT_TREE_DEPTH`]
/// levels below the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiptTreeConfig {
    pub max_depth: usize,
    pub max_receipts: usize,
}

impl Default for ReceiptTreeConfig {
    fn default() -> Self {
        Self {
            max_depth: MAX_RECEIPT_TREE_DEPTH,
            max_receipts: MAX_RECEIPT_TREE_RECEIPTS,
        }
    }
}

/// Every signed receipt-to-receipt edge that a storage adapter must resolve
/// before invoking the shared tree verifier: the receipt's children.
pub fn receipt_edge_references(receipt: &Receipt) -> impl Iterator<Item = &Reference> {
    receipt.lineage.children.iter()
}

pub trait ReceiptResolver {
    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<'a>;
    fn supplied_receipts<'a>(&'a self) -> Vec<ResolvedReceipt<'a>>;
}

pub trait ReceiptProofContextProvider {
    fn proof_context<'a>(&'a self, receipt: &Receipt) -> ReceiptProofContext<'a>;
}

#[derive(Clone, Debug)]
pub struct ResolvedReceipt<'a> {
    pub path: String,
    pub receipt: &'a Receipt,
}

#[derive(Clone, Debug)]
pub enum ReceiptResolveResult<'a> {
    Found(ResolvedReceipt<'a>),
    Missing,
    Malformed,
    Ambiguous,
    ResolverError,
}

#[must_use]
pub fn verify_receipt_tree_proof_with_resolver(
    root: &Receipt,
    resolver: &impl ReceiptResolver,
    config: ReceiptTreeConfig,
    proof_contexts: &impl ReceiptProofContextProvider,
) -> ReceiptVerification {
    let root_context = proof_contexts.proof_context(root);
    let mut findings = verify_receipt_proof(root, &root_context).findings;
    let supplied = resolver.supplied_receipts();
    findings.extend(duplicate_child_findings(&supplied));
    findings.extend(child_receipt_proof_findings(&supplied, proof_contexts));
    let mut traversal = TreeTraversal {
        resolver,
        config,
        proof_policy: ChildProofPolicy::new(&supplied, proof_contexts),
        visiting: BTreeSet::new(),
        reached: BTreeSet::new(),
    };
    findings.extend(traversal.subtree_findings("", root, 0));
    findings.extend(orphan_child_findings(&supplied, &traversal.reached));
    ReceiptVerification::from_findings(findings)
}
