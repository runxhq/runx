use std::collections::BTreeSet;

use runx_contracts::{
    OfferRevisionRef, ParentInvocationBinding, PrincipalReference, Receipt, ReceiptClass,
    ReceiptEvidence, ReceiptIssuer, ReceiptPaidInvocationBinding, Reference, ReferenceType,
    Sha256Digest,
};
use serde::Deserialize;

use super::{ReceiptProofContextProvider, ReceiptResolveResult, ReceiptResolver, ResolvedReceipt};
use crate::{
    ReceiptFindingCode, ReceiptProofContext, ReceiptSignature, ReceiptVerification,
    SignatureVerificationFailure, SignatureVerifier, canonical_receipt_body_digest,
};

pub(super) const SUCCESS_RECEIPT: &str =
    include_str!("../../../../fixtures/contracts/harness-spine/receipt-success.json");
const ABNORMAL_RECEIPT: &str =
    include_str!("../../../../fixtures/contracts/harness-spine/receipt-abnormal.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    expected: Receipt,
}

pub(super) fn child_refs_mut(receipt: &mut Receipt) -> &mut Vec<Reference> {
    &mut receipt
        .lineage
        .get_or_insert_with(Default::default)
        .children
}

pub(super) fn fixture(json: &str) -> Result<Receipt, serde_json::Error> {
    let mut receipt = serde_json::from_str::<Fixture>(json).map(|fixture| fixture.expected)?;
    // The flat success fixture carries no children; the tree tests need one
    // typed child ref to mutate, so seed a single receipt ref.
    if receipt
        .lineage
        .as_ref()
        .is_none_or(|lineage| lineage.children.is_empty())
    {
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

pub(super) fn proof_child(id: &str) -> Result<Receipt, serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    receipt.id = id.into();
    child_refs_mut(&mut receipt).clear();
    refresh_proof_digest_and_signature(&mut receipt)?;
    Ok(receipt)
}

pub(super) fn proof_composition_pair() -> Result<(Receipt, Receipt), serde_json::Error> {
    let mut outer = proof_root()?;
    child_refs_mut(&mut outer).clear();
    outer.class = ReceiptClass::Mediated;
    let parent_binding = ParentInvocationBinding {
        invocation_id: "paid_outer".into(),
        execution_digest: digest('7')?,
    };
    let expected = paid_binding("paid_inner", '8', Some(parent_binding))?;
    let mut outer_binding = paid_binding("paid_outer", '7', None)?;
    outer_binding.mediation = Some(serde_json::from_value(serde_json::json!({
        "listing_ref": "runx:listing:ausca/document-ocr@1.0.0#invoke",
        "endpoint_url": "https://vendor.example/v1/invocations",
        "vendor_offer_revision": expected.offer_revision.clone(),
        "vendor_package_digest": expected.package_digest.clone(),
        "vendor_amount_minor": 100,
        "platform_fee_minor": 25,
        "currency": "USD",
        "settlement_family": "x402",
        "expected_receipt_class": "executed"
    }))?);
    outer.subject.paid_invocation = Some(outer_binding);

    let mut inner = proof_child("paid_inner_receipt")?;
    inner.class = ReceiptClass::Executed;
    inner.subject.paid_invocation = Some(expected.clone());
    refresh_proof_digest_and_signature(&mut inner)?;

    let mut receipt_ref = Reference::runx(ReferenceType::Receipt, &inner.id);
    receipt_ref.locator = Some(inner.digest.clone());
    outer.evidence = vec![ReceiptEvidence::InnerReceipt {
        receipt_ref,
        expected,
    }];
    refresh_proof_digest_and_signature(&mut outer)?;
    Ok((outer, inner))
}

fn paid_binding(
    invocation_id: &str,
    package: char,
    parent_binding: Option<ParentInvocationBinding>,
) -> Result<ReceiptPaidInvocationBinding, serde_json::Error> {
    let vendor_ref = PrincipalReference::new(Reference::runx(ReferenceType::Principal, "vendor-1"))
        .ok_or_else(|| fixture_error("fixture principal reference is invalid"))?;
    Ok(ReceiptPaidInvocationBinding {
        invocation_id: invocation_id.into(),
        vendor_ref,
        offer_revision: OfferRevisionRef {
            offer_id: "ocr-v1".into(),
            revision: "2026-08-22.1".into(),
            revision_digest: digest('4')?,
            input_schema_digest: digest('2')?,
            output_schema_digest: digest('3')?,
        },
        package_digest: digest(package)?,
        input_digest: digest('1')?,
        mediation: None,
        parent_binding,
    })
}

pub(super) fn digest(value: char) -> Result<Sha256Digest, serde_json::Error> {
    Sha256Digest::new(format!("sha256:{}", value.to_string().repeat(64)))
        .ok_or_else(|| fixture_error("fixture digest is invalid"))
}

fn fixture_error(message: &str) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::other(message))
}

pub(super) fn link_child_digest(
    root: &mut Receipt,
    index: usize,
    child: &Receipt,
) -> Result<(), serde_json::Error> {
    child_refs_mut(root)[index].locator = Some(child.digest.clone());
    refresh_proof_digest_and_signature(root)
}

pub(super) fn refresh_proof_digest_and_signature(
    receipt: &mut Receipt,
) -> Result<(), serde_json::Error> {
    let digest = canonical_receipt_body_digest(receipt)
        .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))?;
    receipt.digest = digest.clone().into();
    receipt.signature.value = format!("sig:{digest}").into();
    Ok(())
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

#[derive(Default)]
pub(super) struct FixtureProofContexts {
    verifier: FixtureSignatureVerifier,
}

impl ReceiptProofContextProvider for FixtureProofContexts {
    fn proof_context<'a>(&'a self, _receipt: &Receipt) -> ReceiptProofContext<'a> {
        ReceiptProofContext {
            signature_verifier: Some(&self.verifier),
            authority_verified: true,
            external_attestations_verified: true,
            verified_redaction_refs: BTreeSet::new(),
            verified_hash_commitments: BTreeSet::new(),
        }
    }
}

#[derive(Default)]
struct FixtureSignatureVerifier;

impl SignatureVerifier for FixtureSignatureVerifier {
    fn verify(
        &self,
        _issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        if signature.value == format!("sig:{body_digest}") {
            Ok(())
        } else {
            Err(SignatureVerificationFailure::SignatureMismatch)
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
