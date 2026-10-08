use runx_contracts::{ReceiptClass, ReceiptEvidence, Reference, ReferenceType};

use super::{
    ReceiptTreeConfig, SliceReceiptResolver, validate_receipt_tree_with_resolver,
    verify_receipt_tree, verify_receipt_tree_with_resolver,
};
use crate::ReceiptFindingCode;

use super::test_support::{
    AmbiguousResolver, ResolverErrorResolver, SUCCESS_RECEIPT, assert_finding, child,
    child_refs_mut, digest, fixture, proof_composition_pair, reference,
};

#[test]
fn evidence_only_composite_is_reachable_and_valid() -> Result<(), serde_json::Error> {
    let (outer, inner) = proof_composition_pair()?;

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert!(verification.valid, "{:?}", verification.findings);
    Ok(())
}

#[test]
fn swapped_inner_receipt_is_rejected() -> Result<(), serde_json::Error> {
    let (outer, mut inner) = proof_composition_pair()?;
    let Some(binding) = inner.subject.paid_invocation.as_mut() else {
        return Err(serde_json::Error::io(std::io::Error::other(
            "composition fixture has no binding",
        )));
    };
    binding.input_digest = digest('9')?;

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::InnerReceiptBindingMismatch,
        "children[0].subject.paid_invocation",
    );
    Ok(())
}

#[test]
fn parent_digest_must_equal_outer_package_digest() -> Result<(), serde_json::Error> {
    let (mut outer, mut inner) = proof_composition_pair()?;
    let ReceiptEvidence::InnerReceipt { expected, .. } = &mut outer.evidence[0];
    let Some(parent_binding) = expected.parent_binding.as_mut() else {
        return Err(serde_json::Error::io(std::io::Error::other(
            "composition fixture has no parent",
        )));
    };
    parent_binding.execution_digest = digest('6')?;
    inner.subject.paid_invocation = Some(expected.clone());

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::InnerReceiptParentDigestMismatch,
        "children[0].subject.paid_invocation.parent_binding.execution_digest",
    );
    Ok(())
}

#[test]
fn composite_rejects_generic_lineage_backlink() -> Result<(), serde_json::Error> {
    let (outer, mut inner) = proof_composition_pair()?;
    inner.lineage.get_or_insert_with(Default::default).parent =
        Some(Reference::runx(ReferenceType::Receipt, &outer.id));

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::InnerReceiptLineageConflict,
        "children[0].lineage.parent",
    );
    Ok(())
}

#[test]
fn evidence_requires_mediated_outer_class() -> Result<(), serde_json::Error> {
    let (mut outer, inner) = proof_composition_pair()?;
    outer.class = ReceiptClass::Executed;

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::ReceiptEvidenceClassInvalid,
        "evidence",
    );
    Ok(())
}

#[test]
fn composite_requires_outer_paid_binding() -> Result<(), serde_json::Error> {
    let (mut outer, inner) = proof_composition_pair()?;
    outer.subject.paid_invocation = None;

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::ReceiptPaidBindingMissing,
        "subject.paid_invocation",
    );
    Ok(())
}

#[test]
fn composite_requires_outer_mediated_listing_terms() -> Result<(), serde_json::Error> {
    let (mut outer, inner) = proof_composition_pair()?;
    let Some(binding) = outer.subject.paid_invocation.as_mut() else {
        return Err(serde_json::Error::io(std::io::Error::other(
            "missing outer binding",
        )));
    };
    binding.mediation = None;

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::ReceiptPaidBindingMissing,
        "subject.paid_invocation.mediation",
    );
    Ok(())
}

#[test]
fn composite_inner_vendor_must_equal_outer_listing_vendor() -> Result<(), serde_json::Error> {
    let (mut outer, mut inner) = proof_composition_pair()?;
    let ReceiptEvidence::InnerReceipt { expected, .. } = &mut outer.evidence[0];
    expected.vendor_ref = runx_contracts::PrincipalReference::new(Reference::runx(
        ReferenceType::Principal,
        "other-vendor",
    ))
    .ok_or_else(|| serde_json::Error::io(std::io::Error::other("invalid vendor")))?;
    inner.subject.paid_invocation = Some(expected.clone());

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::InnerReceiptBindingMismatch,
        "evidence[0].expected.vendor_ref",
    );
    Ok(())
}

#[test]
fn composite_evidence_refs_must_be_unique() -> Result<(), serde_json::Error> {
    let (mut outer, inner) = proof_composition_pair()?;
    outer.evidence.push(outer.evidence[0].clone());

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::DuplicateReceiptEvidence,
        "evidence[1].receipt_ref",
    );
    Ok(())
}

