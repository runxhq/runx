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

use runx_contracts::{MAX_RECEIPT_TREE_DEPTH, MAX_RECEIPT_TREE_RECEIPTS, Receipt, Reference};

use crate::{ReceiptProofContext, ReceiptVerification, verify_receipt_proof};
use findings::{duplicate_child_findings, orphan_child_findings};
use proof::{ChildProofPolicy, child_receipt_proof_findings};
use traversal::TreeTraversal;

pub use groups::{ReceiptLinks, ReceiptTreeGroup, group_receipt_trees};

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

/// Where a verifier finds the children a tree's lineage names. A child is
/// needed only while its subtree is walked, so a resolver may lend a
/// receipt it holds or one it reads for that walk alone: the walk then
/// holds the path from the root to the receipt it is at, never the tree.
pub trait ReceiptResolver {
    /// A resolved child, kept for the walk of its subtree and dropped after.
    type Child<'a>: std::ops::Deref<Target = Receipt>
    where
        Self: 'a;

    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<Self::Child<'a>>;
    /// The receipts handed in beside the root, each verified once up front;
    /// a resolver that reads children as they are named supplies none.
    fn supplied_receipts(&self) -> Vec<ResolvedReceipt<&Receipt>>;
}

pub trait ReceiptProofContextProvider {
    fn proof_context<'a>(&'a self, receipt: &Receipt) -> ReceiptProofContext<'a>;
}

#[derive(Clone, Debug)]
pub struct ResolvedReceipt<R> {
    pub path: String,
    pub receipt: R,
}

#[derive(Clone, Debug)]
pub enum ReceiptResolveResult<R> {
    Found(ResolvedReceipt<R>),
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
