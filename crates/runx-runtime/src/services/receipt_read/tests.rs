use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{JsonObject, JsonValue, Receipt, ReceiptReadStatus};
use runx_core::state_machine::StepAdmissionWitness;

use super::read_receipt_claim;
use crate::adapter::{EphemeralValue, InvocationOutput};
use crate::execution::run_record::RunLog;
use crate::execution::runner::StepRun;
use crate::kernel::store::{KERNEL_STORE_FILE, KernelStore};
use crate::paths::{RUNX_CWD_ENV, RUNX_HOME_ENV, RUNX_PROJECT_DIR_ENV, RunxPaths};
use crate::receipts::store::LocalReceiptStore;
use crate::receipts::{RunScope, RuntimeReceiptSignatureConfig, step_receipt, workflow_receipt};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    root: tempfile::TempDir,
    env: BTreeMap<String, String>,
    receipt: Receipt,
    step: StepRun,
    claim: JsonObject,
}

fn fixture(poison_record: bool) -> TestResult<Fixture> {
    let root = tempfile::tempdir()?;
    let env = BTreeMap::from([
        (RUNX_CWD_ENV.to_owned(), root.path().display().to_string()),
        (
            RUNX_HOME_ENV.to_owned(),
            root.path().join("home").display().to_string(),
        ),
        (
            RUNX_PROJECT_DIR_ENV.to_owned(),
            root.path().join(".runx").display().to_string(),
        ),
    ]);
    let paths = RunxPaths::resolve(&env, root.path());
    let signature = RuntimeReceiptSignatureConfig::resolve(&env, &paths)?;
    let log = RunLog::evaluation(
        KernelStore::for_project(&paths)?,
        "run_claim",
        "fixture",
        "fixture",
        "2026-11-03T00:00:00Z",
    )?;
    let claim: JsonObject = serde_json::from_value(serde_json::json!({
        "result": {"id": "exact-result", "text": "東京", "odd/key": {"~": [{"": null}]}}
    }))?;
    // Neither transport output nor the ephemeral overlay is the signed claim.
    let output = InvocationOutput::runtime_success(
        JsonValue::String("transport-only-sentinel".to_owned()),
        0,
    );
    let receipt = step_receipt(
        RunScope {
            run_id: "run_claim",
            workflow_name: "fixture",
        },
        "read",
        1,
        &output,
        &claim,
        "2026-11-03T00:00:00Z",
        signature.signature_policy(),
    )?;
    LocalReceiptStore::new(paths.project().join("receipts"))
        .write_receipt(&receipt, signature.signature_policy())?;
    let mut retained = claim.clone();
    if poison_record {
        retained.insert(
            "result".to_owned(),
            JsonValue::String("changed after sealing".to_owned()),
        );
    }
    let step = StepRun {
        step_id: "read".to_owned(),
        attempt: 1,
        skill: "fixture".to_owned(),
        method: None,
        contract: retained,
        artifact_outputs: BTreeSet::new(),
        ephemeral_contract: EphemeralValue::from_value(JsonValue::String(
            "ephemeral-only-sentinel".to_owned(),
        )),
        outcome: output.into(),
        receipt: receipt.clone(),
        nested_receipts: Vec::new(),
        refusal: None,
        admission_witness: StepAdmissionWitness::local_runtime("read", receipt.id.as_str()),
        settlement: None,
        labels: runx_contracts::ArtifactLabels::CONFIGURED_MODEL,
    };
    log.record_step(&step, &[])?;
    Ok(Fixture {
        root,
        env,
        receipt,
        step,
        claim,
    })
}

fn read(fixture: &Fixture, pointer: &str, budget: u64) -> TestResult<runx_contracts::ReceiptRead> {
    Ok(read_receipt_claim(
        fixture.receipt.id.as_str(),
        None,
        pointer,
        budget,
        &fixture.env,
        fixture.root.path(),
    )?)
}

#[test]
fn receipt_read_returns_only_the_exact_signed_claim_and_rfc6901_selection() -> TestResult {
    let fixture = fixture(false)?;
    let full = read(&fixture, "", 4096)?;
    assert_eq!(full.status, ReceiptReadStatus::Available);
    assert!(full.root_receipt_id.is_none());
    assert!(
        !serde_json::to_value(&full)?
            .as_object()
            .ok_or("read is not an object")?
            .contains_key("root_receipt_id")
    );
    assert_eq!(full.value, Some(JsonValue::Object(fixture.claim.clone())));
    assert!(!serde_json::to_string(&full)?.contains("sentinel"));
    assert_eq!(
        read(&fixture, "/result/id", 100)?.value,
        Some(JsonValue::String("exact-result".to_owned()))
    );
    let null = read(&fixture, "/result/odd~1key/~0/0/", 100)?;
    assert_eq!(null.status, ReceiptReadStatus::Available);
    assert_eq!(null.value, Some(JsonValue::Null));
    assert!(read(&fixture, "/result/odd~1key/~0/00/", 100).is_err());
    assert!(read(&fixture, "not-a-pointer", 100).is_err());
    assert!(read(&fixture, "/missing", 100).is_err());
    // UTF-8 bytes, including the JSON quotes, never character count/truncation.
    assert_eq!(read(&fixture, "/result/text", 8)?.bytes, 8);
    assert!(read(&fixture, "/result/text", 7).is_err());
    assert!(read(&fixture, "", 0).is_err());
    Ok(())
}

