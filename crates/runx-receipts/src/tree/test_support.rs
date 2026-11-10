use std::cell::Cell;
use std::collections::BTreeMap;

use runx_contracts::{Receipt, Reference, ReferenceType};
use serde::Deserialize;

use super::findings::referenced_receipt_id;
use super::{
    ReceiptProofContextProvider, ReceiptResolveResult, ReceiptResolver, ReceiptTreeConfig,
    ResolvedReceipt, verify_receipt_tree_proof_with_resolver,
};
use crate::{
    ReceiptError, ReceiptFindingCode, ReceiptProofContext, ReceiptVerification,
    SignatureVerificationFailure, SignatureVerifier, canonical_receipt_body_digest,
    content_addressed_receipt_id,
};

#[path = "../../tests/fixture_key.rs"]
mod fixture_key;

use fixture_key::{FIXTURE_SEED, FixtureKey};

pub(super) const SUCCESS_RECEIPT: &str =
    include_str!("../../../../fixtures/contracts/run-spine/receipt-success.json");
const ABNORMAL_RECEIPT: &str =
    include_str!("../../../../fixtures/contracts/run-spine/receipt-abnormal.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    expected: Receipt,
}

pub(super) fn child_refs_mut(receipt: &mut Receipt) -> &mut Vec<Reference> {
    &mut receipt.lineage.children
}

pub(super) fn fixture(json: &str) -> Result<Receipt, serde_json::Error> {
    let mut receipt = serde_json::from_str::<Fixture>(json).map(|fixture| fixture.expected)?;
    // The flat success fixture carries no children; the tree tests need one
    // typed child ref to mutate, so seed a single receipt ref.
    if receipt.lineage.children.is_empty() {
        child_refs_mut(&mut receipt).push(Reference::runx(ReferenceType::Receipt, "rcpt_child_1"));
    }
    Ok(receipt)
}

pub(super) fn child(id: &str) -> Result<Receipt, serde_json::Error> {
    let mut receipt = fixture(ABNORMAL_RECEIPT)?;
    receipt.id = id.into();
    child_refs_mut(&mut receipt).clear();
    Ok(receipt)
}

pub(super) fn proof_root() -> Result<Receipt, serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    refresh_proof_digest_and_signature(&mut receipt)?;
    Ok(receipt)
}

/// A sealed child whose outcome, and so whose content address, is its
/// label's: two children with one label are one receipt.
pub(super) fn proof_child(label: &str) -> Result<Receipt, serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    receipt.subject.outcome_digest = runx_contracts::connector::Digest::of(label.as_bytes());
    child_refs_mut(&mut receipt).clear();
    refresh_proof_digest_and_signature(&mut receipt)?;
    Ok(receipt)
}

/// The ref a parent signs for `child`: its id and its digest.
pub(super) fn child_link(child: &Receipt) -> Reference {
    Reference {
        locator: Some(child.digest.clone()),
        ..Reference::runx(ReferenceType::Receipt, &child.id)
    }
}

/// Point the root's child ref at `child` by its id and digest, and reseal.
pub(super) fn link_child_digest(
    root: &mut Receipt,
    index: usize,
    child: &Receipt,
) -> Result<(), serde_json::Error> {
    child_refs_mut(root)[index] = child_link(child);
    refresh_proof_digest_and_signature(root)
}

/// Seal the receipt as it now reads: its content address, digest and
/// signature.
pub(super) fn refresh_proof_digest_and_signature(
    receipt: &mut Receipt,
) -> Result<(), serde_json::Error> {
    FixtureKey::local(FIXTURE_SEED)
        .seal(receipt)
        .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))
}

/// Re-sign the receipt's body under the id it already carries, as a forger
/// reusing another receipt's id would.
pub(super) fn sign_keeping_id(receipt: &mut Receipt) -> Result<(), serde_json::Error> {
    FixtureKey::local(FIXTURE_SEED)
        .sign_keeping_id(receipt)
        .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))
}

pub(super) fn reference(reference_type: ReferenceType, id: &str) -> Reference {
    Reference::runx(reference_type, id)
}

/// Verify the tree `root` heads over the supplied `children`, under the
/// fixture key and the default tree rule.
pub(super) fn verify_tree(root: &Receipt, children: &[Receipt]) -> ReceiptVerification {
    verify_tree_with(
        root,
        &FixtureResolver { children },
        ReceiptTreeConfig::default(),
    )
}

