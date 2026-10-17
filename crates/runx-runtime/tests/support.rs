use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use runx_contracts::{Receipt, ReceiptIssuerType};
use runx_runtime::{
    Ed25519ReceiptSigner, KernelStore, LocalReceiptStore,
    RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_SIGN_KID_ENV, RunLog, RuntimeOptions, RuntimeReceiptSignatureConfig,
    RuntimeReceiptSignaturePolicy, RuntimeReceiptSigningError,
};

static FIXTURE_SIGNATURE: LazyLock<
    Result<RuntimeReceiptSignatureConfig, RuntimeReceiptSigningError>,
> = LazyLock::new(RuntimeReceiptSignatureConfig::fixture);

/// The public fixture identity: what a replayed harness case seals under.
pub(crate) fn fixture_signature()
-> Result<&'static RuntimeReceiptSignatureConfig, RuntimeReceiptSigningError> {
    FIXTURE_SIGNATURE.as_ref().map_err(Clone::clone)
}

/// Seal under, and check against, the fixture identity.
pub(crate) fn fixture_policy()
-> Result<RuntimeReceiptSignaturePolicy<'static>, RuntimeReceiptSigningError> {
    fixture_signature().map(RuntimeReceiptSignatureConfig::signature_policy)
}

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

/// The hosted identity `test_signing_env` configures.
pub(crate) fn test_signature_config()
-> Result<RuntimeReceiptSignatureConfig, RuntimeReceiptSigningError> {
    Ed25519ReceiptSigner::from_seed_base64(
        TEST_SIGNING_KID,
        ReceiptIssuerType::Hosted,
        TEST_SIGNING_SEED_BASE64,
    )
    .map(RuntimeReceiptSignatureConfig::signing)
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
) -> Result<RuntimeOptions, RuntimeReceiptSigningError> {
    Ok(RuntimeOptions::new(
        std::env::vars().collect(),
        test_signature_config()?,
        run,
    ))
}

/// A run sealed under the public fixture identity.
pub(crate) fn fixture_runtime_options(
    env: BTreeMap<String, String>,
    run: RunLog,
) -> Result<RuntimeOptions, RuntimeReceiptSigningError> {
    Ok(RuntimeOptions::new(
        env,
        RuntimeReceiptSignatureConfig::fixture()?,
        run,
    ))
}

pub(crate) fn read_test_signed_receipt(
    receipt_dir: &Path,
    receipt_id: &str,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let signature_config = test_signature_config()?;
    Ok(LocalReceiptStore::new(receipt_dir)
        .read_exact(receipt_id, signature_config.signature_policy())?)
}

/// The graph receipt's first step receipt: where a one-step runner's act lives.
pub(crate) fn read_test_first_step_receipt(
    receipt_dir: &Path,
    graph_receipt: &Receipt,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let child = graph_receipt
        .lineage
        .children
        .first()
        .ok_or("graph receipt has no step receipt")?;
    let id = child
        .uri
        .strip_prefix("runx:receipt:")
        .ok_or("step receipt reference is not a receipt uri")?;
    read_test_signed_receipt(receipt_dir, id)
}

/// Makes `root` a runx plugin holding copies of the runx source connectors
/// `ids`: how a project reaches published connectors without a registry.
pub(crate) fn plugin_with_source_connectors(root: &Path, ids: &[&str]) -> std::io::Result<()> {
    fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }
    fs::write(root.join("plugin.json"), r#"{"namespace":"runx"}"#)?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../connectors");
    for id in ids {
        copy(&source.join(id), &root.join("connectors").join(id))?;
    }
    Ok(())
}

/// Bind a resume written by address alone (`{answers: {address: value},
/// approvals: {address: approved}}`) to the requests run `run_id` waits on
/// now, as a host binds it from its read of the run.
pub(crate) fn bind_resume(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    run_id: &str,
    unbound: serde_json::Value,
) -> Result<runx_contracts::ResumeDocument, Box<dyn std::error::Error>> {
    let waiting = runx_runtime::run_record::paused_requests(env, cwd, run_id)?;
    let digest = |address: &str| {
        waiting
            .iter()
            .find(|request| request.id == address)
            .map(|request| serde_json::Value::String(request.digest.clone()))
            .ok_or_else(|| format!("run {run_id} waits on no request {address}"))
    };
    let mut bound = serde_json::Map::new();
    for (lane, entries) in unbound.as_object().into_iter().flatten() {
        let mut lane_entries = serde_json::Map::new();
        for (address, value) in entries.as_object().into_iter().flatten() {
            let mut entry =
                serde_json::Map::from_iter([("request_digest".to_owned(), digest(address)?)]);
            match (lane.as_str(), value) {
                ("approvals", serde_json::Value::Bool(approved)) => {
                    entry.insert("approved".to_owned(), serde_json::Value::Bool(*approved));
                }
                ("approvals", serde_json::Value::Object(decision)) => {
                    entry.extend(decision.clone());
                }
                _ => {
                    entry.insert("value".to_owned(), value.clone());
                }
            }
            lane_entries.insert(address.clone(), serde_json::Value::Object(entry));
        }
        bound.insert(lane.clone(), serde_json::Value::Object(lane_entries));
    }
    Ok(serde_json::from_value(serde_json::Value::Object(bound))?)
}
