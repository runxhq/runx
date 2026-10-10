use std::error::Error;

use runx_contracts::{
    JsonObject, Receipt, ReceiptIssuer, ReceiptIssuerType, ReceiptSignature, SignatureAlgorithm,
};
use runx_core::state_machine::StepAdmissionWitness;
use runx_receipts::{
    ReceiptFindingCode, ReceiptProofContext, ReceiptVerification, canonical_receipt_body_digest,
    verify_receipt_proof,
};
use runx_runtime::receipts::{
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, ProductionReceiptKey,
    RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_SIGN_KID_ENV, RuntimeReceiptSignatureConfig, RuntimeReceiptSignaturePolicy,
    RuntimeReceiptSigner, RuntimeReceiptSigningError, graph_receipt_with_signature_policy,
    step_receipt_with_signature_policy,
};
use runx_runtime::{EphemeralValue, InvocationOutput, InvocationStatus, StepRun};

const CREATED_AT: &str = "2026-05-22T00:00:00Z";
const FIXTURE_KID: &str = "runx-runtime-prod-fixture-key";
const FIXTURE_SEED: [u8; 32] = [0x42; 32];

#[test]
fn production_step_receipt_uses_real_ed25519_signature() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let receipt = production_step_receipt(&signer, &verifier)?;

    assert_eq!(receipt.issuer.kid, FIXTURE_KID);
    assert_eq!(
        receipt.issuer.public_key_sha256,
        signer.production_key().public_key_sha256()
    );
    assert!(receipt.signature.value.starts_with("base64:"));
    assert!(!receipt.signature.value.starts_with("sig:"));
    assert!(!serde_json::to_string(&receipt)?.contains("QkJCQkJC"));
    let verification = verify_receipt_proof(&receipt, &proof_context(&verifier));
    // The decision -> act-id integrity property is now checked inline against
    // `acts[]` (no journal), so a sealed production receipt verifies cleanly.
    assert!(
        verification.valid,
        "production receipt must verify cleanly: {:?}",
        verification.findings
    );
    Ok(())
}

#[test]
fn production_graph_receipt_commits_immutable_children_and_verifies_tree()
-> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let steps = vec![production_step_run(
        "prod_graph",
        "plan",
        &signer,
        &verifier,
    )?];

    let graph = graph_receipt_with_signature_policy(
        scope("prod_graph"),
        &steps,
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::production_signing(&signer, &verifier),
    )?;
    let children = steps
        .iter()
        .map(|step| step.receipt.clone())
        .collect::<Vec<_>>();

    assert!(graph.signature.value.starts_with("base64:"));
    assert!(children[0].signature.value.starts_with("base64:"));
    assert!(
        children[0]
            .lineage
            .as_ref()
            .and_then(|lineage| lineage.parent.as_ref())
            .is_none(),
        "content-addressed children must remain reusable across graph parents"
    );
    assert!(
        runx_runtime::receipts::tree::validate_runtime_receipt_tree_with_policy(
            &graph,
            children,
            runx_receipts::ReceiptTreeConfig::default(),
            RuntimeReceiptSignaturePolicy::production(&verifier),
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn production_sealing_fails_closed_without_signer_or_verifier() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);

    let Err(missing_signer) = step_receipt_with_signature_policy(
        scope("prod_missing"),
        "signer",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::production(&verifier),
    ) else {
        return Err("production sealing without signer must fail".into());
    };
    assert!(missing_signer.to_string().contains("requires a signer"));

    let Err(missing_verifier) = step_receipt_with_signature_policy(
        scope("prod_missing"),
        "verifier",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::production_signing_without_verifier(&signer),
    ) else {
        return Err("production sealing without verifier must fail".into());
    };
    assert!(missing_verifier.to_string().contains("requires a verifier"));
    Ok(())
}

#[test]
fn production_sealing_rejects_missing_issuer_metadata() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let missing_kid = FixedSigner {
        issuer: ReceiptIssuer {
            issuer_type: ReceiptIssuerType::Hosted,
            kid: " ".into(),
            public_key_sha256: signer.production_key().public_key_sha256().into(),
        },
    };
    let missing_hash = FixedSigner {
        issuer: ReceiptIssuer {
            issuer_type: ReceiptIssuerType::Hosted,
            kid: FIXTURE_KID.into(),
            public_key_sha256: " ".into(),
        },
    };

    let Err(missing_kid_error) = sign_with_fixed_signer(&missing_kid, &verifier) else {
        return Err("production sealing without kid must fail".into());
    };
    assert!(missing_kid_error.to_string().contains("key id is missing"));

    let Err(missing_hash_error) = sign_with_fixed_signer(&missing_hash, &verifier) else {
        return Err("production sealing without public key hash must fail".into());
    };
    assert!(
        missing_hash_error
            .to_string()
            .contains("public key hash is missing")
    );
    Ok(())
}

