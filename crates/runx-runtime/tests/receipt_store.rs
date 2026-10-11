// Test oracle: asserting via expect/unwrap is the intended failure mode, so the
// workspace expect/unwrap bans are lifted for this test target.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::{JsonObject, Receipt};
use runx_runtime::receipts::{RuntimeReceiptSignaturePolicy, step_receipt};
use runx_runtime::{
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, InvocationOutput, InvocationStatus,
    LocalReceiptStore, ReceiptStoreError,
};
use serde_json::json;

// Receipt ids are content-addressed (`id = hash(canonical_body)`), so the
// store fixtures derive their ids from the sealed receipt rather than a literal.
fn success_receipt_id() -> String {
    success_receipt().expect("success receipt").id.into_string()
}

#[test]
fn missing_store_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path().join("missing"));

    let result = store.list(crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::MissingStore { .. })
    ));
    Ok(())
}

#[test]
fn file_instead_of_directory_is_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store_path = temp.path().join("receipts");
    fs::write(&store_path, "not a directory")?;
    let store = LocalReceiptStore::new(&store_path);

    let result = store.list(crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::StoreNotDirectory { .. })
    ));
    Ok(())
}

#[test]
fn malformed_receipt_json_is_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    fs::write(temp.path().join("rcpt_bad.json"), "{")?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.read_exact("rcpt_bad", crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::MalformedJson { .. })
    ));
    Ok(())
}

#[test]
fn wrong_receipt_schema_is_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    write_json(
        temp.path(),
        "rcpt_wrong.json",
        &json!({
            "schema": "runx.not_receipt.v1",
            "id": "rcpt_wrong"
        }),
    )?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.read_exact("rcpt_wrong", crate::support::fixture_policy()?);

    assert!(matches!(result, Err(ReceiptStoreError::WrongSchema { .. })));
    Ok(())
}

#[test]
fn receipt_id_must_match_file_name() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    write_json(temp.path(), "rcpt_other.json", &success_receipt()?)?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.read_exact("rcpt_other", crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::IdFilenameMismatch { .. })
    ));
    Ok(())
}

#[test]
fn receipt_id_must_match_content_address() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let mut receipt = success_receipt()?;
    receipt.id = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        .to_owned()
        .into();
    write_json(temp.path(), &receipt_file_name(&receipt.id), &receipt)?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.read_exact(&receipt.id, crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptProofInvalid { .. })
    ));
    Ok(())
}

#[test]
fn exact_read_does_not_use_partial_or_suffix_lookup() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    write_json(
        temp.path(),
        &receipt_file_name(&success_receipt_id()),
        &success_receipt()?,
    )?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.read_exact("alpha", crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::MissingReceipt { .. })
    ));
    Ok(())
}

#[test]
fn exact_read_list_and_rebuild_index_succeed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let success = success_receipt()?;
    let abnormal = abnormal_receipt()?;
    write_json(temp.path(), &receipt_file_name(&success.id), &success)?;
    write_json(temp.path(), &receipt_file_name(&abnormal.id), &abnormal)?;
    fs::write(temp.path().join("notes.txt"), "ignored")?;
    write_json(
        temp.path(),
        "effect-state.json",
        &json!({
            "schema_version": "runx.effect_state.v1",
            "families": {}
        }),
    )?;
    let store = LocalReceiptStore::new(temp.path());

    let receipt = store.read_exact(&success.id, crate::support::fixture_policy()?)?;
    let listed = store.list(crate::support::fixture_policy()?)?;
    let index = store.rebuild_index(crate::support::fixture_policy()?)?;
    let loaded_index = store.load_index(crate::support::fixture_policy()?)?;

    let mut expected_ids = vec![success.id.clone(), abnormal.id.clone()];
    expected_ids.sort();

    assert_eq!(receipt.id, success.id);
    assert_eq!(
        listed
            .iter()
            .map(|receipt| receipt.id.clone())
            .collect::<Vec<_>>(),
        expected_ids
    );
    assert_eq!(
        index
            .entries
            .iter()
            .map(|entry| entry.receipt_id.clone())
            .collect::<Vec<_>>(),
        expected_ids
    );
    assert_eq!(
        index.entries[0].file_name,
        receipt_file_name(&expected_ids[0])
    );
    assert_eq!(loaded_index.entries, index.entries);
    assert!(temp.path().join("index.json").exists());
    Ok(())
}