#[test]
fn composite_expected_binding_requires_parent() -> Result<(), serde_json::Error> {
    let (mut outer, mut inner) = proof_composition_pair()?;
    let ReceiptEvidence::InnerReceipt { expected, .. } = &mut outer.evidence[0];
    expected.parent_binding = None;
    inner.subject.paid_invocation = Some(expected.clone());

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::InnerReceiptParentBindingMissing,
        "evidence[0].expected.parent_binding",
    );
    Ok(())
}

#[test]
fn composite_inner_receipt_must_be_executed() -> Result<(), serde_json::Error> {
    let (outer, mut inner) = proof_composition_pair()?;
    inner.class = ReceiptClass::Mediated;

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::InnerReceiptClassMismatch,
        "children[0].class",
    );
    Ok(())
}

#[test]
fn composite_parent_invocation_must_equal_outer_invocation() -> Result<(), serde_json::Error> {
    let (mut outer, mut inner) = proof_composition_pair()?;
    let ReceiptEvidence::InnerReceipt { expected, .. } = &mut outer.evidence[0];
    let Some(parent_binding) = expected.parent_binding.as_mut() else {
        return Err(serde_json::Error::io(std::io::Error::other(
            "composition fixture has no parent",
        )));
    };
    parent_binding.invocation_id = "paid_other".into();
    inner.subject.paid_invocation = Some(expected.clone());

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::InnerReceiptParentInvocationMismatch,
        "children[0].subject.paid_invocation.parent_binding.invocation_id",
    );
    Ok(())
}

#[test]
fn composite_pair_must_not_duplicate_the_lineage_edge() -> Result<(), serde_json::Error> {
    let (mut outer, inner) = proof_composition_pair()?;
    let ReceiptEvidence::InnerReceipt { receipt_ref, .. } = &outer.evidence[0];
    let receipt_ref = receipt_ref.clone();
    outer
        .lineage
        .get_or_insert_with(Default::default)
        .children
        .push(receipt_ref);

    let verification = verify_receipt_tree(&outer, &[inner]);

    assert_finding(
        &verification,
        ReceiptFindingCode::InnerReceiptLineageConflict,
        "evidence[0].receipt_ref",
    );
    Ok(())
}

#[test]
fn missing_inner_receipt_fails_closed() -> Result<(), serde_json::Error> {
    let (outer, _) = proof_composition_pair()?;

    let verification = verify_receipt_tree(&outer, &[]);

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptMissing,
        "evidence[0].receipt_ref",
    );
    Ok(())
}

#[test]
fn slice_adapter_accepts_only_typed_receipt_uri() -> Result<(), serde_json::Error> {
    let mut root = fixture(SUCCESS_RECEIPT)?;
    let child = child("rcpt_child_1")?;

    child_refs_mut(&mut root)[0].uri = "rcpt_child_1".to_owned().into();
    let verification = verify_receipt_tree(&root, std::slice::from_ref(&child));
    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );

    child_refs_mut(&mut root)[0].uri = "runx:receipt:rcpt_child_1".to_owned().into();
    assert!(verify_receipt_tree(&root, &[child]).valid);
    Ok(())
}

