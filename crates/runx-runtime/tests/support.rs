use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::Receipt;
use runx_runtime::{HarnessOptions, RuntimeOptions};
use runx_runtime::{
    KernelStore, LocalReceiptStore, RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV,
    RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV, RUNX_RECEIPT_SIGN_KID_ENV, RunLog,
    RuntimeReceiptSignatureConfig,
};

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
    fs::write(dir.join("run.yaml"), profile)
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

/// A project directory of this test's own under the target's test scratch
/// space, so runs and custody never land in the repository's `.runx`.
#[cfg(feature = "mcp")]
pub(crate) fn test_project_dir(label: &str) -> Result<std::path::PathBuf, std::io::Error> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// A started evaluation run recorded under `dir/.runx`.
pub(crate) fn run_in(dir: &Path, run_id: &str) -> Result<RunLog, Box<dyn std::error::Error>> {
    Ok(RunLog::evaluation(
        KernelStore::open(&dir.join(".runx"))?,
        run_id,
        "integration-test",
        "integration-test",
        "2026-05-18T00:00:00Z",
    )?)
}

/// A started evaluation run in a kernel store of its own, discarded with the
/// guard.
pub(crate) fn scratch_run(
    run_id: &str,
) -> Result<(tempfile::TempDir, RunLog), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let log = RunLog::evaluation(
        KernelStore::open(root.path())?,
        run_id,
        "integration-test",
        "integration-test",
        "2026-05-18T00:00:00Z",
    )?;
    Ok((root, log))
}

pub(crate) fn signed_runtime_options(
    run: RunLog,
) -> Result<RuntimeOptions, runx_runtime::RuntimeError> {
    let mut env = std::env::vars().collect();
    insert_test_signing_env(&mut env);
    RuntimeOptions::from_env(env, run)
}

pub(crate) fn local_harness_options() -> HarnessOptions {
    let mut options = HarnessOptions::local_development(std::env::vars().collect());
    options.created_at = TEST_CREATED_AT.to_owned();
    options
}

pub(crate) fn read_test_signed_receipt(
    receipt_dir: &Path,
    receipt_id: &str,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let signature_config = test_signature_config()?;
    Ok(LocalReceiptStore::new(receipt_dir)
        .read_exact_with_policy(receipt_id, signature_config.signature_policy())?)
}

/// The graph receipt's first step receipt: where a one-step runner's act lives.
pub(crate) fn read_test_first_step_receipt(
    receipt_dir: &Path,
    graph_receipt: &Receipt,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let child = graph_receipt
        .lineage
        .as_ref()
        .and_then(|lineage| lineage.children.first())
        .ok_or("graph receipt has no step receipt")?;
    let id = child
        .uri
        .strip_prefix("runx:receipt:")
        .ok_or("step receipt reference is not a receipt uri")?;
    read_test_signed_receipt(receipt_dir, id)
}