#[test]
fn list_and_index_ignore_non_receipt_json_files() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let receipt = success_receipt()?;
    write_json(temp.path(), &receipt_file_name(&receipt.id), &receipt)?;
    fs::write(temp.path().join("sealed.json"), "")?;
    write_json(
        temp.path(),
        "answers.json",
        &json!({
            "answers": {}
        }),
    )?;
    write_json(
        temp.path(),
        "sha256-not-a-digest.json",
        &json!({
            "schema": "runx.receipt.v1",
            "id": "sha256:not-a-digest"
        }),
    )?;
    let store = LocalReceiptStore::new(temp.path());

    let listed = store.list(crate::support::fixture_policy()?)?;
    let index = store.rebuild_index(crate::support::fixture_policy()?)?;

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, receipt.id);
    assert_eq!(index.entries.len(), 1);
    assert_eq!(index.entries[0].receipt_id, receipt.id);
    Ok(())
}

#[test]
fn valid_runtime_generated_receipt_is_accepted_by_read_list_and_index()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let receipt = success_receipt()?;
    write_json(temp.path(), &receipt_file_name(&receipt.id), &receipt)?;
    let store = LocalReceiptStore::new(temp.path());

    assert_eq!(
        store
            .read_exact(&receipt.id, crate::support::fixture_policy()?)?
            .id,
        receipt.id
    );
    assert_eq!(store.list(crate::support::fixture_policy()?)?.len(), 1);
    assert_eq!(
        store
            .rebuild_index(crate::support::fixture_policy()?)?
            .entries[0]
            .receipt_id,
        receipt.id
    );
    assert_eq!(
        store.load_index(crate::support::fixture_policy()?)?.entries[0].receipt_id,
        receipt.id
    );
    Ok(())
}

#[test]
fn store_rejects_untrusted_local_issuer() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let receipt = success_receipt()?;
    write_json(temp.path(), &receipt_file_name(&receipt.id), &receipt)?;
    let store = LocalReceiptStore::new(temp.path());
    let another_home = Ed25519ReceiptSigner::local(&[0x07; 32])?;
    let verifier = Ed25519ReceiptVerifier::new([another_home.trusted_key()]);

    let result = store.read_exact(
        &receipt.id,
        RuntimeReceiptSignaturePolicy::verifying(&verifier),
    );

    assert!(matches!(
        &result,
        Err(ReceiptStoreError::ReceiptProofInvalid { .. })
    ));
    if let Err(ReceiptStoreError::ReceiptProofInvalid { message, .. }) = &result {
        assert!(
            message.contains("SignatureKeyMissing"),
            "expected an untrusted key finding, got {message}"
        );
    }
    Ok(())
}

#[test]
fn exact_read_rejects_structural_receipt_with_tampered_signature()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let mut receipt = success_receipt()?;
    receipt.signature.value = format!("base64:{}", "A".repeat(86)).into();
    write_json(temp.path(), &receipt_file_name(&receipt.id), &receipt)?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.read_exact(&receipt.id, crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptProofInvalid { .. })
    ));
    Ok(())
}

#[test]
fn list_rejects_structural_receipt_with_tampered_digest() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = TestDir::new()?;
    let mut receipt = success_receipt()?;
    receipt.digest = "sha256:tampered".into();
    write_json(temp.path(), &receipt_file_name(&receipt.id), &receipt)?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.list(crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptProofInvalid { .. })
    ));
    Ok(())
}

#[test]
fn load_index_rejects_indexed_structural_receipt_with_invalid_proof()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let mut receipt = success_receipt()?;
    receipt.signature.value = format!("base64:{}", "A".repeat(86)).into();
    write_json(temp.path(), &receipt_file_name(&receipt.id), &receipt)?;
    write_json(
        temp.path(),
        "index.json",
        &json!({
            "schema": "runx.receipt_store_index.v1",
            "generated_at": "1",
            "entries": [{
                "receipt_id": receipt.id,
                "file_name": receipt_file_name(&receipt.id),
                "created_at": receipt.created_at
            }]
        }),
    )?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.load_index(crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptProofInvalid { .. })
    ));
    Ok(())
}

