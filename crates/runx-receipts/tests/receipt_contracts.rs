// Test oracle: asserting via expect/unwrap is the intended failure mode, so the
// workspace expect/unwrap bans are lifted for this test target.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde::Deserialize;

use crate::fixture_key::{FIXTURE_SEED, FixtureKey};
use runx_contracts::{
    ProofKind, Receipt, ReceiptIssuer, ReceiptIssuerType, ReceiptSignature, Reference,
    ReferenceType, RunOutcome,
};
use runx_receipts::{
    ReceiptFindingCode, ReceiptProofContext, ReceiptProofStatusKind, ReceiptVerification,
    SignatureVerificationFailure, SignatureVerifier, canonical_receipt_digest,
    canonical_receipt_json, receipt_proof_status, validate_receipt, validate_receipt_proof,
    validate_receipt_tree, verify_receipt, verify_receipt_proof, verify_receipt_tree,
};

const SUCCESS_RECEIPT: &str =
    include_str!("../../../fixtures/contracts/harness-spine/receipt-success.json");
const ABNORMAL_RECEIPT: &str =
    include_str!("../../../fixtures/contracts/harness-spine/receipt-abnormal.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    expected: Receipt,
}

fn with_child(mut receipt: Receipt) -> Receipt {
    receipt
        .lineage
        .children
        .push(Reference::runx(ReferenceType::Receipt, "rcpt_child_1"));
    receipt
}

#[test]
fn success_receipt_verifies_basic_invariants() -> Result<(), serde_json::Error> {
    let receipt = fixture(SUCCESS_RECEIPT)?;

    assert!(validate_receipt(&receipt).is_ok());
    assert!(matches!(
        canonical_receipt_json(&receipt),
        Ok(json) if json.starts_with(r#"{"acts":"#)
    ));
    assert!(matches!(
        canonical_receipt_digest(&receipt),
        Ok(digest) if digest.starts_with("sha256:")
    ));
    Ok(())
}

#[test]
fn abnormal_failed_receipt_verifies_basic_invariants() -> Result<(), serde_json::Error> {
    let receipt = fixture(ABNORMAL_RECEIPT)?;

    assert!(validate_receipt(&receipt).is_ok());
    assert_eq!(receipt.seal.outcome, RunOutcome::Failed);
    Ok(())
}

#[test]
fn seal_criterion_must_bind_to_act_criteria() -> Result<(), serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    receipt.seal.criteria[0].criterion_id = runx_contracts::Criterion::PacketSchemasVerified;

    let verification = verify_receipt(&receipt);

    assert_finding(&verification, ReceiptFindingCode::SealCriterionUnbound);
    Ok(())
}

#[test]
fn child_refs_must_be_receipt_refs() -> Result<(), serde_json::Error> {
    let mut receipt = with_child(fixture(SUCCESS_RECEIPT)?);
    receipt.lineage.children[0].reference_type = ReferenceType::Artifact;

    let verification = verify_receipt(&receipt);

    assert_finding(&verification, ReceiptFindingCode::ChildReceiptRefInvalid);
    Ok(())
}

#[test]
fn digest_requires_sha256_prefix() -> Result<(), serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    receipt.digest = "not-a-hash".into();

    let verification = verify_receipt(&receipt);

    assert_finding(&verification, ReceiptFindingCode::HashCommitmentInvalid);
    Ok(())
}

#[test]
fn payment_authority_bound_survives_in_body() -> Result<(), serde_json::Error> {
    // Inspectability acceptance: the granted authority stays readable in the body.
    let receipt = fixture(SUCCESS_RECEIPT)?;
    let json = canonical_receipt_json(&receipt).expect("canonical json");
    assert!(json.contains("\"authority\""));
    assert!(json.contains("\"terms\""));
    Ok(())
}

#[test]
fn effect_evidence_requires_authority_grant_refs() -> Result<(), serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    let mut evidence = Reference::runx(ReferenceType::Verification, "effect-proof-1");
    evidence.proof_kind = Some(ProofKind::EffectEvidence);
    receipt.acts[0].criterion_bindings[0]
        .verification_refs
        .push(evidence);
    receipt.authority.grant_refs.clear();

    let verification = verify_receipt(&receipt);

    assert_finding(
        &verification,
        ReceiptFindingCode::EffectGrantEvidenceMissing,
    );
    Ok(())
}

#[test]
fn an_acts_legitimacy_names_a_term_the_authority_holds() -> Result<(), serde_json::Error> {
    let receipt = fixture(SUCCESS_RECEIPT)?;
    assert!(validate_receipt(&receipt).is_ok());

    let mut unbound = receipt;
    unbound.authority.terms.clear();

    assert_finding(
        &verify_receipt(&unbound),
        ReceiptFindingCode::ActLegitimacyTermMissing,
    );
    Ok(())
}

