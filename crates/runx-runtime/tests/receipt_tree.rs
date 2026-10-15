// Test oracle: asserting via expect/unwrap is the intended failure mode, so the
// workspace expect/unwrap bans are lifted for this test target.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use runx_contracts::{JsonObject, Receipt};
use runx_core::state_machine::StepAdmissionWitness;
use runx_receipts::{ReceiptFindingCode, ReceiptTreeConfig, canonical_receipt_body_digest};
use runx_runtime::receipts::{RuntimeReceiptSignaturePolicy, graph_receipt, step_receipt};
use runx_runtime::{
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, EphemeralValue, FIXTURE_RECEIPT_SEED,
    InvocationOutput, InvocationStatus, RuntimeReceiptResolver, RuntimeReceiptSigner, StepRun,
    validate_runtime_receipt_tree, verify_runtime_receipt_tree,
};

const CREATED_AT: &str = "2026-05-18T00:00:00Z";

#[test]
fn runtime_resolver_verifies_graph_receipt_with_children() -> Result<(), Box<dyn std::error::Error>>
{
    let (root, children) = graph_with_steps("tree_runtime_graph", &["plan", "apply"])?;
    let resolver = RuntimeReceiptResolver::new(children.clone());

    assert_eq!(resolver.receipts().len(), 2);
    assert!(
        runx_receipts::validate_receipt_tree_with_resolver(
            &root,
            &resolver,
            ReceiptTreeConfig::default()
        )
        .is_ok()
    );
    assert!(
        validate_runtime_receipt_tree(
            &root,
            children,
            ReceiptTreeConfig::default(),
            crate::support::fixture_policy()?
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn runtime_tree_rejects_bare_id_child_ref() -> Result<(), Box<dyn std::error::Error>> {
    let (mut root, children) = graph_with_steps("tree_runtime_exact", &["child"])?;
    root.lineage.children[0].uri = children[0].id.clone();
    refresh_local_digest_and_signature(&mut root)?;

    let verification = verify_runtime_receipt_tree(
        &root,
        children,
        ReceiptTreeConfig::default(),
        crate::support::fixture_policy()?,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptRefMalformed,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn runtime_resolver_reuses_identical_content_addressed_children()
-> Result<(), Box<dyn std::error::Error>> {
    let (root, mut children) = graph_with_steps("tree_runtime_ambiguous", &["child"])?;
    children.push(children[0].clone());

    let resolver = RuntimeReceiptResolver::new(children.clone());
    let verification = verify_runtime_receipt_tree(
        &root,
        children,
        ReceiptTreeConfig::default(),
        crate::support::fixture_policy()?,
    );

    assert_eq!(resolver.receipts().len(), 1);
    assert!(verification.valid, "{:?}", verification.findings);
    Ok(())
}

#[test]
fn runtime_resolver_reports_conflicting_same_id_children() -> Result<(), Box<dyn std::error::Error>>
{
    let (root, mut children) = graph_with_steps("tree_runtime_ambiguous", &["child"])?;
    let mut conflicting = children[0].clone();
    conflicting.seal.reason = Some(runx_contracts::SealReason::GraphBlocked);
    children.push(conflicting);

    let verification = verify_runtime_receipt_tree(
        &root,
        children,
        ReceiptTreeConfig::default(),
        crate::support::fixture_policy()?,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::DuplicateChildReceipt,
        "runtime_receipts[1].id",
    );
    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptAmbiguous,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn runtime_tree_rejects_missing_child_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let (root, _children) = graph_with_steps("tree_runtime_missing_child", &["child"])?;

    let verification = verify_runtime_receipt_tree(
        &root,
        Vec::new(),
        ReceiptTreeConfig::default(),
        crate::support::fixture_policy()?,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptMissing,
        "lineage.children[0]",
    );
    Ok(())
}

#[test]
fn runtime_tree_rejects_extra_child_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let (root, mut children) = graph_with_steps("tree_runtime_extra_child", &["child"])?;
    children.push(step_receipt(
        scope("tree_runtime_extra_child"),
        "orphan",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        crate::support::fixture_policy()?,
    )?);

    let verification = verify_runtime_receipt_tree(
        &root,
        children,
        ReceiptTreeConfig::default(),
        crate::support::fixture_policy()?,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::OrphanChildReceipt,
        "runtime_receipts[1].id",
    );
    Ok(())
}

#[test]
fn runtime_tree_rejects_structurally_valid_child_proof_tamper()
-> Result<(), Box<dyn std::error::Error>> {
    let (root, mut children) = graph_with_steps("tree_runtime_child_tamper", &["child"])?;
    children[0].seal.reason = Some(runx_contracts::SealReason::GraphBlocked);

    assert!(runx_receipts::verify_receipt_tree(&root, &children).valid);
    let verification = verify_runtime_receipt_tree(
        &root,
        children,
        ReceiptTreeConfig::default(),
        crate::support::fixture_policy()?,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::SealDigestMismatch,
        "runtime_receipts[0].digest",
    );
    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureInvalid,
        "runtime_receipts[0].signature.value",
    );
    Ok(())
}

#[test]
fn runtime_tree_rejects_valid_alternate_child_with_same_id()
-> Result<(), Box<dyn std::error::Error>> {
    let (root, children) = graph_with_steps("tree_runtime_child_digest", &["child"])?;
    let mut alternate = children[0].clone();
    alternate.seal.reason = Some(runx_contracts::SealReason::GraphBlocked);
    refresh_local_digest_and_signature(&mut alternate)?;

    let verification = verify_runtime_receipt_tree(
        &root,
        vec![alternate],
        ReceiptTreeConfig::default(),
        crate::support::fixture_policy()?,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptDigestMismatch,
        "runtime_receipts[0].locator",
    );
    Ok(())
}

#[test]
fn runtime_tree_rejects_child_ref_without_digest_locator() -> Result<(), Box<dyn std::error::Error>>
{
    let (mut root, children) = graph_with_steps("tree_runtime_missing_child_digest", &["child"])?;
    root.lineage.children[0].locator = None;
    refresh_local_digest_and_signature(&mut root)?;

    assert!(runx_receipts::verify_receipt_tree(&root, &children).valid);
    let verification = verify_runtime_receipt_tree(
        &root,
        children,
        ReceiptTreeConfig::default(),
        crate::support::fixture_policy()?,
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::ChildReceiptDigestMismatch,
        "runtime_receipts[0].locator",
    );
    Ok(())
}

#[test]
fn runtime_tree_accepts_reusable_child_without_parent_link()
-> Result<(), Box<dyn std::error::Error>> {
    let (root, children) = graph_with_steps("tree_runtime_reusable_child", &["child"])?;

    assert!(runx_receipts::verify_receipt_tree(&root, &children).valid);
    assert!(
        verify_runtime_receipt_tree(
            &root,
            children,
            ReceiptTreeConfig::default(),
            crate::support::fixture_policy()?
        )
        .valid,
        "the parent id+digest edge is sufficient for an immutable receipt DAG"
    );
    Ok(())
}

#[test]
fn tree_rejects_another_homes_key() -> Result<(), Box<dyn std::error::Error>> {
    let (root, children) = graph_with_steps("tree_runtime_other_home", &["child"])?;
    let other_home = Ed25519ReceiptSigner::local(&[0x07; 32])?;
    let verifier = Ed25519ReceiptVerifier::new([other_home.trusted_key()]);

    let verification = verify_runtime_receipt_tree(
        &root,
        children,
        ReceiptTreeConfig::default(),
        RuntimeReceiptSignaturePolicy::verifying(&verifier),
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureKeyMissing,
        "issuer",
    );
    Ok(())
}

#[test]
fn tree_verifies_under_the_key_that_sealed_it() -> Result<(), Box<dyn std::error::Error>> {
    let (root, children) = graph_with_steps("tree_runtime_sealing_key", &["plan", "apply"])?;

    assert!(
        validate_runtime_receipt_tree(
            &root,
            children,
            ReceiptTreeConfig::default(),
            RuntimeReceiptSignaturePolicy::verifying(
                crate::support::fixture_signature()?.verifier()
            ),
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn tree_with_no_trusted_key_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let (root, children) = graph_with_steps("tree_runtime_no_trust", &["child"])?;
    let verifier = Ed25519ReceiptVerifier::default();

    let verification = verify_runtime_receipt_tree(
        &root,
        children,
        ReceiptTreeConfig::default(),
        RuntimeReceiptSignaturePolicy::verifying(&verifier),
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureKeyMissing,
        "issuer",
    );
    Ok(())
}

fn graph_with_steps(
    graph_name: &str,
    step_ids: &[&str],
) -> Result<(Receipt, Vec<Receipt>), Box<dyn std::error::Error>> {
    let steps = step_ids
        .iter()
        .map(|step_id| step_run(graph_name, step_id, InvocationStatus::Success))
        .collect::<Result<Vec<_>, _>>()?;
    let root = graph_receipt(
        scope(graph_name),
        &steps,
        CREATED_AT,
        crate::support::fixture_policy()?,
    )?;
    Ok((root, child_receipts(&steps)))
}

fn child_receipts(steps: &[StepRun]) -> Vec<Receipt> {
    steps.iter().map(|step| step.receipt.clone()).collect()
}

fn step_run(
    graph_name: &str,
    step_id: &str,
    status: InvocationStatus,
) -> Result<StepRun, Box<dyn std::error::Error>> {
    let output = skill_output(status);
    let receipt = step_receipt(
        scope(graph_name),
        step_id,
        1,
        &output,
        &JsonObject::new(),
        CREATED_AT,
        crate::support::fixture_policy()?,
    )?;
    let admission_witness = StepAdmissionWitness::local_runtime(step_id, receipt.id.as_str());
    Ok(StepRun {
        step_id: step_id.to_owned(),
        attempt: 1,
        skill: step_id.to_owned(),
        runner: None,
        contract: JsonObject::new(),
        artifact_outputs: Default::default(),
        ephemeral_contract: EphemeralValue::default(),
        outcome: output.into(),
        receipt,
        nested_receipts: Vec::new(),
        refusal: None,
        admission_witness,
        settlement: None,
    })
}

fn skill_output(status: InvocationStatus) -> InvocationOutput {
    let (stdout, stderr, exit_code) = match status {
        InvocationStatus::Success => ("ok".to_owned(), String::new(), Some(0)),
        InvocationStatus::Failure => (String::new(), "failed".to_owned(), Some(1)),
    };
    InvocationOutput::process(status, stdout, stderr, exit_code, 1)
}

/// Re-sign the receipt's body under the fixture key and the id it already
/// carries, as a forger reusing another receipt's id would.
fn refresh_local_digest_and_signature(
    receipt: &mut Receipt,
) -> Result<(), Box<dyn std::error::Error>> {
    let digest = canonical_receipt_body_digest(receipt)?;
    receipt.digest = digest.clone().into();
    receipt.signature =
        Ed25519ReceiptSigner::local(&FIXTURE_RECEIPT_SEED)?.sign_receipt_body(&digest)?;
    Ok(())
}

fn assert_finding(
    verification: &runx_receipts::ReceiptVerification,
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

fn scope(graph_name: &str) -> runx_runtime::receipts::RunScope<'_> {
    runx_runtime::receipts::RunScope {
        run_id: "run_test",
        graph_name,
    }
}