#[test]
fn production_verifier_reports_tamper_findings() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let receipt = production_step_receipt(&signer, &verifier)?;

    let mut tampered_body = receipt.clone();
    tampered_body.seal.summary = "tampered body".into();
    let verification = verify_receipt_proof(&tampered_body, &proof_context(&verifier));
    assert_finding(&verification, ReceiptFindingCode::SealDigestMismatch);
    assert_finding(&verification, ReceiptFindingCode::SignatureInvalid);

    let mut tampered_seal = receipt.clone();
    tampered_seal.digest = format!("sha256:{}", "0".repeat(64)).into();
    let verification = verify_receipt_proof(&tampered_seal, &proof_context(&verifier));
    assert_finding(&verification, ReceiptFindingCode::SealDigestMismatch);

    let mut malformed_signature = receipt.clone();
    malformed_signature.signature.value = "base64:!".into();
    let verification = verify_receipt_proof(&malformed_signature, &proof_context(&verifier));
    assert_finding(&verification, ReceiptFindingCode::SignatureMalformed);

    let mut missing_key = receipt.clone();
    missing_key.issuer.kid = "missing-key".into();
    let verification = verify_receipt_proof(&missing_key, &proof_context(&verifier));
    assert_finding(&verification, ReceiptFindingCode::SignatureKeyMissing);

    let mut hash_mismatch = receipt.clone();
    hash_mismatch.issuer.public_key_sha256 = format!("sha256:{}", "1".repeat(64)).into();
    let verification = verify_receipt_proof(&hash_mismatch, &proof_context(&verifier));
    assert_finding(&verification, ReceiptFindingCode::SignatureKeyHashMismatch);

    let mut missing_verifier = receipt;
    refresh_digest_and_signature(&mut missing_verifier, &signer)?;
    let verification = verify_receipt_proof(&missing_verifier, &ReceiptProofContext::default());
    assert_finding(&verification, ReceiptFindingCode::SignatureVerifierMissing);
    Ok(())
}

#[test]
fn production_verifier_rejects_pseudo_and_malformed_public_key() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let mut receipt = production_step_receipt(&signer, &verifier)?;

    let digest = canonical_receipt_body_digest(&receipt)?;
    receipt.signature.value = format!("sig:{digest}").into();
    let verification = verify_receipt_proof(&receipt, &proof_context(&verifier));
    assert_finding(&verification, ReceiptFindingCode::SignatureMalformed);

    let bad_key = ProductionReceiptKey::new(FIXTURE_KID, vec![0x99; 31]);
    let bad_verifier = Ed25519ReceiptVerifier::new([bad_key]);
    let verification = verify_receipt_proof(&receipt, &proof_context(&bad_verifier));
    assert_finding(&verification, ReceiptFindingCode::SignatureKeyMalformed);
    Ok(())
}

#[test]
fn production_signing_env_requires_non_local_issuer_type() -> Result<(), Box<dyn Error>> {
    let empty_env = std::collections::BTreeMap::new();
    let error = RuntimeReceiptSignatureConfig::from_env(&empty_env)
        .err()
        .ok_or("missing signing env unexpectedly succeeded")?;
    assert!(
        error
            .to_string()
            .contains("governed runtime receipt signing")
    );

    let env_without_issuer = signing_env(None);
    let error = RuntimeReceiptSignatureConfig::from_env(&env_without_issuer)
        .err()
        .ok_or("missing issuer type unexpectedly succeeded")?;
    assert!(error.to_string().contains("issuer type is missing"));

    let env_with_local = signing_env(Some("local"));
    let error = RuntimeReceiptSignatureConfig::from_env(&env_with_local)
        .err()
        .ok_or("local production issuer unexpectedly succeeded")?;
    assert!(error.to_string().contains("issuer type is unsupported"));

    let env_with_hosted = signing_env(Some("hosted"));
    let config = RuntimeReceiptSignatureConfig::from_env(&env_with_hosted)?;
    let published_key = config
        .production_key_for_kid(FIXTURE_KID)
        .ok_or("production receipt key should resolve by kid")?;
    assert_eq!(
        published_key.public_key_sha256(),
        fixture_signer()?.production_key().public_key_sha256()
    );
    let receipt = step_receipt_with_signature_policy(
        scope("prod_env"),
        "seal",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        config.signature_policy(),
    )?;
    assert_eq!(receipt.issuer.issuer_type, ReceiptIssuerType::Hosted);
    Ok(())
}