#[test]
fn write_receipt_commits_readable_receipt_and_index() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path().join("receipts"));
    let receipt = success_receipt()?;

    store.write_receipt(&receipt, crate::support::fixture_policy()?)?;

    let stored = store.read_exact(&receipt.id, crate::support::fixture_policy()?)?;
    let index = store.load_index(crate::support::fixture_policy()?)?;
    assert_eq!(stored.id, receipt.id);
    assert_eq!(index.entries.len(), 1);
    assert_eq!(index.entries[0].receipt_id, receipt.id);
    assert!(store.root().join(receipt_file_name(&receipt.id)).exists());
    assert!(store.root().join("index.json").exists());
    Ok(())
}

#[test]
fn write_receipts_commits_one_durable_batch_and_index() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path().join("receipts"));
    let receipts = [success_receipt()?, abnormal_receipt()?];

    store.write_receipts(&receipts, crate::support::fixture_policy()?)?;

    let index = store.load_index(crate::support::fixture_policy()?)?;
    assert_eq!(index.entries.len(), receipts.len());
    for receipt in receipts {
        assert_eq!(
            store
                .read_exact(&receipt.id, crate::support::fixture_policy()?)?
                .id,
            receipt.id
        );
    }
    Ok(())
}

#[test]
fn concurrent_writers_commit_one_complete_index() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let root = Arc::new(temp.path().join("receipts"));
    let receipts = (0..16)
        .map(|index| {
            runtime_receipt(
                "concurrent-store",
                &format!("step-{index}"),
                InvocationStatus::Success,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let barrier = Arc::new(Barrier::new(receipts.len()));
    let writers = receipts
        .iter()
        .cloned()
        .map(|receipt| {
            let root = Arc::clone(&root);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                LocalReceiptStore::new(root.as_ref()).write_receipt(
                    &receipt,
                    crate::support::fixture_policy().expect("fixture key"),
                )
            })
        })
        .collect::<Vec<_>>();

    for writer in writers {
        writer.join().map_err(|_| "receipt writer panicked")??;
    }

    let store = LocalReceiptStore::new(root.as_ref());
    let index = store.load_index(crate::support::fixture_policy()?)?;
    assert_eq!(index.entries.len(), receipts.len());
    assert_eq!(
        store.list(crate::support::fixture_policy()?)?.len(),
        receipts.len()
    );
    Ok(())
}

#[test]
fn write_receipts_preflights_every_proof_before_writing() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path().join("receipts"));
    let valid = success_receipt()?;
    let mut invalid = abnormal_receipt()?;
    invalid.signature.value = format!("base64:{}", "A".repeat(86)).into();

    let result = store.write_receipts(&[valid.clone(), invalid], crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptProofInvalid { .. })
    ));
    assert!(!store.root().join(receipt_file_name(&valid.id)).exists());
    Ok(())
}

#[test]
fn content_addressed_receipts_use_platform_safe_file_names()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path().join("receipts"));
    let receipt = success_receipt()?;
    let file_name = receipt_file_name(&receipt.id);

    store.write_receipt(&receipt, crate::support::fixture_policy()?)?;

    assert!(receipt.id.to_string().starts_with("sha256:"));
    assert!(file_name.starts_with("sha256-"));
    assert!(!file_name.contains(':'));
    assert!(store.root().join(file_name).exists());
    Ok(())
}

#[test]
fn write_receipt_rejects_invalid_proof_without_writing() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path().join("receipts"));
    let mut receipt = success_receipt()?;
    receipt.signature.value = format!("base64:{}", "A".repeat(86)).into();

    let result = store.write_receipt(&receipt, crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptProofInvalid { .. })
    ));
    assert!(!store.root().join(receipt_file_name(&receipt.id)).exists());
    Ok(())
}

#[test]
fn write_receipt_allows_identical_and_rejects_divergent_rewrite()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path());
    let receipt = success_receipt()?;
    let mut changed = receipt.clone();
    changed.signature.value = format!("base64:{}", "B".repeat(86)).into();

    store.write_receipt(&receipt, crate::support::fixture_policy()?)?;
    store.write_receipt(&receipt, crate::support::fixture_policy()?)?;
    let result = store.write_receipt(&changed, crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptAlreadyExists { .. })
    ));
    Ok(())
}

