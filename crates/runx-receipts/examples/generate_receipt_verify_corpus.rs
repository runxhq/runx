//! Regenerates the strict V1 receipt verification corpus after an in-place
//! contract change. The fixture-only Ed25519 seed is deterministic and has no
//! production authority.

#![allow(clippy::print_stdout, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use runx_contracts::{Receipt, ReceiptIssuerType};
use runx_receipts::{ReceiptProofContext, verify_receipt_document_verdict};

#[path = "../tests/fixture_key.rs"]
mod fixture_key;

use fixture_key::{FixtureKey, FixtureKeys};

/// The corpus's own key, apart from the harness's: the corpus pins the
/// verifier, not any one runtime identity.
const CORPUS_SEED: [u8; 32] = [0x52; 32];
const HOSTED_KID: &str = "runx-cli-verify-fixture-key";

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/receipt-verify");
    let hosted = FixtureKey::new(CORPUS_SEED, ReceiptIssuerType::Hosted, HOSTED_KID);
    let local = FixtureKey::local(CORPUS_SEED);
    let keys = [&hosted, &local].map(|key| {
        serde_json::json!({
            "issuer_type": key.issuer().issuer_type,
            "kid": key.kid(),
            "public_key_base64": STANDARD.encode(key.public_key()),
        })
    });
    write_json(
        &root.join("verifier.json"),
        &serde_json::json!({ "keys": keys }),
    );

    let valid_path = root.join("valid-production/receipt.json");
    let mut valid = read_receipt(&valid_path);
    hosted.seal(&mut valid).unwrap();
    write_receipt(&valid_path, &valid);

    let mut tampered_body = valid.clone();
    tampered_body.seal.reason = Some(runx_contracts::SealReason::GraphBlocked);
    write_receipt(&root.join("tampered-body/receipt.json"), &tampered_body);

    let mut tampered_signature = valid.clone();
    tampered_signature.signature.value =
        format!("base64:{}", URL_SAFE_NO_PAD.encode([0_u8; 64])).into();
    write_receipt(
        &root.join("tampered-signature/receipt.json"),
        &tampered_signature,
    );

    // Text past the Basic Multilingual Plane in a signed value: an
    // independent verifier must write it as the canonical writer does to
    // reproduce this receipt's id and digest.
    let mut wide = serde_json::to_value(&valid).unwrap();
    wide["subject"]["ref"]["label"] = serde_json::json!("receipt-verify \u{E000}\u{10000}");
    let mut wide: Receipt = serde_json::from_value(wide).unwrap();
    hosted.seal(&mut wide).unwrap();
    write_receipt(&root.join("valid-wide-text/receipt.json"), &wide);

    let mut unknown_kid = valid.clone();
    unknown_kid.issuer.kid = "unknown-fixture-key".into();
    write_receipt(&root.join("unknown-kid/receipt.json"), &unknown_kid);

    let local_path = root.join("broken-lineage-reference/receipt.json");
    let mut local_receipt = read_receipt(&local_path);
    local.seal(&mut local_receipt).unwrap();
    write_receipt(&local_path, &local_receipt);

    write_expected_verdicts(&root, &FixtureKeys(vec![&hosted, &local]));
    println!("regenerated strict receipt verify corpus");
}

fn read_receipt(path: &Path) -> Receipt {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write_expected_verdicts(root: &Path, keys: &FixtureKeys<'_>) {
    let context = ReceiptProofContext {
        signature_verifier: Some(keys),
    };
    for name in [
        "valid-production",
        "valid-wide-text",
        "tampered-body",
        "tampered-signature",
        "unknown-kid",
        "broken-lineage-reference",
        "malformed-json",
    ] {
        let bytes = fs::read(root.join(name).join("receipt.json")).unwrap();
        let verdict = verify_receipt_document_verdict(&bytes, &context);
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