fn production_step_receipt(
    signer: &Ed25519ReceiptSigner,
    verifier: &Ed25519ReceiptVerifier,
) -> Result<Receipt, Box<dyn Error>> {
    Ok(step_receipt_with_signature_policy(
        scope("prod_step"),
        "seal",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::production_signing(signer, verifier),
    )?)
}

fn production_step_run(
    graph_name: &str,
    step_id: &str,
    signer: &Ed25519ReceiptSigner,
    verifier: &Ed25519ReceiptVerifier,
) -> Result<StepRun, Box<dyn Error>> {
    let output = skill_output(InvocationStatus::Success);
    let receipt = step_receipt_with_signature_policy(
        scope(graph_name),
        step_id,
        1,
        &output,
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::production_signing(signer, verifier),
    )?;
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
        admission_witness: StepAdmissionWitness::local_runtime(step_id, "receipt"),
    })
}

fn fixture_signer() -> Result<Ed25519ReceiptSigner, RuntimeReceiptSigningError> {
    Ed25519ReceiptSigner::from_seed(FIXTURE_KID, ReceiptIssuerType::Hosted, &FIXTURE_SEED)
}

fn fixture_verifier(signer: &Ed25519ReceiptSigner) -> Ed25519ReceiptVerifier {
    Ed25519ReceiptVerifier::new([signer.production_key()])
}

fn proof_context(verifier: &Ed25519ReceiptVerifier) -> ReceiptProofContext<'_> {
    ReceiptProofContext {
        signature_verifier: Some(verifier),
        authority_verified: false,
        external_attestations_verified: true,
        verified_redaction_refs: Default::default(),
        verified_hash_commitments: Default::default(),
    }
}

fn signing_env(issuer_type: Option<&str>) -> std::collections::BTreeMap<String, String> {
    let mut env = [
        (RUNX_RECEIPT_SIGN_KID_ENV.to_owned(), FIXTURE_KID.to_owned()),
        (
            RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV.to_owned(),
            "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=".to_owned(),
        ),
    ]
    .into_iter()
    .collect::<std::collections::BTreeMap<_, _>>();
    if let Some(issuer_type) = issuer_type {
        env.insert(
            RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV.to_owned(),
            issuer_type.to_owned(),
        );
    }
    env
}

fn skill_output(status: InvocationStatus) -> InvocationOutput {
    let (stdout, stderr, exit_code) = match status {
        InvocationStatus::Success => ("ok".to_owned(), String::new(), Some(0)),
        InvocationStatus::Failure => (String::new(), "failed".to_owned(), Some(1)),
    };
    InvocationOutput::process(status, stdout, stderr, exit_code, 1, JsonObject::new())
}

fn refresh_digest_and_signature(
    receipt: &mut Receipt,
    signer: &Ed25519ReceiptSigner,
) -> Result<(), Box<dyn Error>> {
    let digest = canonical_receipt_body_digest(receipt)?;
    receipt.digest = digest.clone().into();
    receipt.signature = signer.sign_receipt_body(&digest)?;
    Ok(())
}

fn sign_with_fixed_signer(
    signer: &FixedSigner,
    verifier: &Ed25519ReceiptVerifier,
) -> Result<Receipt, runx_runtime::RuntimeError> {
    step_receipt_with_signature_policy(
        scope("prod_bad_metadata"),
        "seal",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::production_signing(signer, verifier),
    )
}

struct FixedSigner {
    issuer: ReceiptIssuer,
}

impl RuntimeReceiptSigner for FixedSigner {
    fn issuer(&self) -> ReceiptIssuer {
        self.issuer.clone()
    }

    fn sign_receipt_body(
        &self,
        _body_digest: &str,
    ) -> Result<ReceiptSignature, RuntimeReceiptSigningError> {
        Ok(ReceiptSignature {
            alg: SignatureAlgorithm::Ed25519,
            value: "base64:fixed-signature".into(),
        })
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

fn scope(graph_name: &str) -> runx_runtime::receipts::RunScope<'_> {
    runx_runtime::receipts::RunScope {
        run_id: "run_test",
        graph_name,
    }
}
