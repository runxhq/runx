//! Regenerates the strict V1 receipt verification corpus after an in-place
//! contract change. The fixture-only Ed25519 seed is deterministic and has no
//! production authority.

#![allow(clippy::print_stdout, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use runx_contracts::{
    Receipt, ReceiptIssuer, ReceiptIssuerType, ReceiptSignature, SignatureAlgorithm,
    sha256_prefixed,
};
use runx_receipts::{
    ReceiptProofContext, ReceiptVerifySignatureMode, SignatureVerificationFailure,
    SignatureVerifier, canonical_receipt_body_digest, content_addressed_receipt_id,
    verify_receipt_document_verdict,
};

const FIXTURE_SEED: [u8; 32] = [0x52; 32];
const FIXTURE_KID: &str = "runx-cli-verify-fixture-key";

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/receipt-verify");
    let key_pair = Ed25519KeyPair::from_seed_unchecked(&FIXTURE_SEED).unwrap();
    let public_key = key_pair.public_key().as_ref().to_vec();
    write_json(
        &root.join("verifier.json"),
        &serde_json::json!({
            "kid": FIXTURE_KID,
            "public_key_base64": STANDARD.encode(&public_key),
        }),
    );

    let valid_path = root.join("valid-production/receipt.json");
    let mut valid = read_receipt(&valid_path);
    seal_production(&mut valid, &key_pair);
    write_receipt(&valid_path, &valid);

    let mut tampered_body = valid.clone();
    tampered_body.seal.reason_code = "tampered_receipt_body".into();
    write_receipt(&root.join("tampered-body/receipt.json"), &tampered_body);

    let mut tampered_signature = valid.clone();
    tampered_signature.signature.value =
        format!("base64:{}", URL_SAFE_NO_PAD.encode([0_u8; 64])).into();
    write_receipt(
        &root.join("tampered-signature/receipt.json"),
        &tampered_signature,
    );

    // Keys past the Basic Multilingual Plane and a small float: an independent
    // verifier must order keys by code point and write numbers as the
    // canonical writer does to reproduce this receipt's id and digest.
    let mut wide = serde_json::to_value(&valid).unwrap();
    wide["authority"]["terms"] = serde_json::json!([{
        "term_id": "term_wide",
        "principal_ref": { "type": "principal", "uri": "runx:principal:local_runtime" },
        "resource_ref": { "type": "repository", "uri": "runx:repository:fixture" },
        "resource_family": "workspace",
        "verbs": ["read"],
        "bounds": {},
        "conditions": [{
            "condition_id": "budget",
            "predicate": "within_budget",
            "parameters": { "\u{E000}": 1, "\u{10000}": 0.000001234, "limit": 1.5e20 },
        }],
        "issued_by_ref": { "type": "principal", "uri": "runx:principal:local_runtime" },
    }]);
    let mut wide: Receipt = serde_json::from_value(wide).unwrap();
    seal_production(&mut wide, &key_pair);
    write_receipt(&root.join("valid-wide-keys/receipt.json"), &wide);

    let mut unknown_kid = valid.clone();
    unknown_kid.issuer.kid = "unknown-fixture-key".into();
    write_receipt(&root.join("unknown-kid/receipt.json"), &unknown_kid);

    let local_path = root.join("broken-lineage-reference/receipt.json");
    let mut local = read_receipt(&local_path);
    seal_local(&mut local);
    write_receipt(&local_path, &local);

    write_expected_verdicts(&root, &public_key);
    println!("regenerated strict receipt verify corpus");
}

