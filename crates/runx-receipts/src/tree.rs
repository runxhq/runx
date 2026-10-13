mod findings;
mod proof;
mod resolver;
mod traversal;

#[cfg(test)]
mod proof_tests;
#[cfg(test)]
mod structural_tests;
#[cfg(test)]
mod test_support;

use std::collections::BTreeSet;

use runx_contracts::{Receipt, Reference};

use crate::{
    ReceiptFinding, ReceiptProofContext, ReceiptVerification, verify_receipt, verify_receipt_proof,
};
use findings::{child_receipt_findings, duplicate_child_findings, orphan_child_findings};
use proof::{StrictChildProofPolicy, StructuralChildProofPolicy, child_receipt_proof_findings};
use resolver::SliceReceiptResolver;
use traversal::TreeTraversal;

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

pub fn validate_receipt_tree(
    root: &Receipt,
    children: &[Receipt],
) -> Result<(), ReceiptVerification> {
    let resolver = SliceReceiptResolver { children };
    validate_receipt_tree_with_resolver(root, &resolver, ReceiptTreeConfig::default())
}

#[must_use]
pub fn verify_receipt_tree(root: &Receipt, children: &[Receipt]) -> ReceiptVerification {
    let resolver = SliceReceiptResolver { children };
    verify_receipt_tree_with_resolver(root, &resolver, ReceiptTreeConfig::default())
}

pub fn validate_receipt_tree_proof(
    root: &Receipt,
    children: &[Receipt],
    proof_contexts: &impl ReceiptProofContextProvider,
) -> Result<(), ReceiptVerification> {
    let resolver = SliceReceiptResolver { children };
    validate_receipt_tree_proof_with_resolver(
        root,
        &resolver,
        ReceiptTreeConfig::default(),
        proof_contexts,
    )
}

#[must_use]
pub fn verify_receipt_tree_proof(
    root: &Receipt,
    children: &[Receipt],
    proof_contexts: &impl ReceiptProofContextProvider,
) -> ReceiptVerification {
    let resolver = SliceReceiptResolver { children };
    verify_receipt_tree_proof_with_resolver(
        root,
        &resolver,
        ReceiptTreeConfig::default(),
        proof_contexts,
    )
}

pub fn validate_receipt_tree_with_resolver(
    root: &Receipt,
    resolver: &impl ReceiptResolver,
    config: ReceiptTreeConfig,
) -> Result<(), ReceiptVerification> {
    let verification = verify_receipt_tree_with_resolver(root, resolver, config);
    if verification.valid {
        Ok(())
    } else {
        Err(verification)
    }
}

#[must_use]
pub fn verify_receipt_tree_with_resolver(
    root: &Receipt,
    resolver: &impl ReceiptResolver,
    config: ReceiptTreeConfig,
) -> ReceiptVerification {
    let mut findings = verify_receipt(root).findings;
    let supplied = resolver.supplied_receipts();
    findings.extend(duplicate_child_findings(&supplied));
    findings.extend(child_receipt_findings(&supplied));
    verify_tree_relationships(root, resolver, config, &supplied, findings)
}

pub fn validate_receipt_tree_proof_with_resolver(
    root: &Receipt,
    resolver: &impl ReceiptResolver,
    config: ReceiptTreeConfig,
    proof_contexts: &impl ReceiptProofContextProvider,
) -> Result<(), ReceiptVerification> {
    let verification =
        verify_receipt_tree_proof_with_resolver(root, resolver, config, proof_contexts);
    if verification.valid {
        Ok(())
    } else {
        Err(verification)
    }
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
    verify_tree_relationships_with_proof(
        root,
        resolver,
        config,
        &supplied,
        findings,
        proof_contexts,
    )
}

fn verify_tree_relationships<R: ReceiptResolver>(
    root: &Receipt,
    resolver: &R,
    config: ReceiptTreeConfig,
    supplied: &[ResolvedReceipt<'_>],
    mut findings: Vec<ReceiptFinding>,
) -> ReceiptVerification {
    let mut traversal = TreeTraversal {
        resolver,
        config,
        proof_policy: StructuralChildProofPolicy,
        visiting: BTreeSet::new(),
        reached: BTreeSet::new(),
    };
    findings.extend(traversal.subtree_findings("", root, 0));
    findings.extend(orphan_child_findings(supplied, &traversal.reached));
    ReceiptVerification::from_findings(findings)
}

fn verify_tree_relationships_with_proof<R: ReceiptResolver>(
    root: &Receipt,
    resolver: &R,
    config: ReceiptTreeConfig,
    supplied: &[ResolvedReceipt<'_>],
    mut findings: Vec<ReceiptFinding>,
    proof_contexts: &impl ReceiptProofContextProvider,
) -> ReceiptVerification {
    let mut traversal = TreeTraversal {
        resolver,
        config,
        proof_policy: StrictChildProofPolicy::new(supplied, proof_contexts),
        visiting: BTreeSet::new(),
        reached: BTreeSet::new(),
    };
    findings.extend(traversal.subtree_findings("", root, 0));
    findings.extend(orphan_child_findings(supplied, &traversal.reached));
    ReceiptVerification::from_findings(findings)
}