#[test]
fn effect_evidence_in_the_seal_requires_authority_grant_refs() -> Result<(), serde_json::Error> {
    // A step that made no governed call seals no act; its evidence is the seal's.
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    receipt.acts.clear();
    receipt.decisions.clear();
    let mut evidence = Reference::runx(ReferenceType::Verification, "effect-proof-1");
    evidence.proof_kind = Some(ProofKind::EffectEvidence);
    receipt.seal.criteria[0].verification_refs.push(evidence);
    receipt.authority.grant_refs.clear();

    let verification = verify_receipt(&receipt);

    assert_finding(
        &verification,
        ReceiptFindingCode::EffectGrantEvidenceMissing,
    );
    Ok(())
}

#[test]
fn effect_evidence_accepts_authority_grant_refs() -> Result<(), serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    let mut evidence = Reference::runx(ReferenceType::Verification, "effect-proof-1");
    evidence.proof_kind = Some(ProofKind::EffectEvidence);
    receipt.acts[0].criterion_bindings[0]
        .verification_refs
        .push(evidence);
    receipt
        .authority
        .grant_refs
        .push(Reference::runx(ReferenceType::Grant, "operator-grant-1"));

    assert!(validate_receipt(&receipt).is_ok());
    Ok(())
}

#[test]
fn receipt_tree_requires_supplied_child_receipts() -> Result<(), serde_json::Error> {
    let receipt = with_child(fixture(SUCCESS_RECEIPT)?);

    let verification = verify_receipt_tree(&receipt, &[]);

    assert_finding(&verification, ReceiptFindingCode::ChildReceiptMissing);
    Ok(())
}

#[test]
fn receipt_tree_accepts_matching_child_receipts() -> Result<(), serde_json::Error> {
    let mut receipt = with_child(fixture(SUCCESS_RECEIPT)?);
    let mut child = fixture(ABNORMAL_RECEIPT)?;
    child.id = "rcpt_child_1".into();

    assert!(validate_receipt_tree(&receipt, &[child]).is_ok());
    receipt.lineage.children.clear();
    assert!(validate_receipt_tree(&receipt, &[]).is_ok());
    Ok(())
}

#[test]
fn receipt_tree_rejects_child_receipt_cycles() -> Result<(), serde_json::Error> {
    let receipt = with_child(fixture(SUCCESS_RECEIPT)?);
    let mut child = fixture(ABNORMAL_RECEIPT)?;
    child.id = "rcpt_child_1".into();
    child
        .lineage
        .children
        .push(Reference::runx(ReferenceType::Receipt, "rcpt_child_1"));

    let verification = verify_receipt_tree(&receipt, &[child]);

    assert_finding(&verification, ReceiptFindingCode::ChildReceiptCycle);
    Ok(())
}

#[test]
fn receipt_tree_rejects_orphan_supplied_children() -> Result<(), serde_json::Error> {
    let receipt = fixture(SUCCESS_RECEIPT)?;
    let mut child = fixture(ABNORMAL_RECEIPT)?;
    child.id = "rcpt_orphan".into();

    let verification = verify_receipt_tree(&receipt, &[child]);

    assert_finding(&verification, ReceiptFindingCode::OrphanChildReceipt);
    Ok(())
}

#[test]
fn receipt_tree_rejects_duplicate_child_receipt_ids() -> Result<(), serde_json::Error> {
    let receipt = with_child(fixture(SUCCESS_RECEIPT)?);
    let mut first = fixture(ABNORMAL_RECEIPT)?;
    first.id = "rcpt_child_1".into();
    let second = first.clone();

    let verification = verify_receipt_tree(&receipt, &[first, second]);

    assert_finding(&verification, ReceiptFindingCode::DuplicateChildReceipt);
    Ok(())
}

#[test]
fn an_issuer_type_no_issuer_signs_as_is_refused() {
    let json = r#"{"type":"verifier","kid":"key_1","public_key_digest":"sha256:public","runx_version":"0.11.0"}"#;

    assert!(serde_json::from_str::<ReceiptIssuer>(json).is_err());
}

#[test]
fn strict_proof_accepts_recomputed_digest_and_signature() -> Result<(), serde_json::Error> {
    let receipt = proof_receipt()?;
    let verifier = fixture_key();
    let context = proof_context(&verifier);

    assert!(validate_receipt_proof(&receipt, &context).is_ok());
    Ok(())
}

#[test]
fn structural_validation_does_not_claim_strict_proof() -> Result<(), serde_json::Error> {
    let receipt = fixture(SUCCESS_RECEIPT)?;

    assert!(validate_receipt(&receipt).is_ok());
    assert_finding(
        &verify_receipt_proof(&receipt, &ReceiptProofContext::default()),
        ReceiptFindingCode::SignatureVerifierMissing,
    );
    Ok(())
}

#[test]
fn strict_proof_rejects_tampered_body_digest() -> Result<(), serde_json::Error> {
    let mut receipt = proof_receipt()?;
    receipt.acts[0].intent.purpose = "tampered".into();
    let verifier = fixture_key();
    let context = proof_context(&verifier);

    let verification = verify_receipt_proof(&receipt, &context);

    assert_finding(&verification, ReceiptFindingCode::SealDigestMismatch);
    assert_finding(&verification, ReceiptFindingCode::SignatureInvalid);
    Ok(())
}

