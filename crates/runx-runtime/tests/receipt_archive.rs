use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::{Receipt, ReceiptIssuerType, Reference, ReferenceType};
use runx_runtime::receipts::archive::{
    RECEIPT_LISTING_PROJECTOR_ID, RECEIPT_REF_PREFIX, ReceiptFilter, ReceiptListing,
    ReceiptListingError, list_receipts, receipt_uri,
};
use runx_runtime::receipts::inspection::inspect_local_receipt;
use runx_runtime::receipts::{
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, RunScope, RuntimeReceiptSignaturePolicy,
    step_receipt_with_signature_policy,
};
use runx_runtime::{InvocationOutput, InvocationStatus, LocalReceiptStore};
use serde_json::json;

const SIGNED_LOCAL_ACTOR: &str = "runx:principal:local_runtime";
const SIGNED_RUNTIME_SUBJECT: &str = "runx:run:run_receipt-archive#strict-proof";
const SCOPE: RunScope<'static> = RunScope {
    run_id: "run_receipt-archive",
    graph_name: "receipt-archive",
};

fn list(
    store: &LocalReceiptStore,
    workspace: &Path,
    project_runx_dir: &Path,
    filter: &ReceiptFilter,
) -> Result<ReceiptListing, ReceiptListingError> {
    list_receipts(
        store,
        workspace,
        project_runx_dir,
        filter,
        RuntimeReceiptSignaturePolicy::local_development(),
    )
}

#[test]
fn missing_history_store_projects_empty_safe_result() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));

    let history = list(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter::default(),
    )?;

    assert_eq!(history.projector_id, RECEIPT_LISTING_PROJECTOR_ID);
    assert_eq!(history.store_label, ".runx/receipts");
    assert!(history.receipts.is_empty());
    assert_no_local_paths(&serde_json::to_string(&history)?);
    Ok(())
}

#[test]
fn history_lists_receipts_newest_first_with_safe_refs_and_filters()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let old_receipt = receipt_with_metadata(
        InvocationStatus::Success,
        "rcpt_old",
        "2026-05-18T00:00:01Z",
        "Revision Skill",
        "local",
        "runner-a",
    )?;
    let new_receipt = receipt_with_metadata(
        InvocationStatus::Success,
        "rcpt_new",
        "2026-05-19T00:00:00Z",
        "Deploy Skill",
        "local",
        "runner-b",
    )?;
    let expected_order = vec![receipt_uri(&new_receipt.id), receipt_uri(&old_receipt.id)];
    store.write_receipt(&old_receipt)?;
    store.write_receipt(&new_receipt)?;

    let history = list(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter {
            query: Some("artifact".to_owned()),
            issuer: Some("LOCAL".to_owned()),
            since: Some("2026-05-18T00:00:00Z".to_owned()),
            limit: Some(2),
            ..ReceiptFilter::default()
        },
    )?;
    assert_eq!(
        history
            .receipts
            .iter()
            .map(|receipt| receipt.receipt_ref.clone())
            .collect::<Vec<_>>(),
        expected_order
    );
    assert_eq!(history.receipts[0].subject_ref, SIGNED_RUNTIME_SUBJECT);
    assert_eq!(history.receipts[0].issuer_type, "local");
    assert_eq!(history.receipts[1].actors, vec![SIGNED_LOCAL_ACTOR]);
    assert!(
        history.receipts[0]
            .artifact_types
            .contains(&"artifact".to_owned())
    );
    assert!(
        history.receipts[0]
            .receipt_ref
            .starts_with(RECEIPT_REF_PREFIX)
    );
    assert_no_local_paths(&serde_json::to_string(&history)?);
    Ok(())
}