#[test]
fn write_receipts_coalesces_identical_ids_and_rejects_divergent_content()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path());
    let receipt = success_receipt()?;

    store.write_receipts(
        &[receipt.clone(), receipt.clone()],
        crate::support::fixture_policy()?,
    )?;
    assert_eq!(store.list(crate::support::fixture_policy()?)?.len(), 1);
    assert_eq!(
        store
            .load_index(crate::support::fixture_policy()?)?
            .entries
            .len(),
        1
    );

    let mut changed = receipt.clone();
    changed.signature.value = format!("base64:{}", "B".repeat(86)).into();
    let result = store.write_receipts(&[receipt, changed], crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptAlreadyExists { .. })
    ));
    Ok(())
}

#[test]
fn identical_receipt_retry_repairs_a_missing_index_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path());
    let receipt = success_receipt()?;

    store.write_receipt(&receipt, crate::support::fixture_policy()?)?;
    fs::remove_file(store.root().join("index.json"))?;

    store.write_receipt(&receipt, crate::support::fixture_policy()?)?;

    let index = store.load_index(crate::support::fixture_policy()?)?;
    assert_eq!(index.entries.len(), 1);
    assert_eq!(index.entries[0].receipt_id, receipt.id);
    Ok(())
}

#[test]
fn index_write_failure_reports_stale_but_receipt_stays_readable()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    fs::create_dir(temp.path().join("index.json"))?;
    let store = LocalReceiptStore::new(temp.path());
    let receipt = success_receipt()?;

    let result = store.write_receipt(&receipt, crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptIndexStale { .. })
    ));
    assert_eq!(
        store
            .read_exact(&receipt.id, crate::support::fixture_policy()?)?
            .id,
        receipt.id
    );
    Ok(())
}

#[test]
fn unrelated_historical_proof_does_not_block_a_new_receipt_write()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let mut historical = success_receipt()?;
    historical.signature.value = format!("base64:{}", "C".repeat(86)).into();
    write_json(temp.path(), &receipt_file_name(&historical.id), &historical)?;
    let store = LocalReceiptStore::new(temp.path());
    let receipt = abnormal_receipt()?;

    store.write_receipt(&receipt, crate::support::fixture_policy()?)?;

    assert_eq!(
        store
            .read_exact(&receipt.id, crate::support::fixture_policy()?)?
            .id,
        receipt.id
    );
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(temp.path().join("index.json"))?)?;
    assert_eq!(index["entries"].as_array().map(Vec::len), Some(2));
    assert!(matches!(
        store.load_index(crate::support::fixture_policy()?),
        Err(ReceiptStoreError::ReceiptProofInvalid { .. })
    ));
    Ok(())
}

#[test]
fn malformed_index_is_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    fs::write(temp.path().join("index.json"), "{")?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.load_index(crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::MalformedIndex { .. })
    ));
    Ok(())
}

#[test]
fn stale_index_is_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    write_json(
        temp.path(),
        &receipt_file_name(&success_receipt_id()),
        &success_receipt()?,
    )?;
    write_json(
        temp.path(),
        "index.json",
        &json!({
            "schema": "runx.receipt_store_index.v1",
            "generated_at": "1",
            "entries": []
        }),
    )?;
    let store = LocalReceiptStore::new(temp.path());

    let result = store.load_index(crate::support::fixture_policy()?);

    assert!(matches!(
        result,
        Err(ReceiptStoreError::ReceiptIndexStale { .. })
    ));
    Ok(())
}

#[test]
fn receipt_store_error_display_does_not_leak_absolute_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let external = PathBuf::from("/Users/kam/private/runx-receipts");
    let errors = [
        ReceiptStoreError::MissingStore {
            path: external.clone(),
        }
        .to_string(),
        ReceiptStoreError::MalformedIndex {
            path: external,
            message: "bad".to_owned(),
        }
        .to_string(),
        ReceiptStoreError::ReceiptProofInvalid {
            path: PathBuf::from("/Users/kam/private/runx-receipts"),
            receipt_id: success_receipt_id(),
            message: "bad".to_owned(),
        }
        .to_string(),
    ];

    for message in errors {
        assert_redacts_external_path(&message);
    }
    Ok(())
}

#[test]
fn store_public_projection_redacts_external_absolute_root() {
    let workspace = PathBuf::from("/workspace/runx");
    let external = PathBuf::from("/Users/kam/private/runx-receipts");
    let store = LocalReceiptStore::new(&external);

    let projection = store.public_projection(&project_paths(&workspace));
    let summary = projection.summary();
    let label = projection.label().as_str();

    assert!(label.starts_with("external-receipt-store:"));
    assert!(summary.contains(label));
    assert_redacts_external_path(&summary);
}

