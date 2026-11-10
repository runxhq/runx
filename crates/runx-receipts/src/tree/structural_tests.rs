use runx_contracts::connector::Digest;
use runx_contracts::javascript_worker::MAX_SCRIPT_CALLS;
use runx_contracts::{MAX_RECEIPT_TREE_RECEIPTS, RECEIPTS_PER_SKILL_CALL, Reference};

use super::ReceiptTreeConfig;
use crate::ReceiptFindingCode;

use super::test_support::{
    AmbiguousResolver, FixtureResolver, ReadingResolver, address, assert_finding, child_link,
    child_refs_mut, link_child_digest, proof_child, proof_root, refresh_proof_digest_and_signature,
    verify_tree, verify_tree_trusting_signatures, verify_tree_with,
};

#[test]
fn malformed_and_wrong_namespace_refs_are_stable_findings() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;

    child_refs_mut(&mut root)[0].uri = format!("runx:workflow_receipt:{}", child.id).into();
    refresh_proof_digest_and_signature(&mut root)?;
    let verification = verify_tree(&root, std::slice::from_ref(&child));
    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );

    child_refs_mut(&mut root)[0].uri = format!(":{}", child.id).into();
    refresh_proof_digest_and_signature(&mut root)?;
    let verification = verify_tree(&root, &[child]);
    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn suffix_only_refs_are_malformed_not_aliases() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;
    let suffix = child.id.trim_start_matches("sha256:").to_owned();
    child_refs_mut(&mut root)[0].uri = suffix.into();
    refresh_proof_digest_and_signature(&mut root)?;

    let verification = verify_tree(&root, &[child]);

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn duplicate_ids_make_resolution_ambiguous() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let child = proof_child("rcpt_child_1")?;
    link_child_digest(&mut root, 0, &child)?;

    let verification = verify_tree(&root, &[child.clone(), child]);

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
    let root = proof_root()?;

    let verification = verify_tree_with(&root, &AmbiguousResolver, ReceiptTreeConfig::default());

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptAmbiguous,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn depth_limit_blocks_hostile_nested_tree() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let grandchild = proof_child("grandchild")?;
    let mut child = proof_child("rcpt_child_1")?;
    child_refs_mut(&mut child).push(child_link(&grandchild));
    refresh_proof_digest_and_signature(&mut child)?;
    link_child_digest(&mut root, 0, &child)?;

    let verification = verify_tree_with(
        &root,
        &FixtureResolver {
            children: &[child, grandchild],
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
fn a_tree_holds_at_most_its_receipt_count() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let first = proof_child("rcpt_child_1")?;
    let second = proof_child("second")?;
    *child_refs_mut(&mut root) = vec![child_link(&first), child_link(&second)];
    refresh_proof_digest_and_signature(&mut root)?;

    let verification = verify_tree_with(
        &root,
        &FixtureResolver {
            children: &[first, second],
        },
        ReceiptTreeConfig {
            max_receipts: 2,
            ..ReceiptTreeConfig::default()
        },
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptCountLimit,
        "lineage.children[1]",
    );
    Ok(())
}

#[test]
fn positive_nested_tree_verifies() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let grandchild = proof_child("grandchild")?;
    let mut child = proof_child("rcpt_child_1")?;
    child_refs_mut(&mut child).push(child_link(&grandchild));
    refresh_proof_digest_and_signature(&mut child)?;
    link_child_digest(&mut root, 0, &child)?;

    let verification = verify_tree(&root, &[child, grandchild]);

    assert!(verification.valid, "{:?}", verification.findings);
    Ok(())
}

#[test]
fn positive_fanout_tree_verifies() -> Result<(), serde_json::Error> {
    let mut root = proof_root()?;
    let first = proof_child("rcpt_child_1")?;
    let second = proof_child("second")?;
    *child_refs_mut(&mut root) = vec![child_link(&first), child_link(&second)];
    refresh_proof_digest_and_signature(&mut root)?;

    let verification = verify_tree(&root, &[first, second]);

    assert!(verification.valid, "{:?}", verification.findings);
    Ok(())
}

/// A tree at the bound is the one a run seals when its one step is a script
/// making the kernel's ceiling of skill calls: the root, its script step,
/// and each call's receipt over its child run's workflow and that run's one
/// step. It verifies through a resolver that reads each receipt only when a
/// ref names it, and the walk never has more of them than the tree is deep.
#[test]
fn a_tree_at_the_bound_verifies_keeping_only_the_path_it_walks()
-> Result<(), Box<dyn std::error::Error>> {
    // Every receipt is the fixture with its own outcome and children, so
    // each is a content address of its own; the fixture's acts and decisions
    // say nothing the walk reads.
    let mut template = proof_child("template")?;
    template.acts.clear();
    template.decisions.clear();
    let addressed = |label: String, children: Vec<Reference>| {
        let mut receipt = template.clone();
        receipt.subject.outcome_digest = Digest::of(label.as_bytes());
        receipt.lineage.children = children;
        address(&mut receipt).map(|()| receipt)
    };
    let mut stored = Vec::new();
    let mut calls = Vec::new();
    for call in 0..MAX_SCRIPT_CALLS {
        let step = addressed(format!("child step {call}"), Vec::new())?;
        let workflow = addressed(format!("child run {call}"), vec![child_link(&step)])?;
        let receipt = addressed(format!("call {call}"), vec![child_link(&workflow)])?;
        calls.push(child_link(&receipt));
        let added = [step, workflow, receipt];
        assert_eq!(added.len(), RECEIPTS_PER_SKILL_CALL);
        stored.extend(added);
    }
    let script = addressed("script step".to_owned(), calls)?;
    let root = addressed("root".to_owned(), vec![child_link(&script)])?;
    stored.push(script);
    assert_eq!(stored.len() + 1, MAX_RECEIPT_TREE_RECEIPTS);
    let resolver = ReadingResolver::storing(stored);

    let verification = verify_tree_trusting_signatures(&root, &resolver);

    assert!(verification.valid, "{:?}", verification.findings.first());
    assert_eq!(resolver.reads.get(), MAX_RECEIPT_TREE_RECEIPTS - 1);
    // The script step, a call, its child run, and that run's step.
    assert_eq!(resolver.most_out.get(), 4);
    assert_eq!(resolver.out(), 0);
    Ok(())
}