fn read_receipt(path: &Path) -> Receipt {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn seal_local(receipt: &mut Receipt) {
    receipt.issuer.issuer_type = ReceiptIssuerType::Local;
    receipt.signature = ReceiptSignature {
        alg: SignatureAlgorithm::Ed25519,
        value: "sig:pending".into(),
    };
    receipt.id = content_addressed_receipt_id(receipt).unwrap().into();
    let digest = canonical_receipt_body_digest(receipt).unwrap();
    receipt.digest = digest.clone().into();
    receipt.signature.value = format!("sig:{digest}").into();
}

fn seal_production(receipt: &mut Receipt, key_pair: &Ed25519KeyPair) {
    let public_key = key_pair.public_key().as_ref();
    receipt.issuer = ReceiptIssuer {
        issuer_type: ReceiptIssuerType::Hosted,
        kid: FIXTURE_KID.into(),
        public_key_sha256: sha256_prefixed(public_key).into(),
    };
    receipt.signature = ReceiptSignature {
        alg: SignatureAlgorithm::Ed25519,
        value: "base64:pending".into(),
    };
    receipt.id = content_addressed_receipt_id(receipt).unwrap().into();
    let digest = canonical_receipt_body_digest(receipt).unwrap();
    receipt.digest = digest.clone().into();
    receipt.signature.value = format!(
        "base64:{}",
        URL_SAFE_NO_PAD.encode(key_pair.sign(digest.as_bytes()).as_ref())
    )
    .into();
}

fn write_expected_verdicts(root: &Path, public_key: &[u8]) {
    let production = CorpusVerifier {
        kid: FIXTURE_KID,
        public_key,
    };
    let local = LocalVerifier;
    for (name, mode) in [
        ("valid-production", ReceiptVerifySignatureMode::Production),
        ("valid-wide-keys", ReceiptVerifySignatureMode::Production),
        ("tampered-body", ReceiptVerifySignatureMode::Production),
        ("tampered-signature", ReceiptVerifySignatureMode::Production),
        ("unknown-kid", ReceiptVerifySignatureMode::Production),
        (
            "broken-lineage-reference",
            ReceiptVerifySignatureMode::LocalDevelopment,
        ),
        (
            "malformed-json",
            ReceiptVerifySignatureMode::LocalDevelopment,
        ),
    ] {
        let bytes = fs::read(root.join(name).join("receipt.json")).unwrap();
        let context = ReceiptProofContext {
            signature_verifier: Some(if mode == ReceiptVerifySignatureMode::Production {
                &production
            } else {
                &local
            }),
        };
        let verdict = verify_receipt_document_verdict(&bytes, &context, mode);
        write_json(
            &root.join(name).join("expected.json"),
            &serde_json::to_value(verdict).unwrap(),
        );
    }
}

fn write_receipt(path: &Path, receipt: &Receipt) {
    write_json(path, &serde_json::to_value(receipt).unwrap());
}

fn write_json(path: &Path, value: &serde_json::Value) {
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
    )
    .unwrap();
}

struct CorpusVerifier<'a> {
    kid: &'a str,
    public_key: &'a [u8],
}

impl SignatureVerifier for CorpusVerifier<'_> {
    fn verify(
        &self,
        issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        if issuer.kid.as_str() != self.kid {
            return Err(SignatureVerificationFailure::MissingKey);
        }
        if issuer.public_key_sha256.as_str() != sha256_prefixed(self.public_key) {
            return Err(SignatureVerificationFailure::KeyHashMismatch);
        }
        let encoded = signature
            .value
            .strip_prefix("base64:")
            .ok_or(SignatureVerificationFailure::MalformedSignature)?;
        let signature = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| SignatureVerificationFailure::MalformedSignature)?;
        UnparsedPublicKey::new(&ED25519, self.public_key)
            .verify(body_digest.as_bytes(), &signature)
            .map_err(|_| SignatureVerificationFailure::SignatureMismatch)
    }
}

struct LocalVerifier;

impl SignatureVerifier for LocalVerifier {
    fn verify(
        &self,
        _issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        if signature.value == format!("sig:{body_digest}") {
            Ok(())
        } else {
            Err(SignatureVerificationFailure::SignatureMismatch)
        }
    }
}
