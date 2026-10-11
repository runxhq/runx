use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue, Receipt};
use serde::Deserialize;

use super::prove_receipts;
use crate::adapter::InvocationOutput;
use crate::paths::{RUNX_HOME_ENV, RunxPaths};
use crate::receipts::{
    RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_VERIFY_KID_ENV, RunScope, RuntimeReceiptSignatureConfig, step_receipt,
};
use crate::{RUNX_CWD_ENV, RUNX_RECEIPT_DIR_ENV};

const MISSING_RECEIPT_ID: &str =
    "sha256:fc1bb8c2027c1b0a76d8095a1d6c112e37a4f4d144991be4d505ec21314ccd39";

/// A workspace in `root` with its own home and receipt directory.
fn workspace(root: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        (RUNX_CWD_ENV.to_owned(), root.display().to_string()),
        (
            RUNX_HOME_ENV.to_owned(),
            root.join("home").display().to_string(),
        ),
        (
            RUNX_RECEIPT_DIR_ENV.to_owned(),
            root.join("receipts").display().to_string(),
        ),
    ])
}

/// Trust the corpus key of `issuer_type`, as an operator configures it.
fn trust_corpus_key(env: &mut BTreeMap<String, String>, issuer_type: &str) -> Result<(), String> {
    let verifier: VerifierFixture = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/receipt-verify/verifier.json"
    )))
    .map_err(|error| error.to_string())?;
    let key = verifier
        .keys
        .into_iter()
        .find(|key| key.issuer_type == issuer_type)
        .ok_or("corpus has no key of that issuer type")?;
    env.insert(
        RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV.to_owned(),
        key.issuer_type,
    );
    env.insert(RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(), key.kid);
    env.insert(
        RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV.to_owned(),
        key.public_key_base64,
    );
    Ok(())
}

fn corpus_receipt(case: &str) -> Result<Receipt, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/receipt-verify")
        .join(case)
        .join("receipt.json");
    serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

#[test]
fn proves_exact_hosted_receipt_without_a_store_scan() -> Result<(), String> {
    let receipt = corpus_receipt("valid-production")?;
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut env = workspace(temp.path());
    trust_corpus_key(&mut env, "hosted")?;
    fs::create_dir_all(temp.path().join("receipts")).map_err(|error| error.to_string())?;
    write_receipt(&temp.path().join("receipts"), &receipt)?;

    let proof = prove_receipts(&inputs(&receipt.id), &env, temp.path())
        .map_err(|error| error.to_string())?;

    assert_eq!(text(&proof, &["decision"]), Some("verified"));
    assert_eq!(boolean(&proof, &["verification", "intact"]), Some(true));
    assert_eq!(issuer_type(&proof), Some("hosted"));
    Ok(())
}

#[test]
fn a_missing_receipt_needs_more_evidence() -> Result<(), String> {
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let env = workspace(temp.path());

    let proof = prove_receipts(&inputs(MISSING_RECEIPT_ID), &env, temp.path())
        .map_err(|error| error.to_string())?;

    assert_eq!(text(&proof, &["decision"]), Some("needs_more_evidence"));
    assert_eq!(boolean(&proof, &["verification", "intact"]), Some(false));
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_home_key_proves_the_receipts_it_sealed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let env = workspace(temp.path());
    let signature =
        RuntimeReceiptSignatureConfig::resolve(&env, &RunxPaths::resolve(&env, temp.path()))?;
    let receipt = step_receipt(
        RunScope {
            run_id: "run_home",
            graph_name: "home",
        },
        "step",
        1,
        &InvocationOutput::runtime_success(JsonValue::Object(JsonObject::new()), 0),
        &JsonObject::new(),
        "2026-10-11T00:00:00Z",
        signature.signature_policy(),
    )?;
    fs::create_dir_all(temp.path().join("receipts"))?;
    write_receipt(&temp.path().join("receipts"), &receipt)?;

    let proof = prove_receipts(&inputs(&receipt.id), &env, temp.path())?;

    assert_eq!(text(&proof, &["decision"]), Some("verified"));
    assert_eq!(issuer_type(&proof), Some("local"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_local_receipt_proves_only_under_its_own_key() -> Result<(), String> {
    let receipt = corpus_receipt("broken-lineage-reference")?;
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut env = workspace(temp.path());
    fs::create_dir_all(temp.path().join("receipts")).map_err(|error| error.to_string())?;
    write_receipt(&temp.path().join("receipts"), &receipt)?;

    // Another home's key, however valid, is not this home's.
    RuntimeReceiptSignatureConfig::resolve(&env, &RunxPaths::resolve(&env, temp.path()))
        .map_err(|error| error.to_string())?;
    let untrusted = prove_receipts(&inputs(&receipt.id), &env, temp.path())
        .map_err(|error| error.to_string())?;
    assert_eq!(text(&untrusted, &["decision"]), Some("needs_more_evidence"));

    trust_corpus_key(&mut env, "local")?;
    let trusted = prove_receipts(&inputs(&receipt.id), &env, temp.path())
        .map_err(|error| error.to_string())?;
    assert_eq!(text(&trusted, &["decision"]), Some("verified"));
    assert_eq!(issuer_type(&trusted), Some("local"));
    Ok(())
}

#[derive(Deserialize)]
struct VerifierFixture {
    keys: Vec<VerifierKey>,
}

#[derive(Deserialize)]
struct VerifierKey {
    issuer_type: String,
    kid: String,
    public_key_base64: String,
}

fn issuer_type(proof: &JsonObject) -> Option<&str> {
    proof
        .get("receipt_details")?
        .as_array()?
        .first()?
        .as_object()?
        .get("issuer_type")?
        .as_str()
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
