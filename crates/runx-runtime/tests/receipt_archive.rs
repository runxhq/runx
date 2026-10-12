use std::collections::BTreeMap;
use std::fs;
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
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, FIXTURE_RECEIPT_SEED, RunScope,
    RuntimeReceiptSignaturePolicy, RuntimeReceiptSigner, step_receipt,
};
use runx_runtime::{InvocationOutput, InvocationStatus, LocalReceiptStore, RunxPaths};

const ADMITTED_ACTOR: &str = "runx:principal:operator";
const SIGNED_RUNTIME_SUBJECT: &str = "runx:run:run_receipt-archive#strict-proof";
const SCOPE: RunScope<'static> = RunScope {
    run_id: "run_receipt-archive",
    graph_name: "receipt-archive",
};

fn paths(workspace: &Path) -> RunxPaths {
    let env = BTreeMap::from([(
        "RUNX_CWD".to_owned(),
        workspace.to_string_lossy().into_owned(),
    )]);
    RunxPaths::resolve(&env, workspace)
}

fn list(
    store: &LocalReceiptStore,
    workspace: &Path,
    filter: &ReceiptFilter,
    policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<ReceiptListing, ReceiptListingError> {
    list_receipts(store, &paths(workspace), filter, policy)
}

#[test]
fn missing_history_store_projects_empty_safe_result() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let store = LocalReceiptStore::new(paths(&workspace).receipts());

    let history = list(
        &store,
        &workspace,
        &ReceiptFilter::default(),
        crate::support::fixture_policy()?,
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
    let store = LocalReceiptStore::new(paths(&workspace).receipts());
    let old_receipt = generated_runtime_receipt_with(
        "rcpt_old",
        InvocationStatus::Success,
        "2026-05-18T00:00:01Z",
    )?;
    let new_receipt = generated_runtime_receipt_with(
        "rcpt_new",
        InvocationStatus::Success,
        "2026-05-19T00:00:00Z",
    )?;
    let expected_order = vec![receipt_uri(&new_receipt.id), receipt_uri(&old_receipt.id)];
    store.write_receipt(&old_receipt, crate::support::fixture_policy()?)?;
    store.write_receipt(&new_receipt, crate::support::fixture_policy()?)?;

    let history = list(
        &store,
        &workspace,
        &ReceiptFilter {
            query: Some("receipt-archive".to_owned()),
            issuer: Some("LOCAL".to_owned()),
            since: Some("2026-05-18T00:00:00Z".to_owned()),
            limit: Some(2),
            ..ReceiptFilter::default()
        },
        crate::support::fixture_policy()?,
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
    // A step that made no admitted call states no actor.
    assert_eq!(history.receipts[1].actor, None);
    assert!(
        history.receipts[0]
            .receipt_ref
            .starts_with(RECEIPT_REF_PREFIX)
    );
    assert_no_local_paths(&serde_json::to_string(&history)?);
    Ok(())
}

#[test]
fn receipt_inspection_projects_signed_governance_without_execution_bodies()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let store = LocalReceiptStore::new(paths(&workspace).receipts());
    let mut receipt = generated_runtime_receipt()?;
    receipt.authority.scope_refs.push(Reference::with_uri(
        ReferenceType::ScopeAdmission,
        "runx:scope_admission:repo.read",
    ));
    reseal_receipt(&mut receipt)?;
    store.write_receipt(&receipt, crate::support::fixture_policy()?)?;

    let inspection = inspect_local_receipt(
        &store,
        &paths(&workspace),
        receipt.id.as_str(),
        crate::support::fixture_policy()?,
    )?;
    assert_eq!(inspection.receipt.id, receipt.id.to_string());
    assert_eq!(
        inspection.receipt.authority.exercised_scopes[0].scope,
        "repo:read"
    );
    // A step that made no governed call seals no act; how it ended is the
    // seal's, and inspection shows it.
    assert!(inspection.receipt.acts.is_empty());
    assert_eq!(
        inspection.receipt.seal_criteria[0].criterion_id,
        "step_outcome"
    );
    assert_eq!(inspection.receipt.seal_criteria[0].status, "verified");
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
    let store = LocalReceiptStore::new(paths(&workspace).receipts());
    let mut revision = generated_runtime_receipt_with(
        "rcpt_revision",
        InvocationStatus::Success,
        "2026-05-18T00:01:00Z",
    )?;
    revision.authority.actor_ref = Some(Reference::with_uri(
        ReferenceType::Principal,
        ADMITTED_ACTOR,
    ));
    reseal_receipt(&mut revision)?;
    let revision_id = revision.id.to_string();
    store.write_receipt(&revision, crate::support::fixture_policy()?)?;
    store.write_receipt(
        &generated_runtime_receipt_with(
            "rcpt_deploy",
            InvocationStatus::Failure,
            "2026-05-19T00:01:00Z",
        )?,
        crate::support::fixture_policy()?,
    )?;

    let history = list(
        &store,
        &workspace,
        &ReceiptFilter {
            skill: Some("receipt-archive".to_owned()),
            status: Some("closed".to_owned()),
            actor: Some(ADMITTED_ACTOR.to_owned()),
            until: Some("2026-05-18T23:59:59Z".to_owned()),
            ..ReceiptFilter::default()
        },
        crate::support::fixture_policy()?,
    )?;

    assert_eq!(history.receipts.len(), 1);
    assert_eq!(history.receipts[0].id, revision_id);
    Ok(())
}

#[test]
fn history_filter_intersects_skill_status_issuer_and_date_range()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let store = LocalReceiptStore::new(paths(&workspace).receipts());

    let matching = generated_runtime_receipt_with(
        "rcpt_matching",
        InvocationStatus::Success,
        "2026-05-18T12:00:00Z",
    )?;
    let matching_id = matching.id.to_string();
    store.write_receipt(&matching, crate::support::fixture_policy()?)?;

    store.write_receipt(
        &generated_runtime_receipt_with(
            "rcpt_wrong_status",
            InvocationStatus::Failure,
            "2026-05-18T12:30:00Z",
        )?,
        crate::support::fixture_policy()?,
    )?;

    store.write_receipt(
        &generated_runtime_receipt_with(
            "rcpt_outside_window",
            InvocationStatus::Success,
            "2026-05-19T00:00:01Z",
        )?,
        crate::support::fixture_policy()?,
    )?;

    let history = list(
        &store,
        &workspace,
        &ReceiptFilter {
            skill: Some("receipt-archive".to_owned()),
            status: Some("CLOSED".to_owned()),
            issuer: Some("LOCAL".to_owned()),
            since: Some("2026-05-18T00:00:00Z".to_owned()),
            until: Some("2026-05-19T00:00:00Z".to_owned()),
            ..ReceiptFilter::default()
        },
        crate::support::fixture_policy()?,
    )?;

    assert_eq!(
        history
            .receipts
            .iter()
            .map(|receipt| receipt.id.as_str())
            .collect::<Vec<_>>(),
        vec![matching_id.as_str()]
    );
    Ok(())
}

