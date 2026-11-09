//! The canonical receipts Cloud's spec shows are exactly what their scenarios
//! seal, and each verifies against the demo JWKS the spec names, so neither
//! the fixtures nor the spec can drift from what runx makes.

use std::error::Error;
use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use runx_contracts::ReceiptIssuerType;
use runx_receipts::{ReceiptProofContext, verify_receipt_document_verdict};
use runx_runtime::{Ed25519ReceiptVerifier, TrustedReceiptKey};

#[path = "../examples/runx-canonical-receipts/scenarios.rs"]
mod scenarios;

#[test]
fn the_canonical_receipts_are_what_their_scenarios_seal_and_verify_under_the_demo_jwks()
-> Result<(), Box<dyn Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let verifier = demo_jwks_verifier(&repo)?;
    let receipts = scenarios::canonical_receipts(&repo)?;
    assert_eq!(receipts.len(), 3);
    for canonical in receipts {
        let fixture = fs::read_to_string(repo.join(scenarios::FIXTURE_DIR).join(canonical.file))?;
        assert_eq!(
            fixture,
            scenarios::render(&canonical.receipt)?,
            "{} is not what its scenario seals; make it again with `pnpm fixtures:canonical-receipts:generate`",
            canonical.file
        );
        let verdict = verify_receipt_document_verdict(
            fixture.as_bytes(),
            &ReceiptProofContext {
                signature_verifier: Some(&verifier),
            },
        );
        assert!(verdict.valid, "{}: {verdict:?}", canonical.file);
    }
    Ok(())
}

/// A verifier that trusts exactly the keys tools/verify/runx-demo-jwks.json
/// publishes.
fn demo_jwks_verifier(repo: &Path) -> Result<Ed25519ReceiptVerifier, Box<dyn Error>> {
    let jwks: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        repo.join("tools/verify/runx-demo-jwks.json"),
    )?)?;
    let keys = jwks["keys"]
        .as_array()
        .ok_or("the demo JWKS has no keys")?
        .iter()
        .map(|key| {
            assert_eq!(key["kty"], "OKP");
            assert_eq!(key["crv"], "Ed25519");
            assert_eq!(key["issuer_type"], "local");
            let kid = key["kid"].as_str().ok_or("a demo key has no kid")?;
            let x = key["x"].as_str().ok_or("a demo key has no x")?;
            Ok(TrustedReceiptKey::new(
                ReceiptIssuerType::Local,
                kid,
                URL_SAFE_NO_PAD.decode(x)?,
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    Ok(Ed25519ReceiptVerifier::new(keys))
}