/// Verify the tree `root` heads through `resolver` under `config`, with the
/// fixture key as the trusted signer.
pub(super) fn verify_tree_with(
    root: &Receipt,
    resolver: &impl ReceiptResolver,
    config: ReceiptTreeConfig,
) -> ReceiptVerification {
    verify_receipt_tree_proof_with_resolver(
        root,
        resolver,
        config,
        &FixtureProofContexts::default(),
    )
}

/// Give `receipt` the content address and digest of what it now reads,
/// leaving its signature as it was: for a walk that trusts every signature.
pub(super) fn address(receipt: &mut Receipt) -> Result<(), ReceiptError> {
    receipt.id = content_addressed_receipt_id(receipt)?.into();
    receipt.digest = canonical_receipt_body_digest(receipt)?.into();
    Ok(())
}

/// Verify the tree `root` heads through `resolver` under the default tree
/// rule, trusting every signature: the tests above prove signatures, and a
/// test of the walk itself need not pay for them.
pub(super) fn verify_tree_trusting_signatures(
    root: &Receipt,
    resolver: &impl ReceiptResolver,
) -> ReceiptVerification {
    verify_receipt_tree_proof_with_resolver(
        root,
        resolver,
        ReceiptTreeConfig::default(),
        &TrustingProofContexts,
    )
}

struct TrustingProofContexts;

impl SignatureVerifier for TrustingProofContexts {
    fn verify(
        &self,
        _issuer: &runx_contracts::ReceiptIssuer,
        _signature: &runx_contracts::ReceiptSignature,
        _body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        Ok(())
    }
}

impl ReceiptProofContextProvider for TrustingProofContexts {
    fn proof_context<'a>(&'a self, _receipt: &Receipt) -> ReceiptProofContext<'a> {
        ReceiptProofContext {
            signature_verifier: Some(self),
        }
    }
}

pub(super) fn assert_finding(
    verification: &ReceiptVerification,
    code: ReceiptFindingCode,
    path: &str,
) {
    assert!(
        verification
            .findings
            .iter()
            .any(|finding| finding.code == code && finding.path == path),
        "expected finding {code:?} at {path}; got {:?}",
        verification.findings
    );
}

struct FixtureProofContexts {
    verifier: FixtureKey,
}

impl Default for FixtureProofContexts {
    fn default() -> Self {
        Self {
            verifier: FixtureKey::local(FIXTURE_SEED),
        }
    }
}

impl ReceiptProofContextProvider for FixtureProofContexts {
    fn proof_context<'a>(&'a self, _receipt: &Receipt) -> ReceiptProofContext<'a> {
        ReceiptProofContext {
            signature_verifier: Some(&self.verifier),
        }
    }
}

/// The supplied receipts, each at `children[index]`, resolved by the typed
/// `runx:receipt:` id a ref names: two supplied receipts with one id are
/// ambiguous.
pub(super) struct FixtureResolver<'a> {
    pub(super) children: &'a [Receipt],
}

impl ReceiptResolver for FixtureResolver<'_> {
    type Child<'a>
        = &'a Receipt
    where
        Self: 'a;

    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<&'a Receipt> {
        let Some(receipt_id) = referenced_receipt_id(reference) else {
            return ReceiptResolveResult::Malformed;
        };
        let mut matches = self
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.id == receipt_id);
        let Some((index, receipt)) = matches.next() else {
            return ReceiptResolveResult::Missing;
        };
        if matches.next().is_some() {
            return ReceiptResolveResult::Ambiguous;
        }
        ReceiptResolveResult::Found(ResolvedReceipt {
            path: format!("children[{index}]"),
            receipt,
        })
    }

    fn supplied_receipts(&self) -> Vec<ResolvedReceipt<&Receipt>> {
        self.children
            .iter()
            .enumerate()
            .map(|(index, receipt)| ResolvedReceipt {
                path: format!("children[{index}]"),
                receipt,
            })
            .collect()
    }
}

pub(super) struct AmbiguousResolver;

impl ReceiptResolver for AmbiguousResolver {
    type Child<'a>
        = &'a Receipt
    where
        Self: 'a;