#[test]
fn history_rejects_invalid_date_filters() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let store = LocalReceiptStore::new(paths(&workspace).receipts());
    let result = list(
        &store,
        &workspace,
        &ReceiptFilter {
            since: Some("not-a-date".to_owned()),
            ..ReceiptFilter::default()
        },
        crate::support::fixture_policy()?,
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
        &ReceiptFilter::default(),
        crate::support::fixture_policy()?,
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
    let store = LocalReceiptStore::new(paths(&workspace).receipts());
    let mut receipt = generated_runtime_receipt()?;
    receipt.digest = "sha256:stale".into();
    assert!(runx_receipts::verify_receipt(&receipt).valid);
    write_receipt_json(store.root(), &receipt)?;

    let history = list(
        &store,
        &workspace,
        &ReceiptFilter::default(),
        crate::support::fixture_policy()?,
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
    let store = LocalReceiptStore::new(paths(&workspace).receipts());
    let mut receipt = generated_runtime_receipt()?;
    receipt.signature.value = format!("base64:{}", "A".repeat(86)).into();
    assert!(runx_receipts::verify_receipt(&receipt).valid);
    write_receipt_json(store.root(), &receipt)?;

    let history = list(
        &store,
        &workspace,
        &ReceiptFilter::default(),
        crate::support::fixture_policy()?,
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
    let store = LocalReceiptStore::new(paths(&workspace).receipts());
    // "verified" means the receipt's proof checks under a key the reader
    // trusts.
    let signer = fixture_signer()?;
    let verifier = fixture_verifier(&signer);
    let receipt = production_generated_receipt(&signer, &verifier)?;
    write_receipt_json(store.root(), &receipt)?;

    let history = list_receipts(
        &store,
        &paths(&workspace),
        &ReceiptFilter::default(),
        RuntimeReceiptSignaturePolicy::verifying(&verifier),
    )?;
    assert_eq!(history.receipts[0].verification.status, "verified");
    Ok(())
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
        format!(r#"{{"message":"{id}"}}"#),
        String::new(),
        Some(if status == InvocationStatus::Success {
            0
        } else {
            1
        }),
        10,
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
        crate::support::fixture_policy()?,
    )?;
    reseal_receipt(&mut receipt)?;
    Ok(receipt)
}

fn reseal_receipt(receipt: &mut Receipt) -> Result<(), Box<dyn std::error::Error>> {
    receipt.id = runx_receipts::content_addressed_receipt_id(receipt)?.into();
    let digest = runx_receipts::canonical_receipt_body_digest(receipt)?;
    receipt.digest = digest.clone().into();
    receipt.signature =
        Ed25519ReceiptSigner::local(&FIXTURE_RECEIPT_SEED)?.sign_receipt_body(&digest)?;
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
    Ed25519ReceiptVerifier::new([signer.trusted_key()])
}

fn production_generated_receipt(
    signer: &Ed25519ReceiptSigner,
    verifier: &Ed25519ReceiptVerifier,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    let output = InvocationOutput::process(
        InvocationStatus::Success,
        r#"{"message":"production"}"#.to_owned(),
        String::new(),
        Some(0),
        10,
    );
    let claim = output
        .value
        .as_object()
        .cloned()
        .ok_or("fixture output must be an object")?;
    Ok(step_receipt(
        SCOPE,
        "strict-proof",
        1,
        &output,
        &claim,
        "2026-05-18T00:00:01Z",
        RuntimeReceiptSignaturePolicy::signing(signer, verifier),
    )?)
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
