use std::fs;

use runx_contracts::{JsonObject, JsonValue, Receipt};
use serde::Deserialize;

use super::prove_receipts;
use crate::{
    RUNX_CWD_ENV, RUNX_RECEIPT_DIR_ENV,
    receipts::{RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_KID_ENV},
};

const MISSING_RECEIPT_ID: &str =
    "sha256:fc1bb8c2027c1b0a76d8095a1d6c112e37a4f4d144991be4d505ec21314ccd39";

#[test]
fn proves_exact_production_receipt_without_a_store_scan() -> Result<(), String> {
    const PRODUCTION_RECEIPT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/receipt-verify/valid-production/receipt.json"
    ));
    let receipt: Receipt =
        serde_json::from_str(PRODUCTION_RECEIPT).map_err(|error| error.to_string())?;
    let verifier: VerifierFixture = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/receipt-verify/verifier.json"
    )))
    .map_err(|error| error.to_string())?;
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let receipt_dir = temp.path().join("receipts");
    fs::create_dir_all(&receipt_dir).map_err(|error| error.to_string())?;
    write_receipt(&receipt_dir, &receipt)?;
    let env = std::collections::BTreeMap::from([
        (
            RUNX_CWD_ENV.to_owned(),
            temp.path().to_string_lossy().into_owned(),
        ),
        (
            RUNX_RECEIPT_DIR_ENV.to_owned(),
            receipt_dir.to_string_lossy().into_owned(),
        ),
        (RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(), verifier.kid),
        (
            RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV.to_owned(),
            verifier.public_key_base64,
        ),
    ]);
    let proof = prove_receipts(&inputs(&receipt.id), &env, temp.path())
        .map_err(|error| error.to_string())?;
    assert_eq!(text(&proof, &["decision"]), Some("verified"));
    assert_eq!(
        text(&proof, &["verification", "signature_mode"]),
        Some("production")
    );
    assert_eq!(boolean(&proof, &["verification", "intact"]), Some(true));
    Ok(())
}

#[test]
fn local_receipt_proof_never_claims_production_verification() -> Result<(), String> {
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let env = std::collections::BTreeMap::from([
        (
            RUNX_CWD_ENV.to_owned(),
            temp.path().to_string_lossy().into_owned(),
        ),
        (
            RUNX_RECEIPT_DIR_ENV.to_owned(),
            temp.path().join("receipts").to_string_lossy().into_owned(),
        ),
    ]);
    let proof = prove_receipts(&inputs(MISSING_RECEIPT_ID), &env, temp.path())
        .map_err(|error| error.to_string())?;
    assert_eq!(text(&proof, &["decision"]), Some("needs_more_evidence"));
    assert_eq!(
        text(&proof, &["verification", "signature_mode"]),
        Some("local-development")
    );
    Ok(())
}

#[test]
fn valid_local_receipt_remains_explicitly_unverified() -> Result<(), String> {
    const LOCAL_RECEIPT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/harness/oracle/echo-skill.default.json"
    ));
    let receipt: Receipt =
        serde_json::from_str(LOCAL_RECEIPT).map_err(|error| error.to_string())?;
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let receipt_dir = temp.path().join("receipts");
    fs::create_dir_all(&receipt_dir).map_err(|error| error.to_string())?;
    fs::write(
        receipt_dir.join(format!("sha256-{}.json", &receipt.id[7..])),
        LOCAL_RECEIPT,
    )
    .map_err(|error| error.to_string())?;
    let env = std::collections::BTreeMap::from([(
        RUNX_RECEIPT_DIR_ENV.to_owned(),
        receipt_dir.to_string_lossy().into_owned(),
    )]);

    let proof = prove_receipts(&inputs(&receipt.id), &env, temp.path())
        .map_err(|error| error.to_string())?;

    assert_eq!(text(&proof, &["decision"]), Some("unverified"));
    assert_eq!(
        text(&proof, &["verification", "signature_mode"]),
        Some("local-development")
    );
    assert_eq!(boolean(&proof, &["verification", "intact"]), None);
    Ok(())
}

#[derive(Deserialize)]
struct CompositeFixture {
    outer: Receipt,
    inner_receipts: Vec<Receipt>,
}

#[derive(Deserialize)]
struct VerifierFixture {
    kid: String,
    public_key_base64: String,
}

#[test]
fn local_composite_proof_loads_typed_inner_evidence() -> Result<(), String> {
    const COMPOSITE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/contracts/receipt-composition/marketplace-composite.json"
    ));
    let fixture: CompositeFixture =
        serde_json::from_str(COMPOSITE).map_err(|error| error.to_string())?;
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let receipt_dir = temp.path().join("receipts");
    fs::create_dir_all(&receipt_dir).map_err(|error| error.to_string())?;
    write_receipt(&receipt_dir, &fixture.outer)?;
    for inner in &fixture.inner_receipts {
        write_receipt(&receipt_dir, inner)?;
    }
    let env = std::collections::BTreeMap::from([(
        RUNX_RECEIPT_DIR_ENV.to_owned(),
        receipt_dir.to_string_lossy().into_owned(),
    )]);

    let proof = prove_receipts(&inputs(&fixture.outer.id), &env, temp.path())
        .map_err(|error| error.to_string())?;
    assert_eq!(text(&proof, &["decision"]), Some("unverified"));
    let verification = at(&proof, &["verification"])
        .and_then(JsonValue::as_object)
        .ok_or("missing verification object")?;
    let trees = verification
        .get("trees")
        .and_then(JsonValue::as_array)
        .ok_or("missing receipt trees")?;
    assert_eq!(
        trees
            .first()
            .and_then(JsonValue::as_object)
            .and_then(|tree| tree.get("valid"))
            .and_then(JsonValue::as_bool),
        Some(true)
    );
    assert!(
        verification
            .get("findings")
            .and_then(JsonValue::as_array)
            .is_some_and(Vec::is_empty)
    );
    Ok(())
}

fn write_receipt(receipt_dir: &std::path::Path, receipt: &Receipt) -> Result<(), String> {
    let digest = receipt
        .id
        .strip_prefix("sha256:")
        .ok_or("receipt fixture id is not content-addressed")?;
    fs::write(
        receipt_dir.join(format!("sha256-{digest}.json")),
        serde_json::to_vec(receipt).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn inputs(receipt_id: &str) -> Vec<String> {
    vec![receipt_id.to_owned()]
}

fn text<'a>(object: &'a JsonObject, path: &[&str]) -> Option<&'a str> {
    at(object, path)?.as_str()
}

fn boolean(object: &JsonObject, path: &[&str]) -> Option<bool> {
    at(object, path)?.as_bool()
}

fn at<'a>(object: &'a JsonObject, path: &[&str]) -> Option<&'a JsonValue> {
    let (first, rest) = path.split_first()?;
    let mut value = object.get(*first)?;
    for key in rest {
        value = value.as_object()?.get(*key)?;
    }
    Some(value)
}
