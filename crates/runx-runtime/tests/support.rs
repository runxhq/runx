use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::Receipt;
#[cfg(feature = "cli-tool")]
use runx_runtime::RuntimeOptions;
use runx_runtime::{
    LocalReceiptStore, RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV,
    RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV, RUNX_RECEIPT_SIGN_KID_ENV, RuntimeReceiptSignatureConfig,
};

#[cfg(feature = "cli-tool")]
pub(crate) const TEST_CREATED_AT: &str = "2026-05-18T00:00:00Z";
pub(crate) const TEST_SIGNING_KID: &str = "runx-runtime-prod-fixture-key";
pub(crate) const TEST_SIGNING_SEED_BASE64: &str = "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=";
pub(crate) const TEST_SIGNING_ISSUER_TYPE: &str = "hosted";

pub(crate) fn write_test_skill_package(
    dir: &Path,
    manual: &str,
    profile: &str,
) -> Result<(), std::io::Error> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join("SKILL.md"), manual)?;
    fs::write(dir.join("X.yaml"), profile)
}

pub(crate) fn test_signing_env() -> BTreeMap<String, String> {
    [
        (
            RUNX_RECEIPT_SIGN_KID_ENV.to_owned(),
            TEST_SIGNING_KID.to_owned(),
        ),
        (
            RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV.to_owned(),
            TEST_SIGNING_SEED_BASE64.to_owned(),
        ),
        (
            RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV.to_owned(),
            TEST_SIGNING_ISSUER_TYPE.to_owned(),
        ),
    ]
    .into_iter()
    .collect()
}

pub(crate) fn insert_test_signing_env(env: &mut BTreeMap<String, String>) {
    for (key, value) in test_signing_env() {
        env.entry(key).or_insert(value);
    }
}

pub(crate) fn test_signature_config()
-> Result<RuntimeReceiptSignatureConfig, Box<dyn std::error::Error>> {
    Ok(RuntimeReceiptSignatureConfig::from_env(&test_signing_env())?)
}

#[cfg(feature = "cli-tool")]
pub(crate) fn signed_runtime_options() -> Result<RuntimeOptions, runx_runtime::RuntimeError> {
    let mut env = std::env::vars().collect();
    insert_test_signing_env(&mut env);
    RuntimeOptions::from_env(env)
}

#[cfg(feature = "cli-tool")]
pub(crate) fn local_harness_runtime_options() -> RuntimeOptions {
    RuntimeOptions {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        created_at: TEST_CREATED_AT.to_owned(),
        ..RuntimeOptions::local_development(std::env::vars().collect())
    }
}

pub(crate) fn read_test_signed_receipt(
    receipt_dir: &Path,
    receipt_id: &str,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let signature_config = test_signature_config()?;
    Ok(LocalReceiptStore::new(receipt_dir)
        .read_exact_with_policy(receipt_id, signature_config.signature_policy())?)
}