#[test]
fn malformed_and_wrong_namespace_refs_are_stable_findings() -> Result<(), serde_json::Error> {
    let mut root = fixture(SUCCESS_RECEIPT)?;
    let child = child("rcpt_child_1")?;

    child_refs_mut(&mut root)[0].uri = "runx:graph_receipt:rcpt_child_1".to_owned().into();
    let verification = verify_receipt_tree(&root, std::slice::from_ref(&child));
    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );

    child_refs_mut(&mut root)[0].uri = ":rcpt_child_1".to_owned().into();
    let verification = verify_receipt_tree(&root, &[child]);
    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn suffix_only_refs_are_malformed_not_aliases() -> Result<(), serde_json::Error> {
    let mut root = fixture(SUCCESS_RECEIPT)?;
    child_refs_mut(&mut root)[0].uri = "child_1".to_owned().into();
    let child = child("rcpt_child_1")?;

    let verification = verify_receipt_tree(&root, &[child]);

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn duplicate_ids_make_slice_resolution_ambiguous() -> Result<(), serde_json::Error> {
    let root = fixture(SUCCESS_RECEIPT)?;
    let first = child("rcpt_child_1")?;
    let second = child("rcpt_child_1")?;

    let verification = verify_receipt_tree(&root, &[first, second]);

    assert_finding(
        &verification,
        ReceiptFindingCode::DuplicateChildReceipt,
        "children[1].id",
    );
    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptAmbiguous,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn resolver_ambiguous_result_is_a_stable_finding() -> Result<(), serde_json::Error> {
    let root = fixture(SUCCESS_RECEIPT)?;

    let verification =
        verify_receipt_tree_with_resolver(&root, &AmbiguousResolver, ReceiptTreeConfig::default());

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptAmbiguous,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn resolver_error_result_is_a_stable_finding() -> Result<(), serde_json::Error> {
    let root = fixture(SUCCESS_RECEIPT)?;

    let verification = verify_receipt_tree_with_resolver(
        &root,
        &ResolverErrorResolver,
        ReceiptTreeConfig::default(),
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptResolverError,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn strict_mode_rejects_mismatched_parent_link() -> Result<(), serde_json::Error> {
    let root = fixture(SUCCESS_RECEIPT)?;
    let mut child = child("rcpt_child_1")?;
    child.lineage.get_or_insert_with(Default::default).parent =
        Some(reference(ReferenceType::Receipt, "other"));

    let verification = verify_receipt_tree_with_resolver(
        &root,
        &SliceReceiptResolver {
            children: std::slice::from_ref(&child),
        },
        ReceiptTreeConfig {
            require_parent_links: true,
            ..ReceiptTreeConfig::default()
        },
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptParentMismatch,
        "lineage.children[0].lineage.parent",
    );
    Ok(())
}

#[test]
fn strict_mode_requires_present_parent_link() -> Result<(), serde_json::Error> {
    let root = fixture(SUCCESS_RECEIPT)?;
    let child = child("rcpt_child_1")?;

    let verification = verify_receipt_tree_with_resolver(
        &root,
        &SliceReceiptResolver {
            children: std::slice::from_ref(&child),
        },
        ReceiptTreeConfig {
            require_parent_links: true,
            ..ReceiptTreeConfig::default()
        },
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptParentMismatch,
        "lineage.children[0].lineage.parent",
    );
    Ok(())
}

#[test]
fn depth_limit_blocks_hostile_nested_tree() -> Result<(), serde_json::Error> {
    let root = fixture(SUCCESS_RECEIPT)?;
    let mut child_receipt = child("rcpt_child_1")?;
    child_refs_mut(&mut child_receipt).push(reference(ReferenceType::Receipt, "grandchild"));
    let grandchild = child("grandchild")?;

    let verification = verify_receipt_tree_with_resolver(
        &root,
        &SliceReceiptResolver {
            children: &[child_receipt, grandchild],
        },
        ReceiptTreeConfig {
            max_depth: 1,
            ..ReceiptTreeConfig::default()
        },
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptDepthLimit,
        "children[0].lineage.children[0]",
    );
    Ok(())
}

#[test]
fn breadth_limit_blocks_hostile_fanout() -> Result<(), serde_json::Error> {
    let mut root = fixture(SUCCESS_RECEIPT)?;
    child_refs_mut(&mut root).push(reference(ReferenceType::Receipt, "second"));
    let first = child("rcpt_child_1")?;
    let second = child("second")?;

    let verification = verify_receipt_tree_with_resolver(
        &root,
        &SliceReceiptResolver {
            children: &[first, second],
        },
        ReceiptTreeConfig {
            max_breadth: 1,
            ..ReceiptTreeConfig::default()
        },
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptBreadthLimit,
        "lineage.children",
    );
    Ok(())
}

#[test]
fn positive_nested_tree_verifies() -> Result<(), serde_json::Error> {
    let root = fixture(SUCCESS_RECEIPT)?;
    let mut child_receipt = child("rcpt_child_1")?;
    child_refs_mut(&mut child_receipt).push(reference(ReferenceType::Receipt, "grandchild"));
    let grandchild = child("grandchild")?;

    assert!(verify_receipt_tree(&root, &[child_receipt, grandchild]).valid);
    Ok(())
}

#[test]
fn positive_fanout_tree_verifies() -> Result<(), serde_json::Error> {
    let mut root = fixture(SUCCESS_RECEIPT)?;
    child_refs_mut(&mut root).push(reference(ReferenceType::Receipt, "second"));
    let first = child("rcpt_child_1")?;
    let second = child("second")?;

    assert!(verify_receipt_tree(&root, &[first, second]).valid);
    Ok(())
}

#[test]
fn strict_parent_links_can_verify_cleanly() -> Result<(), serde_json::Error> {
    let root = fixture(SUCCESS_RECEIPT)?;
    let mut child = child("rcpt_child_1")?;
    child.lineage.get_or_insert_with(Default::default).parent =
        Some(Reference::runx(ReferenceType::Receipt, &root.id));

    assert!(
        validate_receipt_tree_with_resolver(
            &root,
            &SliceReceiptResolver {
                children: std::slice::from_ref(&child),
            },
            ReceiptTreeConfig {
                require_parent_links: true,
                ..ReceiptTreeConfig::default()
            },
        )
        .is_ok()
    );
    Ok(())
}
