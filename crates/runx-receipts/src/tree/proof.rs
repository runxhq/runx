use std::collections::BTreeSet;

use runx_contracts::{Receipt, Reference};

use super::{ReceiptProofContextProvider, ResolvedReceipt};
use crate::tree::findings::{child_digest_link_findings, child_finding};
use crate::{ReceiptFinding, verify_receipt_proof};

pub(super) fn child_receipt_proof_findings(
    children: &[ResolvedReceipt<&Receipt>],
    proof_contexts: &impl ReceiptProofContextProvider,
) -> Vec<ReceiptFinding> {
    children
        .iter()
        .flat_map(|child| {
            let context = proof_contexts.proof_context(child.receipt);
            verify_receipt_proof(child.receipt, &context)
                .findings
                .into_iter()
                .map(|finding| child_finding(&child.path, finding))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What a child edge must prove: the parent's ref carries the child's exact
/// digest, and the child its own proof. A supplied receipt was verified up
/// front, so it is not verified again; a receipt the resolver lends for one
/// walk is verified each time it is lent, since a later one may sit where it
/// sat.
pub(super) struct ChildProofPolicy<'a, P: ReceiptProofContextProvider> {
    proof_contexts: &'a P,
    supplied_receipts: BTreeSet<usize>,
}

impl<'a, P: ReceiptProofContextProvider> ChildProofPolicy<'a, P> {
    pub(super) fn new(supplied: &[ResolvedReceipt<&Receipt>], proof_contexts: &'a P) -> Self {
        Self {
            proof_contexts,
            supplied_receipts: supplied
                .iter()
                .map(|child| receipt_address(child.receipt))
                .collect(),
        }
    }

    pub(super) fn findings(
        &mut self,
        path: &str,
        reference: &Reference,
        receipt: &Receipt,
    ) -> Vec<ReceiptFinding> {
        let mut findings = child_digest_link_findings(path, reference, receipt);
        if self.supplied_receipts.contains(&receipt_address(receipt)) {
            return findings;
        }
        let context = self.proof_contexts.proof_context(receipt);
        findings.extend(
            verify_receipt_proof(receipt, &context)
                .findings
                .into_iter()
                .map(|finding| child_finding(path, finding)),
        );
        findings
    }
}

fn receipt_address(receipt: &Receipt) -> usize {
    receipt as *const Receipt as usize
}