#[test]
fn history_display_identity_ignores_unsigned_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    store.write_receipt(&receipt_with_metadata(
        InvocationStatus::Success,
        "rcpt_forged_metadata",
        "2026-05-18T00:00:01Z",
        "Forged Skill",
        "forged-source",
        "forged-runner",
    )?)?;

    let history = list(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter::default(),
    )?;

    assert_eq!(history.receipts[0].subject_ref, SIGNED_RUNTIME_SUBJECT);
    assert_eq!(history.receipts[0].name, "receipt-archive");
    assert_eq!(history.receipts[0].issuer_type, "local");
    assert_eq!(history.receipts[0].actors, vec![SIGNED_LOCAL_ACTOR]);
    Ok(())
}

#[test]
fn receipt_inspection_projects_signed_governance_without_execution_bodies()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let mut receipt = generated_runtime_receipt()?;
    receipt.authority.scope_refs.push(Reference::with_uri(
        ReferenceType::ScopeAdmission,
        "runx:scope_admission:repo.read",
    ));
    reseal_receipt(&mut receipt)?;
    store.write_receipt(&receipt)?;

    let inspection =
        inspect_local_receipt(&store, &workspace, &project_runx_dir, receipt.id.as_str())?;
    assert_eq!(inspection.receipt.id, receipt.id.to_string());
    assert_eq!(
        inspection.receipt.authority.exercised_scopes[0].scope,
        "repo:read"
    );
    assert!(!inspection.receipt.acts.is_empty());
    let serialized = serde_json::to_string(&inspection)?;
    assert!(!serialized.contains("structured_output"));
    assert!(!serialized.contains("stderr"));
    assert_no_local_paths(&serialized);
    Ok(())
}

#[test]
fn history_filter_matches_actor_status_skill_and_date() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let revision = receipt_with_metadata(
        InvocationStatus::Success,
        "rcpt_revision",
        "2026-05-18T00:01:00Z",
        "Revision Skill",
        "local",
        "runner-a",
    )?;
    let revision_id = revision.id.to_string();
    store.write_receipt(&revision)?;
    store.write_receipt(&receipt_with_metadata(
        InvocationStatus::Failure,
        "rcpt_deploy",
        "2026-05-19T00:01:00Z",
        "Deploy Skill",
        "local",
        "runner-b",
    )?)?;

    let history = list(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter {
            skill: Some("receipt-archive".to_owned()),
            status: Some("closed".to_owned()),
            actor: Some(SIGNED_LOCAL_ACTOR.to_owned()),
            until: Some("2026-05-18T23:59:59Z".to_owned()),
            ..ReceiptFilter::default()
        },
    )?;

    assert_eq!(history.receipts.len(), 1);
    assert_eq!(history.receipts[0].id, revision_id);
    Ok(())
}

#[test]
fn history_filter_intersects_skill_status_source_artifact_and_date_range()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));

    let mut matching = receipt_with_metadata(
        InvocationStatus::Success,
        "rcpt_matching",
        "2026-05-18T12:00:00Z",
        "Deploy Skill",
        "local",
        "runner-a",
    )?;
    set_artifact_label(&mut matching, "deploy-bundle")?;
    let matching_id = matching.id.to_string();
    store.write_receipt(&matching)?;

    let mut wrong_artifact = receipt_with_metadata(
        InvocationStatus::Success,
        "rcpt_wrong_artifact",
        "2026-05-18T12:30:00Z",
        "Deploy Skill",
        "local",
        "runner-a",
    )?;
    set_artifact_label(&mut wrong_artifact, "diagnostic-log")?;
    store.write_receipt(&wrong_artifact)?;

    let mut outside_window = receipt_with_metadata(
        InvocationStatus::Success,
        "rcpt_outside_window",
        "2026-05-19T00:00:01Z",
        "Deploy Skill",
        "local",
        "runner-a",
    )?;
    set_artifact_label(&mut outside_window, "deploy-bundle")?;
    store.write_receipt(&outside_window)?;

    let history = list(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter {
            skill: Some("receipt-archive".to_owned()),
            status: Some("CLOSED".to_owned()),
            issuer: Some("LOCAL".to_owned()),
            artifact_type: Some("deploy-bundle".to_owned()),
            since: Some("2026-05-18T00:00:00Z".to_owned()),
            until: Some("2026-05-19T00:00:00Z".to_owned()),
            ..ReceiptFilter::default()
        },
    )?;

    assert_eq!(
        history
            .receipts
            .iter()
            .map(|receipt| receipt.id.as_str())
            .collect::<Vec<_>>(),
        vec![matching_id.as_str()]
    );
    assert_eq!(history.receipts[0].artifact_types, vec!["deploy-bundle"]);
    Ok(())
}

