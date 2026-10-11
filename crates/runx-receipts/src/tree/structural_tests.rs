use runx_contracts::ReferenceType;

use super::{
    ReceiptTreeConfig, SliceReceiptResolver, verify_receipt_tree, verify_receipt_tree_with_resolver,
};
use crate::ReceiptFindingCode;

use super::test_support::{
    AmbiguousResolver, ResolverErrorResolver, SUCCESS_RECEIPT, assert_finding, child,
    child_refs_mut, fixture, reference,
};

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