fn parent(fixture: &Fixture, run_id: &str, steps: &[StepRun]) -> TestResult<Receipt> {
    let paths = RunxPaths::resolve(&fixture.env, fixture.root.path());
    let signature = RuntimeReceiptSignatureConfig::resolve(&fixture.env, &paths)?;
    let receipt = workflow_receipt(
        RunScope {
            run_id,
            workflow_name: "parent",
        },
        steps,
        "2026-11-04T00:00:00Z",
        signature.signature_policy(),
    )?;
    LocalReceiptStore::new(paths.project().join("receipts"))
        .write_receipt(&receipt, signature.signature_policy())?;
    Ok(receipt)
}

fn other_retained_step(fixture: &Fixture) -> TestResult<StepRun> {
    let paths = RunxPaths::resolve(&fixture.env, fixture.root.path());
    let signature = RuntimeReceiptSignatureConfig::resolve(&fixture.env, &paths)?;
    let output = InvocationOutput::runtime_success(JsonValue::Null, 0);
    let receipt = step_receipt(
        RunScope {
            run_id: "run_other",
            workflow_name: "other",
        },
        "other",
        1,
        &output,
        &fixture.claim,
        "2026-11-04T00:00:00Z",
        signature.signature_policy(),
    )?;
    let mut step = fixture.step.clone();
    step.step_id = "other".to_owned();
    step.admission_witness = StepAdmissionWitness::local_runtime("other", receipt.id.as_str());
    step.receipt = receipt;
    RunLog::evaluation(
        KernelStore::for_project(&paths)?,
        "run_other",
        "other",
        "other",
        "2026-11-04T00:00:00Z",
    )?
    .record_step(&step, &[])?;
    LocalReceiptStore::new(paths.project().join("receipts"))
        .write_receipt(&step.receipt, signature.signature_policy())?;
    Ok(step)
}

#[test]
fn receipt_read_binds_child_grandchild_and_self_to_the_original_signed_tree() -> TestResult {
    let fixture = fixture(false)?;
    let inner = parent(&fixture, "run_inner", std::slice::from_ref(&fixture.step))?;
    let direct = read_receipt_claim(
        fixture.receipt.id.as_str(),
        Some(inner.id.as_str()),
        "/result/id",
        100,
        &fixture.env,
        fixture.root.path(),
    )?;
    assert_eq!(direct.status, ReceiptReadStatus::Available);
    assert_eq!(
        direct.root_receipt_id.as_ref().map(|id| id.as_str()),
        Some(inner.id.as_str())
    );
    assert_eq!(
        direct.value,
        Some(JsonValue::String("exact-result".to_owned()))
    );
    let mut wrapper = fixture.step.clone();
    wrapper.receipt = inner.clone();
    wrapper.nested_receipts = vec![fixture.receipt.clone()];
    let outer = parent(&fixture, "run_outer", &[wrapper])?;
    let nested = read_receipt_claim(
        fixture.receipt.id.as_str(),
        Some(outer.id.as_str()),
        "/result/text",
        100,
        &fixture.env,
        fixture.root.path(),
    )?;
    assert_eq!(nested.value, Some(JsonValue::String("東京".to_owned())));
    assert_eq!(
        nested.root_receipt_id.as_ref().map(|id| id.as_str()),
        Some(outer.id.as_str())
    );
    let workflow_self = read_receipt_claim(
        outer.id.as_str(),
        Some(outer.id.as_str()),
        "",
        100,
        &fixture.env,
        fixture.root.path(),
    )?;
    assert_eq!(workflow_self.status, ReceiptReadStatus::NotAStep);
    let leaf_self = read_receipt_claim(
        fixture.receipt.id.as_str(),
        Some(fixture.receipt.id.as_str()),
        "",
        4096,
        &fixture.env,
        fixture.root.path(),
    )?;
    assert_eq!(leaf_self.value, Some(JsonValue::Object(fixture.claim)));
    Ok(())
}

#[test]
fn receipt_read_refuses_an_unrelated_valid_retained_receipt_in_the_same_store() -> TestResult {
    let fixture = fixture(false)?;
    let root = parent(
        &fixture,
        "run_original",
        std::slice::from_ref(&fixture.step),
    )?;
    let other = other_retained_step(&fixture)?;
    let separate = read_receipt_claim(
        other.receipt.id.as_str(),
        None,
        "",
        4096,
        &fixture.env,
        fixture.root.path(),
    )?;
    assert_eq!(separate.status, ReceiptReadStatus::Available);
    let error = read_receipt_claim(
        other.receipt.id.as_str(),
        Some(root.id.as_str()),
        "",
        4096,
        &fixture.env,
        fixture.root.path(),
    )
    .err()
    .ok_or("unrelated retained receipt was accepted")?;
    assert!(error.to_string().contains("does not belong"), "{error}");
    Ok(())
}

