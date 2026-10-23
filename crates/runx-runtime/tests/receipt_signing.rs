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
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV,
    RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV, RUNX_RECEIPT_SIGN_KID_ENV, ReceiptTrust,
    RuntimeReceiptSignatureConfig, RuntimeReceiptSignaturePolicy, RuntimeReceiptSigner,
    RuntimeReceiptSigningError, TrustedReceiptKey, graph_receipt, receipt_verifier, step_receipt,
};
use runx_runtime::{EphemeralValue, InvocationOutput, InvocationStatus, RunxPaths, StepRun};

const CREATED_AT: &str = "2026-05-22T00:00:00Z";
const FIXTURE_KID: &str = "runx-runtime-prod-fixture-key";
const FIXTURE_SEED: [u8; 32] = [0x42; 32];

#[test]
fn hosted_step_receipt_uses_real_ed25519_signature() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let receipt = production_step_receipt(&signer, &verifier)?;

    assert_eq!(receipt.issuer.kid, FIXTURE_KID);
    assert_eq!(
        receipt.issuer.public_key_digest,
        signer.trusted_key().public_key_digest()
    );
    assert!(receipt.signature.value.starts_with("base64:"));
    assert!(!serde_json::to_string(&receipt)?.contains("QkJCQkJC"));
    let verification = verify_receipt_proof(&receipt, &proof_context(&verifier));
    // The decision -> act-id integrity property is now checked inline against
    // `acts[]` (no journal), so a sealed receipt verifies cleanly.
    assert!(
        verification.valid,
        "a sealed receipt must verify cleanly: {:?}",
        verification.findings
    );
    Ok(())
}

