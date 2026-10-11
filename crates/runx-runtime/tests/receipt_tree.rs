// Test oracle: asserting via expect/unwrap is the intended failure mode, so the
// workspace expect/unwrap bans are lifted for this test target.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use runx_contracts::{JsonObject, Receipt, ReceiptIssuer, ReceiptSignature};
use runx_core::state_machine::StepAdmissionWitness;
use runx_receipts::{
    ReceiptFindingCode, ReceiptTreeConfig, SignatureVerificationFailure, SignatureVerifier,
    canonical_receipt_body_digest,
};
use runx_runtime::receipts::tree::{
    validate_runtime_receipt_tree_with_policy, verify_runtime_receipt_tree_with_policy,
};
use runx_runtime::receipts::{RuntimeReceiptSignaturePolicy, graph_receipt, step_receipt};
use runx_runtime::{
    EphemeralValue, InvocationOutput, InvocationStatus, RuntimeReceiptResolver, StepRun,
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
    assert!(validate_runtime_receipt_tree(&root, children, ReceiptTreeConfig::default()).is_ok());
    Ok(())
}

#[test]
fn runtime_tree_rejects_bare_id_child_ref() -> Result<(), Box<dyn std::error::Error>> {
    let (mut root, children) = graph_with_steps("tree_runtime_exact", &["child"])?;
    root.lineage.children[0].uri = children[0].id.clone();
    refresh_local_digest_and_signature(&mut root)?;

    let verification = verify_runtime_receipt_tree(&root, children, ReceiptTreeConfig::default());

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
    let verification = verify_runtime_receipt_tree(&root, children, ReceiptTreeConfig::default());

    assert_eq!(resolver.receipts().len(), 1);
    assert!(verification.valid, "{:?}", verification.findings);
    Ok(())
}

#[test]
fn runtime_resolver_reports_conflicting_same_id_children() -> Result<(), Box<dyn std::error::Error>>
{
    let (root, mut children) = graph_with_steps("tree_runtime_ambiguous", &["child"])?;
    let mut conflicting = children[0].clone();
    conflicting.seal.reason_code = "different_signed_body".into();
    children.push(conflicting);

    let verification = verify_runtime_receipt_tree(&root, children, ReceiptTreeConfig::default());

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

    let verification = verify_runtime_receipt_tree(&root, Vec::new(), ReceiptTreeConfig::default());

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
    )?);

    let verification = verify_runtime_receipt_tree(&root, children, ReceiptTreeConfig::default());

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
    children[0].seal.reason_code = "tampered_child_body".into();

    assert!(runx_receipts::verify_receipt_tree(&root, &children).valid);
    let verification = verify_runtime_receipt_tree(&root, children, ReceiptTreeConfig::default());

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
    alternate.seal.reason_code = "alternate_child_body".into();
    refresh_local_digest_and_signature(&mut alternate)?;

    let verification =
        verify_runtime_receipt_tree(&root, vec![alternate], ReceiptTreeConfig::default());

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
    let verification = verify_runtime_receipt_tree(&root, children, ReceiptTreeConfig::default());

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
        verify_runtime_receipt_tree(&root, children, ReceiptTreeConfig::default()).valid,
        "the parent id+digest edge is sufficient for an immutable receipt DAG"
    );
    Ok(())
}

#[test]
fn production_tree_policy_rejects_local_pseudo_signature_even_with_permissive_verifier()
-> Result<(), Box<dyn std::error::Error>> {
    let (root, children) = graph_with_steps("tree_runtime_prod_pseudo", &["child"])?;
    let verifier = PermissiveProductionVerifier;

    let verification = verify_runtime_receipt_tree_with_policy(
        &root,
        children,
        ReceiptTreeConfig::default(),
        RuntimeReceiptSignaturePolicy::production(&verifier),
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureMalformed,
        "signature.value",
    );
    Ok(())
}

#[test]
fn production_tree_policy_accepts_supplied_non_pseudo_verifier()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut root, mut children) = graph_with_steps("tree_runtime_prod_real", &["plan", "apply"])?;
    resign_for_test_verifier(&mut root)?;
    for child in &mut children {
        resign_for_test_verifier(child)?;
    }
    let verifier = TestProductionVerifier;

    assert!(
        validate_runtime_receipt_tree_with_policy(
            &root,
            children,
            ReceiptTreeConfig::default(),
            RuntimeReceiptSignaturePolicy::production(&verifier),
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn production_tree_policy_without_verifier_fails_closed() -> Result<(), Box<dyn std::error::Error>>
{
    let (root, children) = graph_with_steps("tree_runtime_prod_missing", &["child"])?;

    let verification = verify_runtime_receipt_tree_with_policy(
        &root,
        children,
        ReceiptTreeConfig::default(),
        RuntimeReceiptSignaturePolicy::production_without_verifier(),
    );

    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureVerifierMissing,
        "signature",
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
    let root = graph_receipt(scope(graph_name), &steps, CREATED_AT)?;
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
        admission_witness,
    })
}

fn skill_output(status: InvocationStatus) -> InvocationOutput {
    let (stdout, stderr, exit_code) = match status {
        InvocationStatus::Success => ("ok".to_owned(), String::new(), Some(0)),
        InvocationStatus::Failure => (String::new(), "failed".to_owned(), Some(1)),
    };
    InvocationOutput::process(status, stdout, stderr, exit_code, 1)
}

fn resign_for_test_verifier(receipt: &mut Receipt) -> Result<(), Box<dyn std::error::Error>> {
    let digest = canonical_receipt_body_digest(receipt)?;
    receipt.signature.value = format!("ed25519-test:{digest}").into();
    Ok(())
}

fn refresh_local_digest_and_signature(
    receipt: &mut Receipt,
) -> Result<(), Box<dyn std::error::Error>> {
    let digest = canonical_receipt_body_digest(receipt)?;
    receipt.digest = digest.clone().into();
    receipt.signature.value = format!("sig:{digest}").into();
    Ok(())
}

struct PermissiveProductionVerifier;

impl SignatureVerifier for PermissiveProductionVerifier {
    fn verify(
        &self,
        _issuer: &ReceiptIssuer,
        _signature: &ReceiptSignature,
        _body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        Ok(())
    }
}

struct TestProductionVerifier;

impl SignatureVerifier for TestProductionVerifier {
    fn verify(
        &self,
        _issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        if signature.value == format!("ed25519-test:{body_digest}") {
            Ok(())
        } else {
            Err(SignatureVerificationFailure::SignatureMismatch)
        }
    }
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