#[test]
fn receipt_read_requires_the_whole_original_tree_even_when_the_selected_leaf_is_valid() -> TestResult
{
    let fixture = fixture(false)?;
    let other = other_retained_step(&fixture)?;
    let root = parent(
        &fixture,
        "run_original",
        &[fixture.step.clone(), other.clone()],
    )?;
    let paths = RunxPaths::resolve(&fixture.env, fixture.root.path());
    let store = LocalReceiptStore::new(paths.project().join("receipts"));
    std::fs::remove_file(store.receipt_path(other.receipt.id.as_str())?)?;
    assert_eq!(
        read(&fixture, "", 4096)?.status,
        ReceiptReadStatus::Available
    );
    let error = read_receipt_claim(
        fixture.receipt.id.as_str(),
        Some(root.id.as_str()),
        "",
        4096,
        &fixture.env,
        fixture.root.path(),
    )
    .err()
    .ok_or("incomplete original tree was accepted")?;
    assert!(
        error.to_string().contains("incomplete or invalid"),
        "{error}"
    );
    Ok(())
}

#[test]
fn receipt_read_rejects_a_retained_record_that_the_signed_receipt_does_not_bind() -> TestResult {
    let fixture = fixture(true)?;
    let error = read(&fixture, "", 4096)
        .err()
        .ok_or("poisoned record was read")?;
    assert!(
        error
            .to_string()
            .contains("does not bind its retained step claim")
    );
    Ok(())
}

#[test]
fn receipt_read_does_not_treat_a_shared_receipt_destination_as_shared_workspace_state() -> TestResult
{
    let fixture = fixture(false)?;
    let other = tempfile::tempdir()?;
    let mut env = fixture.env.clone();
    env.insert(RUNX_CWD_ENV.to_owned(), other.path().display().to_string());
    env.insert(
        RUNX_PROJECT_DIR_ENV.to_owned(),
        other.path().join(".runx").display().to_string(),
    );
    env.insert(
        crate::RUNX_RECEIPT_DIR_ENV.to_owned(),
        fixture
            .root
            .path()
            .join(".runx/receipts")
            .display()
            .to_string(),
    );
    for original_root in [None, Some(fixture.receipt.id.as_str())] {
        let read = read_receipt_claim(
            fixture.receipt.id.as_str(),
            original_root,
            "",
            4096,
            &env,
            other.path(),
        )?;
        assert_eq!(read.status, ReceiptReadStatus::NotRetained);
        assert!(read.value.is_none());
    }
    assert!(!other.path().join(".runx").exists());
    Ok(())
}

#[test]
fn receipt_read_verifies_the_receipt_before_consulting_its_retained_claim() -> TestResult {
    let fixture = fixture(false)?;
    let mut env = fixture.env.clone();
    let foreign_home = fixture.root.path().join("foreign-home");
    env.insert(RUNX_HOME_ENV.to_owned(), foreign_home.display().to_string());
    RuntimeReceiptSignatureConfig::resolve(&env, &RunxPaths::resolve(&env, fixture.root.path()))?;
    assert!(
        read_receipt_claim(
            fixture.receipt.id.as_str(),
            None,
            "",
            4096,
            &env,
            fixture.root.path()
        )
        .is_err()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn receipt_read_follows_the_kernel_owners_database_alias_semantics() -> TestResult {
    let fixture = fixture(false)?;
    let other = tempfile::tempdir()?;
    std::fs::create_dir_all(other.path().join(".runx"))?;
    std::os::unix::fs::symlink(
        fixture.root.path().join(".runx").join(KERNEL_STORE_FILE),
        other.path().join(".runx").join(KERNEL_STORE_FILE),
    )?;
    let mut env = fixture.env.clone();
    env.insert(
        RUNX_PROJECT_DIR_ENV.to_owned(),
        other.path().join(".runx").display().to_string(),
    );
    env.insert(
        crate::RUNX_RECEIPT_DIR_ENV.to_owned(),
        fixture
            .root
            .path()
            .join(".runx/receipts")
            .display()
            .to_string(),
    );
    // An operator-selected database alias is the store this workspace's runs
    // use too. The reader must share that owner rather than invent a separate
    // identity rule. A shared receipt directory alone still grants no state.
    let paths = RunxPaths::resolve(&env, other.path());
    let _store = KernelStore::for_project(&paths)?;
    let read = read_receipt_claim(
        fixture.receipt.id.as_str(),
        None,
        "",
        4096,
        &env,
        other.path(),
    )?;
    assert_eq!(read.status, ReceiptReadStatus::Available);
    assert_eq!(read.value, Some(JsonValue::Object(fixture.claim)));
    Ok(())
}