#[test]
fn history_rejects_invalid_date_filters() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let result = list(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter {
            since: Some("not-a-date".to_owned()),
            ..ReceiptFilter::default()
        },
    );

    assert!(matches!(
        result,
        Err(ReceiptListingError::InvalidTimestamp { field, .. }) if field == "since"
    ));
    Ok(())
}

#[test]
fn malformed_history_store_remains_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path());
    fs::create_dir_all(temp.path())?;
    fs::write(store.receipt_path(&valid_content_receipt_id())?, "{")?;

    let result = list(
        &store,
        temp.path(),
        &temp.path().join(".runx"),
        &ReceiptFilter::default(),
    );

    assert!(matches!(
        result,
        Err(ReceiptListingError::ReceiptStore(
            runx_runtime::ReceiptStoreError::MalformedJson { .. }
        ))
    ));
    Ok(())
}

#[test]
fn history_projection_fails_structurally_valid_stale_receipt_digest()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let mut receipt = generated_runtime_receipt()?;
    receipt.digest = "sha256:stale".into();
    assert!(runx_receipts::verify_receipt(&receipt).valid);
    write_receipt_json(store.root(), &receipt)?;

    let history = list(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter::default(),
    )?;

    // History lists every receipt and labels trust (verified/unverified/invalid)
    // rather than failing closed: a structurally-valid but tamper-detected
    // receipt projects as "invalid".
    assert_eq!(history.receipts[0].verification.status, "invalid");
    Ok(())
}

#[test]
fn history_projection_fails_structurally_valid_tampered_receipt_signature()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let mut receipt = generated_runtime_receipt()?;
    receipt.signature.value = "sig:sha256:tampered".into();
    assert!(runx_receipts::verify_receipt(&receipt).valid);
    write_receipt_json(store.root(), &receipt)?;

    let history = list(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter::default(),
    )?;

    // History lists every receipt and labels trust (verified/unverified/invalid)
    // rather than failing closed: a structurally-valid but tamper-detected
    // receipt projects as "invalid".
    assert_eq!(history.receipts[0].verification.status, "invalid");
    Ok(())
}

#[test]
fn runtime_generated_receipts_project_verified() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    // "verified" is reserved for production-signed receipts confirmed by a real
    // verifier; a local pseudo-signature can never earn it (see dod3).
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let receipt = production_generated_receipt(&signer, &verifier)?;
    write_receipt_json(store.root(), &receipt)?;

    let history = list_receipts(
        &store,
        &workspace,
        &project_runx_dir,
        &ReceiptFilter::default(),
        RuntimeReceiptSignaturePolicy::production(&verifier),
    )?;
    assert_eq!(history.receipts[0].verification.status, "verified");
    Ok(())
}

fn receipt_with_metadata(
    status: InvocationStatus,
    id: &str,
    created_at: &str,
    skill_name: &str,
    source_type: &str,
    actor: &str,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let mut receipt = generated_runtime_receipt_with(id, status, created_at)?;
    receipt.metadata = Some(json_object(json!({
        "skill_name": skill_name,
        "source_type": source_type,
        "runner": {
            "provider": actor
        }
    }))?);
    reseal_receipt(&mut receipt)?;
    Ok(receipt)
}

fn generated_runtime_receipt() -> Result<Receipt, Box<dyn std::error::Error>> {
    generated_runtime_receipt_with(
        "rcpt_strict-proof",
        InvocationStatus::Success,
        "2026-05-18T00:00:01Z",
    )
}