    fn resolve_child<'a>(&'a self, _reference: &Reference) -> ReceiptResolveResult<&'a Receipt> {
        ReceiptResolveResult::Ambiguous
    }

    fn supplied_receipts(&self) -> Vec<ResolvedReceipt<&Receipt>> {
        Vec::new()
    }
}

pub(super) struct ResolverErrorResolver;

impl ReceiptResolver for ResolverErrorResolver {
    type Child<'a>
        = &'a Receipt
    where
        Self: 'a;

    fn resolve_child<'a>(&'a self, _reference: &Reference) -> ReceiptResolveResult<&'a Receipt> {
        ReceiptResolveResult::ResolverError
    }

    fn supplied_receipts(&self) -> Vec<ResolvedReceipt<&Receipt>> {
        Vec::new()
    }
}

pub(super) struct HiddenChildResolver<'a> {
    pub(super) child: &'a Receipt,
}

impl ReceiptResolver for HiddenChildResolver<'_> {
    type Child<'a>
        = &'a Receipt
    where
        Self: 'a;

    fn resolve_child<'a>(&'a self, _reference: &Reference) -> ReceiptResolveResult<&'a Receipt> {
        ReceiptResolveResult::Found(ResolvedReceipt {
            path: "hidden_child".to_owned(),
            receipt: self.child,
        })
    }

    fn supplied_receipts(&self) -> Vec<ResolvedReceipt<&Receipt>> {
        Vec::new()
    }
}

pub(super) struct DuplicateIdResolver<'a> {
    pub(super) first: &'a Receipt,
    pub(super) second: &'a Receipt,
}

impl ReceiptResolver for DuplicateIdResolver<'_> {
    type Child<'a>
        = &'a Receipt
    where
        Self: 'a;

    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<&'a Receipt> {
        if reference.uri.ends_with(":first") {
            return ReceiptResolveResult::Found(ResolvedReceipt {
                path: "hidden_first".to_owned(),
                receipt: self.first,
            });
        }
        ReceiptResolveResult::Found(ResolvedReceipt {
            path: "hidden_second".to_owned(),
            receipt: self.second,
        })
    }

    fn supplied_receipts(&self) -> Vec<ResolvedReceipt<&Receipt>> {
        Vec::new()
    }
}

/// A store's resolver: it reads a receipt only when a ref names it, as a
/// copy lent for the walk of that receipt's subtree. It counts the copies it
/// has lent and not had back, and the most it ever had out.
pub(super) struct ReadingResolver {
    stored: BTreeMap<String, Receipt>,
    out: Cell<usize>,
    pub(super) most_out: Cell<usize>,
    pub(super) reads: Cell<usize>,
}

impl ReadingResolver {
    pub(super) fn storing(receipts: Vec<Receipt>) -> Self {
        Self {
            stored: receipts
                .into_iter()
                .map(|receipt| (receipt.id.to_string(), receipt))
                .collect(),
            out: Cell::new(0),
            most_out: Cell::new(0),
            reads: Cell::new(0),
        }
    }

    pub(super) fn out(&self) -> usize {
        self.out.get()
    }
}

/// A receipt read for one walk; dropping it hands it back.
pub(super) struct Lent<'a> {
    receipt: Receipt,
    out: &'a Cell<usize>,
}

impl std::ops::Deref for Lent<'_> {
    type Target = Receipt;

    fn deref(&self) -> &Receipt {
        &self.receipt
    }
}

impl Drop for Lent<'_> {
    fn drop(&mut self) {
        self.out.set(self.out.get() - 1);
    }
}

impl ReceiptResolver for ReadingResolver {
    type Child<'a>
        = Lent<'a>
    where
        Self: 'a;

    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<Lent<'a>> {
        let Some(receipt_id) = referenced_receipt_id(reference) else {
            return ReceiptResolveResult::Malformed;
        };
        let Some(stored) = self.stored.get(receipt_id) else {
            return ReceiptResolveResult::Missing;
        };
        self.reads.set(self.reads.get() + 1);
        self.out.set(self.out.get() + 1);
        self.most_out.set(self.most_out.get().max(self.out.get()));
        ReceiptResolveResult::Found(ResolvedReceipt {
            path: format!("store[{receipt_id}]"),
            receipt: Lent {
                receipt: stored.clone(),
                out: &self.out,
            },
        })
    }

    fn supplied_receipts(&self) -> Vec<ResolvedReceipt<&Receipt>> {
        Vec::new()
    }
}
