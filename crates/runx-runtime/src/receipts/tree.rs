use std::collections::BTreeMap;

use runx_contracts::{Receipt, Reference, ReferenceType};
use runx_receipts::{
    ReceiptResolveResult, ReceiptResolver, ReceiptTreeConfig, ReceiptVerification, ResolvedReceipt,
    verify_receipt_tree_proof_with_resolver,
};

use super::seal::{RuntimeReceiptProofContextProvider, RuntimeReceiptSignaturePolicy};

#[derive(Clone, Debug, Default)]
pub struct RuntimeReceiptResolver {
    receipts: Vec<Receipt>,
    positions: BTreeMap<String, Vec<usize>>,
}

impl RuntimeReceiptResolver {
    #[must_use]
    pub fn new(receipts: impl IntoIterator<Item = Receipt>) -> Self {
        let receipts = deduplicate_identical_receipts(
            receipts,
            |receipt| receipt.id.as_str(),
            |left, right| left == right,
        );
        let positions = receipt_positions(receipts.iter());
        Self {
            receipts,
            positions,
        }
    }

    #[must_use]
    pub fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }
}

impl ReceiptResolver for RuntimeReceiptResolver {
    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<'a> {
        let Some(receipt_id) = referenced_receipt_id(reference) else {
            return ReceiptResolveResult::Malformed;
        };
        let Some(indexes) = self.positions.get(receipt_id) else {
            return ReceiptResolveResult::Missing;
        };
        let [index] = indexes.as_slice() else {
            return ReceiptResolveResult::Ambiguous;
        };
        let Some(receipt) = self.receipts.get(*index) else {
            return ReceiptResolveResult::ResolverError;
        };
        ReceiptResolveResult::Found(ResolvedReceipt {
            path: runtime_receipt_path(*index),
            receipt,
        })
    }

    fn supplied_receipts<'a>(&'a self) -> Vec<ResolvedReceipt<'a>> {
        self.receipts
            .iter()
            .enumerate()
            .map(|(index, receipt)| ResolvedReceipt {
                path: runtime_receipt_path(index),
                receipt,
            })
            .collect()
    }
}

struct RuntimeReceiptRefResolver<'a> {
    receipts: Vec<&'a Receipt>,
    positions: BTreeMap<String, Vec<usize>>,
}

impl<'a> RuntimeReceiptRefResolver<'a> {
    fn new(receipts: impl IntoIterator<Item = &'a Receipt>) -> Self {
        let receipts = deduplicate_identical_receipts(
            receipts,
            |receipt| receipt.id.as_str(),
            |left, right| left == right,
        );
        let positions = receipt_positions(receipts.iter().copied());
        Self {
            receipts,
            positions,
        }
    }
}

impl ReceiptResolver for RuntimeReceiptRefResolver<'_> {
    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<'a> {
        let Some(receipt_id) = referenced_receipt_id(reference) else {
            return ReceiptResolveResult::Malformed;
        };
        let Some(indexes) = self.positions.get(receipt_id) else {
            return ReceiptResolveResult::Missing;
        };
        let [index] = indexes.as_slice() else {
            return ReceiptResolveResult::Ambiguous;
        };
        let Some(receipt) = self.receipts.get(*index) else {
            return ReceiptResolveResult::ResolverError;
        };
        ReceiptResolveResult::Found(ResolvedReceipt {
            path: runtime_receipt_path(*index),
            receipt,
        })
    }

    fn supplied_receipts<'a>(&'a self) -> Vec<ResolvedReceipt<'a>> {
        self.receipts
            .iter()
            .enumerate()
            .map(|(index, receipt)| ResolvedReceipt {
                path: runtime_receipt_path(index),
                receipt,
            })
            .collect()
    }
}

pub fn validate_runtime_receipt_tree(
    root: &Receipt,
    receipts: impl IntoIterator<Item = Receipt>,
    config: ReceiptTreeConfig,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), ReceiptVerification> {
    let verification = verify_runtime_receipt_tree(root, receipts, config, signature_policy);
    if verification.valid {
        Ok(())
    } else {
        Err(verification)
    }
}

pub(crate) fn validate_runtime_receipt_tree_refs<'a>(
    root: &Receipt,
    receipts: impl IntoIterator<Item = &'a Receipt>,
    config: ReceiptTreeConfig,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<(), ReceiptVerification> {
    let verification = verify_runtime_receipt_tree_refs(root, receipts, config, signature_policy);
    if verification.valid {
        Ok(())
    } else {
        Err(verification)
    }
}

#[must_use]
pub fn verify_runtime_receipt_tree(
    root: &Receipt,
    receipts: impl IntoIterator<Item = Receipt>,
    config: ReceiptTreeConfig,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> ReceiptVerification {
    let resolver = RuntimeReceiptResolver::new(receipts);
    let proof_contexts = RuntimeReceiptProofContextProvider::new(signature_policy);
    verify_receipt_tree_proof_with_resolver(root, &resolver, config, &proof_contexts)
}

fn verify_runtime_receipt_tree_refs<'a>(
    root: &Receipt,
    receipts: impl IntoIterator<Item = &'a Receipt>,
    config: ReceiptTreeConfig,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> ReceiptVerification {
    let resolver = RuntimeReceiptRefResolver::new(receipts);
    let proof_contexts = RuntimeReceiptProofContextProvider::new(signature_policy);
    verify_receipt_tree_proof_with_resolver(root, &resolver, config, &proof_contexts)
}

fn runtime_receipt_path(index: usize) -> String {
    format!("runtime_receipts[{index}]")
}

fn deduplicate_identical_receipts<T>(
    receipts: impl IntoIterator<Item = T>,
    receipt_id: impl for<'a> Fn(&'a T) -> &'a str,
    identical: impl Fn(&T, &T) -> bool,
) -> Vec<T> {
    let mut unique = Vec::<T>::new();
    let mut positions = BTreeMap::<String, Vec<usize>>::new();
    for receipt in receipts {
        let id = receipt_id(&receipt).to_owned();
        let duplicate = positions.get(&id).is_some_and(|indexes| {
            indexes
                .iter()
                .any(|index| identical(&unique[*index], &receipt))
        });
        if duplicate {
            continue;
        }
        let index = unique.len();
        unique.push(receipt);
        positions.entry(id).or_default().push(index);
    }
    unique
}

fn receipt_positions<'a>(
    receipts: impl Iterator<Item = &'a Receipt>,
) -> BTreeMap<String, Vec<usize>> {
    let mut positions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, receipt) in receipts.enumerate() {
        positions
            .entry(receipt.id.to_string())
            .or_default()
            .push(index);
    }
    positions
}

fn referenced_receipt_id(reference: &Reference) -> Option<&str> {
    if reference.reference_type != ReferenceType::Receipt {
        return None;
    }
    reference
        .uri
        .strip_prefix("runx:receipt:")
        .filter(|id| !id.is_empty())
}
