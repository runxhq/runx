use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, UnparsedPublicKey};
use runx_contracts::{ReceiptIssuer, ReceiptIssuerType, ReceiptSignature, sha256_prefixed};
use runx_receipts::{
    ReceiptProofContext, SignatureVerificationFailure, SignatureVerifier,
    verify_receipt_document_verdict,
};
use serde::Deserialize;

const CORPUS_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/receipt-verify");

/// The public keys the corpus's receipts are verified under.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusVerifier {
    keys: Vec<CorpusKey>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusKey {
    issuer_type: ReceiptIssuerType,
    kid: String,
    public_key_base64: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusCase {
    name: String,
    receipt: String,
    expected: String,
    /// The issuer the case's receipt names; none for a document that is not
    /// a receipt.
    issuer_type: Option<ReceiptIssuerType>,
}

#[test]
fn receipt_verify_corpus_replays_through_library_api() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(CORPUS_ROOT);
    let verifier = CorpusEd25519Verifier::from_fixture(&root)?;
    let context = ReceiptProofContext {
        signature_verifier: Some(&verifier),
    };

    for (case_dir, case) in corpus_cases(&root)? {
        let document = fs::read(case_dir.join(&case.receipt))?;
        assert_eq!(
            runx_receipts::decode_receipt(&document)
                .ok()
                .map(|receipt| receipt.issuer.issuer_type),
            case.issuer_type,
            "corpus case {} names another issuer",
            case.name
        );
        let actual = serde_json::to_value(verify_receipt_document_verdict(&document, &context))?;
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

struct CorpusEd25519Verifier {
    keys: Vec<(ReceiptIssuerType, String, Vec<u8>)>,
}

impl CorpusEd25519Verifier {
    fn from_fixture(root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let fixture: CorpusVerifier =
            serde_json::from_str(&fs::read_to_string(root.join("verifier.json"))?)?;
        Ok(Self {
            keys: fixture
                .keys
                .into_iter()
                .map(|key| {
                    STANDARD
                        .decode(key.public_key_base64)
                        .map(|public_key| (key.issuer_type, key.kid, public_key))
                })
                .collect::<Result<_, _>>()?,
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
        let Some((_, _, public_key)) = self.keys.iter().find(|(issuer_type, kid, _)| {
            *issuer_type == issuer.issuer_type && kid == issuer.kid.as_str()
        }) else {
            return Err(SignatureVerificationFailure::MissingKey);
        };
        if issuer.public_key_sha256.as_str() != sha256_prefixed(public_key) {
            return Err(SignatureVerificationFailure::KeyHashMismatch);
        }
        let Some(signature) = signature.value.strip_prefix("base64:") else {
            return Err(SignatureVerificationFailure::MalformedSignature);
        };
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| SignatureVerificationFailure::MalformedSignature)?;
        UnparsedPublicKey::new(&ED25519, public_key)
            .verify(body_digest.as_bytes(), &signature)
            .map_err(|_| SignatureVerificationFailure::SignatureMismatch)
    }
}