fn generated_runtime_receipt_with(
    id: &str,
    status: InvocationStatus,
    created_at: &str,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let output = InvocationOutput::process(
        status.clone(),
        format!(r#"{{"artifact":{{"artifact_id":"artifact_{id}","artifact_type":"artifact"}}}}"#),
        String::new(),
        Some(if status == InvocationStatus::Success {
            0
        } else {
            1
        }),
        10,
        BTreeMap::new(),
    );
    let claim = output
        .value
        .as_object()
        .cloned()
        .ok_or("fixture output must be an object")?;
    let mut receipt = runx_runtime::receipts::step_receipt(
        SCOPE,
        "strict-proof",
        1,
        &output,
        &claim,
        created_at,
    )?;
    reseal_receipt(&mut receipt)?;
    Ok(receipt)
}

fn reseal_receipt(receipt: &mut Receipt) -> Result<(), Box<dyn std::error::Error>> {
    receipt.id = runx_receipts::content_addressed_receipt_id(receipt)?.into();
    let digest = runx_receipts::canonical_receipt_body_digest(receipt)?;
    receipt.digest = digest.clone().into();
    receipt.signature.value = format!("sig:{digest}").into();
    Ok(())
}

fn valid_content_receipt_id() -> String {
    format!("sha256:{}", "0".repeat(64))
}

fn fixture_signer() -> Result<Ed25519ReceiptSigner, Box<dyn std::error::Error>> {
    Ok(Ed25519ReceiptSigner::from_seed(
        FIXTURE_KID,
        ReceiptIssuerType::Hosted,
        &FIXTURE_SEED,
    )?)
}

fn fixture_verifier(signer: &Ed25519ReceiptSigner) -> Ed25519ReceiptVerifier {
    Ed25519ReceiptVerifier::new([signer.production_key()])
}

fn production_generated_receipt(
    signer: &Ed25519ReceiptSigner,
    verifier: &Ed25519ReceiptVerifier,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let output = InvocationOutput::process(
        InvocationStatus::Success,
        r#"{"artifact":{"artifact_id":"artifact_prod","artifact_type":"artifact"}}"#.to_owned(),
        String::new(),
        Some(0),
        10,
        BTreeMap::new(),
    );
    let claim = output
        .value
        .as_object()
        .cloned()
        .ok_or("fixture output must be an object")?;
    Ok(step_receipt_with_signature_policy(
        SCOPE,
        "strict-proof",
        1,
        &output,
        &claim,
        "2026-05-18T00:00:01Z",
        RuntimeReceiptSignaturePolicy::production_signing(signer, verifier),
    )?)
}

fn set_artifact_label(
    receipt: &mut Receipt,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for reference in receipt
        .acts
        .iter_mut()
        .flat_map(|act| act.artifact_refs.iter_mut())
    {
        if reference.reference_type == ReferenceType::Artifact {
            reference.label = Some(label.to_owned().into());
        }
    }
    reseal_receipt(receipt)?;
    Ok(())
}

fn json_object(value: serde_json::Value) -> Result<runx_contracts::JsonObject, io::Error> {
    serde_json::from_value(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
}

fn write_receipt_json(dir: &Path, receipt: &Receipt) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    fs::write(
        LocalReceiptStore::new(dir).receipt_path(&receipt.id)?,
        serde_json::to_string(receipt)?,
    )?;
    Ok(())
}

fn assert_no_local_paths(text: &str) {
    assert!(!text.contains("/Users/"));
    assert!(!text.contains("/private/"));
    assert!(!text.contains("runx-runtime-receipt-archive"));
}

const FIXTURE_KID: &str = "runx-runtime-prod-fixture-key";
const FIXTURE_SEED: [u8; 32] = [0x42; 32];

struct TestDir {
    path: PathBuf,
}

static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

impl TestDir {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let serial = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "runx-runtime-receipt-archive-{}-{serial}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.path);
    }
}