#[test]
fn store_public_projection_uses_project_relative_root() {
    let workspace = PathBuf::from("/workspace/runx");
    let store = LocalReceiptStore::new(project_paths(&workspace).receipts());

    let projection = store.public_projection(&project_paths(&workspace));

    assert_eq!(projection.label().as_str(), ".runx/receipts");
    assert_eq!(projection.summary(), "receipt store: .runx/receipts");
}

#[test]
fn receipt_store_error_public_message_uses_safe_label() {
    let workspace = PathBuf::from("/workspace/runx");
    let external = PathBuf::from("/Users/kam/private/runx-receipts");
    let store = LocalReceiptStore::new(&external);
    let projection = store.public_projection(&project_paths(&workspace));
    let error = ReceiptStoreError::MissingStore { path: external };

    let message = error.public_message(projection.label());

    assert!(message.contains(projection.label().as_str()));
    assert_redacts_external_path(&message);
}

#[test]
fn receipt_store_error_public_message_redacts_path_like_fields() {
    let workspace = PathBuf::from("/workspace/runx");
    let external = PathBuf::from("/Users/kam/private/runx-receipts");
    let store = LocalReceiptStore::new(&external);
    let projection = store.public_projection(&project_paths(&workspace));
    let invalid_id = ReceiptStoreError::InvalidReceiptId {
        receipt_id: external.to_string_lossy().into_owned(),
    };
    let mismatch = ReceiptStoreError::IdFilenameMismatch {
        path: external,
        receipt_id: "/Users/kam/private/runx-receipts".to_owned(),
        file_stem: "runx-receipts".to_owned(),
    };

    let invalid_id_message = invalid_id.public_message(projection.label());
    let mismatch_message = mismatch.public_message(projection.label());

    assert_redacts_external_path(&invalid_id_message);
    assert_redacts_external_path(&mismatch_message);
}

fn success_receipt() -> Result<Receipt, Box<dyn std::error::Error>> {
    runtime_receipt("store", "alpha", InvocationStatus::Success)
}

fn abnormal_receipt() -> Result<Receipt, Box<dyn std::error::Error>> {
    runtime_receipt("store", "beta", InvocationStatus::Failure)
}

fn runtime_receipt(
    graph_name: &str,
    step_id: &str,
    status: InvocationStatus,
) -> Result<Receipt, Box<dyn std::error::Error>> {
    step_receipt(
        scope(graph_name),
        step_id,
        1,
        &skill_output(status),
        &JsonObject::new(),
        "2026-05-18T00:01:00Z",
        crate::support::fixture_policy()?,
    )
    .map_err(Into::into)
}

fn skill_output(status: InvocationStatus) -> InvocationOutput {
    let (stdout, stderr, exit_code) = match status {
        InvocationStatus::Success => ("ok".to_owned(), String::new(), Some(0)),
        InvocationStatus::Failure => (String::new(), "failed".to_owned(), Some(1)),
    };
    InvocationOutput::process(status, stdout, stderr, exit_code, 1)
}

fn receipt_file_name(receipt_id: &str) -> String {
    if let Some(digest) = receipt_id.strip_prefix("sha256:") {
        return format!("sha256-{digest}.json");
    }
    format!("{receipt_id}.json")
}

fn write_json<T: serde::Serialize>(
    dir: &Path,
    file_name: &str,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(dir.join(file_name), serde_json::to_string(value)?)?;
    Ok(())
}

struct TestDir {
    path: PathBuf,
}

static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

impl TestDir {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let serial = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "runx-runtime-receipt-store-{}-{serial}-{nanos}",
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

fn assert_redacts_external_path(text: &str) {
    assert!(!text.contains("/Users"));
    assert!(!text.contains("kam"));
    assert!(!text.contains("private"));
    assert!(!text.contains("runx-receipts"));
}

fn scope(graph_name: &str) -> runx_runtime::receipts::RunScope<'_> {
    runx_runtime::receipts::RunScope {
        run_id: "run_test",
        graph_name,
    }
}

fn project_paths(workspace: &std::path::Path) -> runx_runtime::RunxPaths {
    let env = std::collections::BTreeMap::from([(
        "RUNX_CWD".to_owned(),
        workspace.to_string_lossy().into_owned(),
    )]);
    runx_runtime::RunxPaths::resolve(&env, workspace)
}