#[test]
fn hosted_graph_receipt_commits_immutable_children_and_verifies_tree() -> Result<(), Box<dyn Error>>
{
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let steps = vec![production_step_run(
        "prod_graph",
        "plan",
        &signer,
        &verifier,
    )?];

    let graph = graph_receipt(
        scope("prod_graph"),
        &steps,
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::signing(&signer, &verifier),
    )?;
    let children = steps
        .iter()
        .map(|step| step.receipt.clone())
        .collect::<Vec<_>>();

    assert!(graph.signature.value.starts_with("base64:"));
    assert!(children[0].signature.value.starts_with("base64:"));
    assert!(
        runx_runtime::receipts::tree::validate_runtime_receipt_tree(
            &graph,
            children,
            runx_receipts::ReceiptTreeConfig::default(),
            RuntimeReceiptSignaturePolicy::verifying(&verifier),
        )
        .is_ok()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn local_step_and_graph_receipts_use_real_ed25519() -> Result<(), Box<dyn Error>> {
    let home = tempfile::tempdir()?;
    let env = home_env(home.path());
    let paths = RunxPaths::resolve(&env, home.path());
    let config = RuntimeReceiptSignatureConfig::resolve(&env, &paths)?;
    let step = step_receipt(
        scope("local_graph"),
        "plan",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        config.signature_policy(),
    )?;
    let graph = graph_receipt(
        scope("local_graph"),
        &[step_run("plan", step.clone())],
        CREATED_AT,
        config.signature_policy(),
    )?;

    for receipt in [&step, &graph] {
        assert_eq!(receipt.issuer.issuer_type, ReceiptIssuerType::Local);
        assert!(receipt.issuer.kid.starts_with("local-"));
        assert!(receipt.signature.value.starts_with("base64:"));
    }
    let verifier = receipt_verifier(&env, &paths, ReceiptTrust::HomeAndConfigured)?;
    assert!(
        runx_runtime::validate_runtime_receipt_tree(
            &graph,
            vec![step],
            runx_receipts::ReceiptTreeConfig::default(),
            RuntimeReceiptSignaturePolicy::verifying(&verifier),
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn sealing_fails_closed_without_a_signer_or_against_its_verifier() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);

    let Err(missing_signer) = step_receipt(
        scope("missing"),
        "signer",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::verifying(&verifier),
    ) else {
        return Err("sealing without a signer must fail".into());
    };
    assert!(missing_signer.to_string().contains("requires a signer"));

    let other = Ed25519ReceiptSigner::local(&[0x07; 32])?;
    let other_verifier = fixture_verifier(&other);
    let Err(refused) = step_receipt(
        scope("refused"),
        "verifier",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::signing(&signer, &other_verifier),
    ) else {
        return Err("a seal its verifier refuses must not be kept".into());
    };
    assert!(refused.to_string().contains("did not verify"));
    Ok(())
}

#[test]
fn sealing_rejects_an_issuer_its_verifier_does_not_hold() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let blank_kid = FixedSigner {
        issuer: ReceiptIssuer {
            issuer_type: ReceiptIssuerType::Hosted,
            kid: " ".into(),
            public_key_digest: signer.trusted_key().public_key_digest().into(),
            runx_version: runx_runtime::RUNX_VERSION.into(),
        },
    };
    let wrong_hash = FixedSigner {
        issuer: ReceiptIssuer {
            issuer_type: ReceiptIssuerType::Hosted,
            kid: FIXTURE_KID.into(),
            public_key_digest: format!("sha256:{}", "1".repeat(64)).into(),
            runx_version: runx_runtime::RUNX_VERSION.into(),
        },
    };

    for signer in [&blank_kid, &wrong_hash] {
        let Err(error) = sign_with_fixed_signer(signer, &verifier) else {
            return Err("a seal under an untrusted issuer must fail".into());
        };
        assert!(error.to_string().contains("did not verify"));
    }
    Ok(())
}

#[test]
fn verifier_reports_tamper_findings() -> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let receipt = production_step_receipt(&signer, &verifier)?;

    let mut tampered_body = receipt.clone();
    tampered_body.seal.reason = Some(runx_contracts::SealReason::GraphBlocked);
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
    hash_mismatch.issuer.public_key_digest = format!("sha256:{}", "1".repeat(64)).into();
    let verification = verify_receipt_proof(&hash_mismatch, &proof_context(&verifier));
    assert_finding(
        &verification,
        ReceiptFindingCode::SignatureKeyDigestMismatch,
    );

    let mut missing_verifier = receipt;
    refresh_digest_and_signature(&mut missing_verifier, &signer)?;
    let verification = verify_receipt_proof(&missing_verifier, &ReceiptProofContext::default());
    assert_finding(&verification, ReceiptFindingCode::SignatureVerifierMissing);
    Ok(())
}

#[test]
fn verifier_rejects_a_foreign_signature_value_and_malformed_public_key()
-> Result<(), Box<dyn Error>> {
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let mut receipt = production_step_receipt(&signer, &verifier)?;

    let digest = canonical_receipt_body_digest(&receipt)?;
    receipt.signature.value = format!("hex:{digest}").into();
    let verification = verify_receipt_proof(&receipt, &proof_context(&verifier));
    assert_finding(&verification, ReceiptFindingCode::SignatureMalformed);

    let bad_key = TrustedReceiptKey::new(ReceiptIssuerType::Hosted, FIXTURE_KID, vec![0x99; 31]);
    let bad_verifier = Ed25519ReceiptVerifier::new([bad_key]);
    let verification = verify_receipt_proof(&receipt, &proof_context(&bad_verifier));
    assert_finding(&verification, ReceiptFindingCode::SignatureKeyMalformed);
    Ok(())
}

#[test]
fn configured_signing_requires_a_complete_non_local_identity() -> Result<(), Box<dyn Error>> {
    let home = tempfile::tempdir()?;
    let paths = RunxPaths::resolve(&home_env(home.path()), home.path());

    let env_without_issuer = signing_env(None);
    let error = RuntimeReceiptSignatureConfig::resolve(&env_without_issuer, &paths)
        .err()
        .ok_or("missing issuer type unexpectedly succeeded")?;
    assert!(error.to_string().contains("to be set together"));

    let env_with_local = signing_env(Some("local"));
    let error = RuntimeReceiptSignatureConfig::resolve(&env_with_local, &paths)
        .err()
        .ok_or("a configured local issuer unexpectedly succeeded")?;
    assert!(error.to_string().contains("issuer type is unsupported"));
    assert!(!paths.local_receipt_key().exists());

    let env_with_hosted = signing_env(Some("hosted"));
    let config = RuntimeReceiptSignatureConfig::resolve(&env_with_hosted, &paths)?;
    assert_eq!(config.issuer(), fixture_signer()?.issuer());
    let receipt = step_receipt(
        scope("prod_env"),
        "seal",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        config.signature_policy(),
    )?;
    assert_eq!(receipt.issuer.issuer_type, ReceiptIssuerType::Hosted);
    assert!(!paths.local_receipt_key().exists());
    Ok(())
}

fn home_env(home: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([(
        runx_runtime::RUNX_HOME_ENV.to_owned(),
        home.display().to_string(),
    )])
}

fn step_run(step_id: &str, receipt: Receipt) -> StepRun {
    StepRun {
        step_id: step_id.to_owned(),
        attempt: 1,
        skill: step_id.to_owned(),
        runner: None,
        contract: JsonObject::new(),
        artifact_outputs: Default::default(),
        ephemeral_contract: EphemeralValue::default(),
        outcome: skill_output(InvocationStatus::Success).into(),
        admission_witness: StepAdmissionWitness::local_runtime(step_id, receipt.id.as_str()),
        settlement: None,
        receipt,
        nested_receipts: Vec::new(),
        refusal: None,
    }
}

fn production_step_receipt(
    signer: &Ed25519ReceiptSigner,
    verifier: &Ed25519ReceiptVerifier,
) -> Result<Receipt, Box<dyn Error>> {
    Ok(step_receipt(
        scope("prod_step"),
        "seal",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::signing(signer, verifier),
    )?)
}

fn production_step_run(
    graph_name: &str,
    step_id: &str,
    signer: &Ed25519ReceiptSigner,
    verifier: &Ed25519ReceiptVerifier,
) -> Result<StepRun, Box<dyn Error>> {
    let output = skill_output(InvocationStatus::Success);
    let receipt = step_receipt(
        scope(graph_name),
        step_id,
        1,
        &output,
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::signing(signer, verifier),
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
        refusal: None,
        admission_witness: StepAdmissionWitness::local_runtime(step_id, "receipt"),
        settlement: None,
    })
}

fn fixture_signer() -> Result<Ed25519ReceiptSigner, RuntimeReceiptSigningError> {
    Ed25519ReceiptSigner::from_seed(FIXTURE_KID, ReceiptIssuerType::Hosted, &FIXTURE_SEED)
}

fn fixture_verifier(signer: &Ed25519ReceiptSigner) -> Ed25519ReceiptVerifier {
    Ed25519ReceiptVerifier::new([signer.trusted_key()])
}

fn proof_context(verifier: &Ed25519ReceiptVerifier) -> ReceiptProofContext<'_> {
    ReceiptProofContext {
        signature_verifier: Some(verifier),
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
    InvocationOutput::process(status, stdout, stderr, exit_code, 1)
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
    step_receipt(
        scope("prod_bad_metadata"),
        "seal",
        1,
        &skill_output(InvocationStatus::Success),
        &JsonObject::new(),
        CREATED_AT,
        RuntimeReceiptSignaturePolicy::signing(signer, verifier),
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