#[test]
fn strict_proof_requires_signature_verifier() -> Result<(), serde_json::Error> {
    let receipt = proof_receipt()?;

    let verification = verify_receipt_proof(&receipt, &ReceiptProofContext::default());

    assert_finding(&verification, ReceiptFindingCode::SignatureVerifierMissing);
    Ok(())
}

#[test]
fn strict_proof_rejects_tampered_signature() -> Result<(), serde_json::Error> {
    let mut receipt = proof_receipt()?;
    receipt.signature.value = format!("base64:{}", "A".repeat(86)).into();
    let verifier = fixture_key();
    let context = proof_context(&verifier);

    let verification = verify_receipt_proof(&receipt, &context);

    assert_finding(&verification, ReceiptFindingCode::SignatureInvalid);
    Ok(())
}

#[test]
fn strict_proof_reports_unsupported_issuer() -> Result<(), serde_json::Error> {
    let mut receipt = proof_receipt()?;
    FixtureKey::new(FIXTURE_SEED, ReceiptIssuerType::Ci, "ci-fixture-key")
        .seal(&mut receipt)
        .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))?;
    let verifier = UnsupportedIssuerVerifier;
    let context = ReceiptProofContext {
        signature_verifier: Some(&verifier),
    };

    let verification = verify_receipt_proof(&receipt, &context);

    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureUnsupportedIssuer,
    );
    Ok(())
}

#[test]
fn inline_decision_integrity_catches_tampered_selected_act_id() -> Result<(), serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    // The reasoning is inline; the selected_act_id integrity property is checked
    // against the inline acts[] with no journal indirection.
    assert!(!receipt.decisions.is_empty());
    receipt.decisions[0].selected_act_id = "missing_act".into();

    let verification = verify_receipt(&receipt);
    assert_finding(
        &verification,
        ReceiptFindingCode::DecisionSelectedActMissing,
    );
    Ok(())
}

#[test]
fn inline_decision_integrity_accepts_real_selected_act_id() -> Result<(), serde_json::Error> {
    let receipt = fixture(SUCCESS_RECEIPT)?;
    let act_id = receipt.acts[0].id.clone();
    assert_eq!(
        receipt.decisions[0].selected_act_id.as_str(),
        act_id.as_str()
    );

    assert!(validate_receipt(&receipt).is_ok());
    Ok(())
}

#[test]
fn proof_status_projects_safe_public_summary() -> Result<(), serde_json::Error> {
    let receipt = proof_receipt()?;
    let verifier = fixture_key();
    let verification = verify_receipt_proof(&receipt, &proof_context(&verifier));

    let status = receipt_proof_status(&receipt, &verification);

    assert_eq!(status.receipt_id, receipt.id);
    assert_eq!(status.status, ReceiptProofStatusKind::Verified);
    assert!(status.finding_summaries.is_empty());
    Ok(())
}

#[test]
fn proof_status_redacts_absolute_paths_from_findings() -> Result<(), serde_json::Error> {
    let receipt = proof_receipt()?;
    let verification = ReceiptVerification::from_findings(vec![runx_receipts::ReceiptFinding {
        code: ReceiptFindingCode::SignatureInvalid,
        path: "(/Users/kam/private/key)".to_owned(),
        message: "failed at /Users/kam/private/key and C:\\Users\\kam\\private\\key with VGhpcy1sb29rcy1saWtlLWEtc2VjcmV0LXZhbHVlPQ==".to_owned(),
    }]);

    let status = receipt_proof_status(&receipt, &verification);

    assert_eq!(status.status, ReceiptProofStatusKind::Failed);
    assert_eq!(status.finding_summaries[0].path, "[local-path]");
    assert_eq!(
        status.finding_summaries[0].message,
        "failed at [local-path] and [local-path] with [secret]"
    );
    Ok(())
}

fn fixture(json: &str) -> Result<Receipt, serde_json::Error> {
    serde_json::from_str::<Fixture>(json).map(|fixture| fixture.expected)
}

fn proof_receipt() -> Result<Receipt, serde_json::Error> {
    let mut receipt = fixture(SUCCESS_RECEIPT)?;
    refresh_proof_digest_and_signature(&mut receipt)?;
    Ok(receipt)
}

fn refresh_proof_digest_and_signature(receipt: &mut Receipt) -> Result<(), serde_json::Error> {
    fixture_key()
        .seal(receipt)
        .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))
}

fn fixture_key() -> FixtureKey {
    FixtureKey::local(FIXTURE_SEED)
}

fn proof_context(verifier: &FixtureKey) -> ReceiptProofContext<'_> {
    ReceiptProofContext {
        signature_verifier: Some(verifier),
    }
}

struct UnsupportedIssuerVerifier;

impl SignatureVerifier for UnsupportedIssuerVerifier {
    fn verify(
        &self,
        _issuer: &ReceiptIssuer,
        _signature: &ReceiptSignature,
        _body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        Err(SignatureVerificationFailure::UnsupportedIssuer)
    }
}

fn assert_finding(verification: &ReceiptVerification, code: ReceiptFindingCode) {
    assert!(
        verification
            .findings
            .iter()
            .any(|finding| finding.code == code),
        "expected finding {code:?}; got {:?}",
        verification.findings
    );
}
