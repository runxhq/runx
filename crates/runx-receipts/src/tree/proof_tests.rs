use runx_contracts::ReferenceType;

use super::{
    ReceiptTreeConfig, validate_receipt_tree_proof, verify_receipt_tree, verify_receipt_tree_proof,
    verify_receipt_tree_proof_with_resolver, verify_receipt_tree_with_resolver,
};
use crate::ReceiptFindingCode;

use super::test_support::{
    DuplicateIdResolver, FixtureProofContexts, HiddenChildResolver, ResolverErrorResolver,
    assert_finding, child_refs_mut, link_child_digest, proof_child, proof_composition_pair,
    proof_root, reference, refresh_proof_digest_and_signature,
};

#[test]
fn strict_proof_accepts_digest_bound_composite() -> Result<(), serde_json::Error> {
    let (outer, inner) = proof_composition_pair()?;
    let proof_contexts = FixtureProofContexts::default();

    let verification = verify_receipt_tree_proof(&outer, &[inner], &proof_contexts);

    assert!(verification.valid, "{:?}", verification.findings);
    Ok(())
}

#[test]
fn strict_proof_rejects_swapped_inner_digest() -> Result<(), serde_json::Error> {
    let (outer, mut inner) = proof_composition_pair()?;
    inner.acts[0].summary = "alternate valid execution".into();
    refresh_proof_digest_and_signature(&mut inner)?;
    let proof_contexts = FixtureProofContexts::default();

    let verification = verify_receipt_tree_proof(&outer, &[inner], &proof_contexts);

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptDigestMismatch,
        "children[0].locator",
    );
    Ok(())
}

#[test]
fn strict_tree_proof_accepts_root_and_child() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;
    let proof_contexts = FixtureProofContexts::default();

    assert!(validate_receipt_tree_proof(&root, &[child], &proof_contexts).is_ok());
    Ok(())
}

#[test]
fn strict_tree_proof_rejects_missing_child() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;
    let proof_contexts = FixtureProofContexts::default();

    let verification = verify_receipt_tree_proof(&root, &[], &proof_contexts);

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptMissing,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn strict_tree_proof_rejects_extra_child() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;
    let extra = proof_child("rcpt_extra")?;
    let proof_contexts = FixtureProofContexts::default();

    let verification = verify_receipt_tree_proof(&root, &[child, extra], &proof_contexts);

    assert_finding(
        &verification,
        ReceiptFindingCode::OrphanChildReceipt,
        "children[1].id",
    );
    Ok(())
}

#[test]
fn strict_tree_proof_rejects_untyped_exact_id_child_ref() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;
    child_refs_mut(&mut root)[0].uri = child.id.clone();
    refresh_proof_digest_and_signature(&mut root)?;
    let proof_contexts = FixtureProofContexts::default();

    let verification = verify_receipt_tree_proof(&root, &[child], &proof_contexts);

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn strict_tree_proof_rejects_structurally_valid_child_proof_mismatch()
-> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let mut child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;
    child.acts[0].summary = "tampered child proof body".into();
    let proof_contexts = FixtureProofContexts::default();

    assert!(verify_receipt_tree(&root, std::slice::from_ref(&child)).valid);
    let verification = verify_receipt_tree_proof(&root, &[child], &proof_contexts);

    assert_finding(
        &verification,
        ReceiptFindingCode::SealDigestMismatch,
        "children[0].digest",
    );
    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureInvalid,
        "children[0].signature.value",
    );
    Ok(())
}

#[test]
fn strict_tree_proof_rejects_valid_alternate_child_with_same_id() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let original = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &original)?;
    let mut alternate = proof_child("rcpt_child_1")?;
    alternate.acts[0].summary = "valid alternate child body".into();
    refresh_proof_digest_and_signature(&mut alternate)?;
    let proof_contexts = FixtureProofContexts::default();

    let verification = verify_receipt_tree_proof(&root, &[alternate], &proof_contexts);

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptDigestMismatch,
        "children[0].locator",
    );
    Ok(())
}

#[test]
fn strict_tree_proof_rejects_custom_resolver_child_not_in_supplied_receipts()
-> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let mut child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;
    child.acts[0].summary = "hidden tampered child".into();
    let resolver = HiddenChildResolver { child: &child };
    let proof_contexts = FixtureProofContexts::default();

    assert!(
        verify_receipt_tree_with_resolver(&root, &resolver, ReceiptTreeConfig::default()).valid
    );
    let verification = verify_receipt_tree_proof_with_resolver(
        &root,
        &resolver,
        ReceiptTreeConfig::default(),
        &proof_contexts,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::SealDigestMismatch,
        "hidden_child.digest",
    );
    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureInvalid,
        "hidden_child.signature.value",
    );
    Ok(())
}

#[test]
fn strict_tree_proof_rejects_resolver_error() -> Result<(), serde_json::Error> {
    let root = proof_root()?;
    let proof_contexts = FixtureProofContexts::default();

    let verification = verify_receipt_tree_proof_with_resolver(
        &root,
        &ResolverErrorResolver,
        ReceiptTreeConfig::default(),
        &proof_contexts,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptResolverError,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn strict_tree_proof_rejects_custom_resolver_duplicate_id_child_after_reached()
-> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let first = proof_child("shared_child")?;
    let mut second = proof_child("shared_child")?;
    *child_refs_mut(&mut root) = vec![
        reference(ReferenceType::Receipt, "first"),
        reference(ReferenceType::Receipt, "second"),
    ];
    child_refs_mut(&mut root)[0].locator = Some(first.digest.clone());
    child_refs_mut(&mut root)[1].locator = Some(second.digest.clone());
    refresh_proof_digest_and_signature(&mut root)?;
    second.acts[0].summary = "hidden duplicate-id tamper".into();
    let resolver = DuplicateIdResolver {
        first: &first,
        second: &second,
    };
    let proof_contexts = FixtureProofContexts::default();

    let verification = verify_receipt_tree_proof_with_resolver(
        &root,
        &resolver,
        ReceiptTreeConfig::default(),
        &proof_contexts,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::SealDigestMismatch,
        "hidden_second.digest",
    );
    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureInvalid,
        "hidden_second.signature.value",
    );
    Ok(())
}
