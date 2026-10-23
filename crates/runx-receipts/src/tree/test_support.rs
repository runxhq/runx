use runx_contracts::{Receipt, Reference, ReferenceType};
use serde::Deserialize;

use super::{ReceiptProofContextProvider, ReceiptResolveResult, ReceiptResolver, ResolvedReceipt};
use crate::{ReceiptFindingCode, ReceiptProofContext, ReceiptVerification};

#[path = "../../tests/fixture_key.rs"]
mod fixture_key;

use fixture_key::{FIXTURE_SEED, FixtureKey};

pub(super) const SUCCESS_RECEIPT: &str =
    include_str!("../../../../fixtures/contracts/harness-spine/receipt-success.json");
const ABNORMAL_RECEIPT: &str =
    include_str!("../../../../fixtures/contracts/harness-spine/receipt-abnormal.json");

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

/// Point the root's child ref at `child` by its id and digest, and reseal.
pub(super) fn link_child_digest(
    root: &mut Receipt,
    index: usize,
    child: &Receipt,
) -> Result<(), serde_json::Error> {
    child_refs_mut(root)[index] = Reference {
        locator: Some(child.digest.clone()),
        ..Reference::runx(ReferenceType::Receipt, &child.id)
    };
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

pub(super) struct FixtureProofContexts {
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

pub(super) struct AmbiguousResolver;

impl ReceiptResolver for AmbiguousResolver {
    fn resolve_child<'a>(&'a self, _reference: &Reference) -> ReceiptResolveResult<'a> {
        ReceiptResolveResult::Ambiguous
    }

    fn supplied_receipts<'a>(&'a self) -> Vec<ResolvedReceipt<'a>> {
        Vec::new()
    }
}

pub(super) struct ResolverErrorResolver;

impl ReceiptResolver for ResolverErrorResolver {
    fn resolve_child<'a>(&'a self, _reference: &Reference) -> ReceiptResolveResult<'a> {
        ReceiptResolveResult::ResolverError
    }

    fn supplied_receipts<'a>(&'a self) -> Vec<ResolvedReceipt<'a>> {
        Vec::new()
    }
}

pub(super) struct HiddenChildResolver<'a> {
    pub(super) child: &'a Receipt,
}

impl ReceiptResolver for HiddenChildResolver<'_> {
    fn resolve_child<'a>(&'a self, _reference: &Reference) -> ReceiptResolveResult<'a> {
        ReceiptResolveResult::Found(ResolvedReceipt {
            path: "hidden_child".to_owned(),
            receipt: self.child,
        })
    }

    fn supplied_receipts<'a>(&'a self) -> Vec<ResolvedReceipt<'a>> {
        Vec::new()
    }
}

pub(super) struct DuplicateIdResolver<'a> {
    pub(super) first: &'a Receipt,
    pub(super) second: &'a Receipt,
}

impl ReceiptResolver for DuplicateIdResolver<'_> {
    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<'a> {
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

    fn supplied_receipts<'a>(&'a self) -> Vec<ResolvedReceipt<'a>> {
        Vec::new()
    }
}
