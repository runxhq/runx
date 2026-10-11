use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, UnparsedPublicKey};
use runx_contracts::{ReceiptIssuer, ReceiptSignature, sha256_prefixed};
use runx_receipts::{
    ReceiptProofContext, ReceiptVerifySignatureMode, SignatureVerificationFailure,
    SignatureVerifier, verify_receipt_document_verdict,
};
use serde::Deserialize;

const CORPUS_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/receipt-verify");

#[derive(Debug, Deserialize)]
struct CorpusVerifier {
    kid: String,
    public_key_base64: String,
}

#[derive(Debug, Deserialize)]
struct CorpusCase {
    name: String,
    receipt: String,
    expected: String,
    signature_mode: String,
}

#[test]
fn receipt_verify_corpus_replays_through_library_api() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(CORPUS_ROOT);
    let production_verifier = CorpusEd25519Verifier::from_fixture(&root)?;
    let local_verifier = LocalDevelopmentReceiptVerifier;

    for (case_dir, case) in corpus_cases(&root)? {
        let document = fs::read(case_dir.join(&case.receipt))?;
        let actual = if case.signature_mode == "production" {
            let context = proof_context(&production_verifier);
            serde_json::to_value(verify_receipt_document_verdict(
                &document,
                &context,
                ReceiptVerifySignatureMode::Production,
            ))?
        } else {
            let context = proof_context(&local_verifier);
            serde_json::to_value(verify_receipt_document_verdict(
                &document,
                &context,
                ReceiptVerifySignatureMode::LocalDevelopment,
            ))?
        };
        let expected: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(case_dir.join(&case.expected))?)?;

        assert_eq!(actual, expected, "corpus case {} drifted", case.name);
    }
    Ok(())
}

fn corpus_cases(root: &Path) -> Result<Vec<(PathBuf, CorpusCase)>, Box<dyn std::error::Error>> {
    let mut cases = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let case_path = path.join("case.json");
        if !case_path.exists() {
            continue;
        }
        let case: CorpusCase = serde_json::from_str(&fs::read_to_string(case_path)?)?;
        cases.push((path, case));
    }
    cases.sort_by(|left, right| left.1.name.cmp(&right.1.name));
    Ok(cases)
}

fn proof_context<'a>(verifier: &'a dyn SignatureVerifier) -> ReceiptProofContext<'a> {
    ReceiptProofContext {
        signature_verifier: Some(verifier),
    }
}

struct CorpusEd25519Verifier {
    kid: String,
    public_key: Vec<u8>,
}

impl CorpusEd25519Verifier {
    fn from_fixture(root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let fixture: CorpusVerifier =
            serde_json::from_str(&fs::read_to_string(root.join("verifier.json"))?)?;
        Ok(Self {
            kid: fixture.kid,
            public_key: STANDARD.decode(fixture.public_key_base64)?,
        })
    }
}

impl SignatureVerifier for CorpusEd25519Verifier {
    fn verify(
        &self,
        issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        if issuer.kid.as_str() != self.kid {
            return Err(SignatureVerificationFailure::MissingKey);
        }
        if issuer.public_key_sha256.as_str() != sha256_prefixed(&self.public_key) {
            return Err(SignatureVerificationFailure::KeyHashMismatch);
        }
        let Some(signature) = signature.value.strip_prefix("base64:") else {
            return Err(SignatureVerificationFailure::MalformedSignature);
        };
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| SignatureVerificationFailure::MalformedSignature)?;
        UnparsedPublicKey::new(&ED25519, &self.public_key)
            .verify(body_digest.as_bytes(), &signature)
            .map_err(|_| SignatureVerificationFailure::SignatureMismatch)
    }
}

struct LocalDevelopmentReceiptVerifier;

impl SignatureVerifier for LocalDevelopmentReceiptVerifier {
    fn verify(
        &self,
        _issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        if !signature.value.starts_with("sig:sha256:") {
            return Err(SignatureVerificationFailure::MalformedSignature);
        }
        if signature.value == format!("sig:{body_digest}") {
            Ok(())
        } else {
            Err(SignatureVerificationFailure::SignatureMismatch)
        }
    }
}
