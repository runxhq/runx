// Module rationale: the initial journal projection slice
// keeps history filtering and receipt-backed rows together until CLI wiring
// decides the permanent module boundary.
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use fs2::FileExt;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ClosureDisposition, ExecutionEvent, Receipt, ReferenceType, canonical_stable_json, sha256_hex,
};
use runx_core::policy::parse_rfc3339_moment;
use runx_receipts::{
    ReceiptFindingCode, ReceiptProofContextProvider, signed_display_identity, verify_receipt_proof,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::lifecycle::receipt_lifecycle_records;
use crate::receipts::paths::safe_receipt_store_label;
use crate::receipts::store::{LocalReceiptStore, ReceiptStoreError};
use crate::receipts::{RuntimeReceiptProofContextProvider, RuntimeReceiptSignaturePolicy};

mod inspection;

pub use inspection::{
    RECEIPT_INSPECTION_PROJECTOR_ID, RECEIPT_INSPECTION_SCHEMA, ReceiptActInspection,
    ReceiptAuthorityInspection, ReceiptDecisionInspection, ReceiptExercisedScope,
    ReceiptInspection, ReceiptInspectionProjection, inspect_local_receipt,
    inspect_local_receipt_with_policy, project_receipt_inspection,
    project_receipt_inspection_with_policy,
};

pub const JOURNAL_PROJECTION_SCHEMA: &str = "runx.journal_projection.v1";
pub const JOURNAL_PROJECTOR_ID: &str = "runx-runtime.local-journal.v1";
pub const HISTORY_PROJECTOR_ID: &str = "runx-runtime.local-history.v1";
pub const RECEIPT_REF_PREFIX: &str = "runx:receipt:";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub event: ExecutionEvent,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExecutionJournal {
    entries: Vec<JournalEntry>,
}

impl ExecutionJournal {
    pub fn push(&mut self, event: ExecutionEvent) {
        self.entries.push(JournalEntry { event });
    }

    #[must_use]
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryFilter {
    pub query: Option<String>,
    pub skill: Option<String>,
    pub status: Option<String>,
    pub source: Option<String>,
    pub actor: Option<String>,
    pub artifact_type: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<usize>,
    pub include_harness: bool,
    /// Include graph-internal step receipts. The default history surface is
    /// one row per top-level run; use this only for diagnostics.
    pub include_internal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalHistoryProjection {
    pub projector_id: String,
    pub store_label: String,
    pub receipts: Vec<LocalHistoryReceipt>,
    #[serde(rename = "pendingRuns")]
    pub pending_runs: Vec<PausedRunSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalHistoryReceipt {
    pub id: String,
    pub receipt_ref: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub harness_id: String,
    pub harness_state: String,
    pub summary: String,
    pub source_type: Option<String>,
    pub actors: Vec<String>,
    pub artifact_types: Vec<String>,
    pub verification: ReceiptVerificationProjection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptVerificationProjection {
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PausedRunSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_skill_ref: Option<String>,
    pub selected_runner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_closure_digest: Option<String>,
    pub step_ids: Vec<String>,
    pub step_labels: Vec<String>,
    pub ledger_verification: Option<LedgerVerificationProjection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerVerificationProjection {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PausedRunCheckpoint {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub started_at: Option<String>,
    pub resume_skill_ref: Option<String>,
    pub selected_runner: Option<String>,
    pub credential_profile: Option<String>,
    pub package_digest: Option<String>,
    pub execution_closure_digest: Option<String>,
    pub step_ids: Vec<String>,
    pub step_labels: Vec<String>,
}

pub fn append_paused_run_checkpoint(
    receipt_dir: &Path,
    checkpoint: &PausedRunCheckpoint,
) -> Result<(), std::io::Error> {
    let ledgers_dir = receipt_dir.join("ledgers");
    fs::create_dir_all(&ledgers_dir)?;
    let ledger_path = ledgers_dir.join(format!("{}.jsonl", checkpoint.id));
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(ledger_path)?;
    file.lock_exclusive()?;

    let mut existing = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut existing)?;
    let (index, previous_hash) = verified_ledger_head(&existing, &checkpoint.id)?;
    let entry = paused_checkpoint_entry(checkpoint)?;
    let entry_hash = ledger_entry_hash(index, previous_hash.as_deref(), &entry)?;
    let record = serde_json::json!({
        "schema_version": "runx.ledger.entry.v1",
        "chain": {
            "version": "runx.ledger.chain.v1",
            "algorithm": "sha256",
            "canonicalization": "runx.stable-json.v1",
            "index": index,
            "previous_hash": previous_hash,
            "entry_hash": entry_hash,
        },
        "entry": entry,
    });

    file.seek(SeekFrom::End(0))?;
    serde_json::to_writer(&mut file, &record)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

fn paused_checkpoint_entry(
    checkpoint: &PausedRunCheckpoint,
) -> Result<serde_json::Value, std::io::Error> {
    let data = serde_json::json!({
        "kind": "resolution_requested",
        "status": "waiting",
        "step_id": checkpoint.step_ids.first(),
        "detail": {
            "resume_skill_ref": checkpoint.resume_skill_ref,
            "selected_runner": checkpoint.selected_runner,
            "credential_profile": checkpoint.credential_profile,
            "package_digest": checkpoint.package_digest,
            "execution_closure_digest": checkpoint.execution_closure_digest,
            "step_ids": checkpoint.step_ids,
            "step_labels": checkpoint.step_labels,
        },
    });
    let payload = serde_json::json!({
        "type": "run_event",
        "version": "1",
        "data": data,
    });
    let payload_hash = stable_json_sha256(&payload)?;
    let created_at = checkpoint
        .started_at
        .clone()
        .unwrap_or_else(crate::time::now_iso8601);
    let runner = checkpoint
        .selected_runner
        .clone()
        .unwrap_or_else(|| "default".to_owned());
    let size_bytes = serde_json::to_vec(&data)?.len();
    Ok(serde_json::json!({
        "type": "run_event",
        "version": "1",
        "data": data,
        "meta": {
            "artifact_id": format!("ax_{}", &payload_hash[..16]),
            "run_id": checkpoint.id,
            "step_id": checkpoint.step_ids.first(),
            "producer": { "skill": checkpoint.name, "runner": runner },
            "created_at": created_at,
            "hash": payload_hash,
            "size_bytes": size_bytes,
            "parent_artifact_id": null,
            "receipt_id": null,
            "redacted": false,
        },
    }))
}

fn verified_ledger_head(
    contents: &str,
    run_id: &str,
) -> Result<(u64, Option<String>), std::io::Error> {
    let mut previous_hash: Option<String> = None;
    let mut index = 0_u64;
    for (line_index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| invalid_ledger(line_index, error.to_string()))?;
        serde_json::from_value::<runx_contracts::LedgerEntry>(record.clone())
            .map_err(|error| invalid_ledger(line_index, error.to_string()))?;
        let chain = record
            .get("chain")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| invalid_ledger(line_index, "missing chain"))?;
        let actual_index = chain
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| invalid_ledger(line_index, "missing chain index"))?;
        let actual_previous = chain
            .get("previous_hash")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let actual_hash = chain
            .get("entry_hash")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid_ledger(line_index, "missing entry hash"))?;
        let entry = record
            .get("entry")
            .ok_or_else(|| invalid_ledger(line_index, "missing entry"))?;
        let entry_run_id = entry
            .pointer("/meta/run_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid_ledger(line_index, "missing entry run id"))?;
        let expected_hash = ledger_entry_hash(index, previous_hash.as_deref(), entry)?;
        if actual_index != index
            || actual_previous != previous_hash
            || actual_hash != expected_hash
            || entry_run_id != run_id
        {
            return Err(invalid_ledger(line_index, "ledger chain mismatch"));
        }
        previous_hash = Some(expected_hash);
        index = index.saturating_add(1);
    }
    Ok((index, previous_hash))
}

fn ledger_entry_hash(
    index: u64,
    previous_hash: Option<&str>,
    entry: &serde_json::Value,
) -> Result<String, std::io::Error> {
    stable_json_sha256(&serde_json::json!({
        "version": "runx.ledger.chain-payload.v1",
        "index": index,
        "previous_hash": previous_hash,
        "entry": entry,
    }))
}

fn stable_json_sha256(value: &serde_json::Value) -> Result<String, std::io::Error> {
    let value = serde_json::from_value::<runx_contracts::JsonValue>(value.clone())
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
    canonical_stable_json(&value)
        .map(|canonical| sha256_hex(canonical.as_bytes()))
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))
}

fn invalid_ledger(line_index: usize, reason: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(
        ErrorKind::InvalidData,
        format!("ledger line {} is invalid: {reason}", line_index + 1),
    )
}

/// Resolve one paused run from its own ledger and graph checkpoint.
///
/// Resume is a point lookup, not a history projection. Keeping it independent
/// of the receipt archive prevents an unrelated unreadable receipt from
/// blocking a valid continuation while still failing closed on the requested
/// run's own state.
pub fn find_paused_run(
    receipt_dir: &Path,
    run_id: &str,
) -> Result<Option<PausedRunSummary>, JournalProjectionError> {
    if !valid_run_id(run_id) {
        return Ok(None);
    }
    let ledger_path = receipt_dir.join("ledgers").join(format!("{run_id}.jsonl"));
    let pending = match fs::metadata(&ledger_path) {
        Ok(metadata) if metadata.is_file() => paused_run_from_ledger(run_id, &ledger_path),
        Ok(_) => Err(JournalProjectionError::LedgerStoreUnreadable),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err(JournalProjectionError::LedgerStoreUnreadable),
    }?;
    let Some(pending) = pending else {
        return Ok(None);
    };
    match graph_checkpoint_status_for_run(receipt_dir, run_id)? {
        GraphCheckpointStatus::Terminal => return Ok(None),
        GraphCheckpointStatus::Active => return Ok(Some(pending)),
        GraphCheckpointStatus::Missing => {}
    }
    if terminal_receipt_exists_for_run(receipt_dir, &pending)? {
        return Ok(None);
    }
    Ok(Some(pending))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalProjection {
    pub schema: String,
    pub projector_id: String,
    pub receipt_ref: String,
    pub watermark: String,
    pub rows: Vec<JournalProjectionRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalProjectionRow {
    pub schema: String,
    pub entry_id: String,
    pub recorded_at: String,
    pub projector_id: String,
    pub source_refs: Vec<String>,
    pub watermark: String,
    pub event_kind: String,
    pub summary: String,
    pub receipt_ref: Option<String>,
    pub harness_ref: Option<String>,
    pub act_ref: Option<String>,
    pub decision_ref: Option<String>,
    pub artifact_refs: Vec<String>,
    pub status: Option<String>,
    pub verification: Option<ReceiptVerificationProjection>,
}

#[derive(Debug, Error)]
pub enum JournalProjectionError {
    #[error(transparent)]
    ReceiptStore(#[from] ReceiptStoreError),
    #[error("invalid {field} timestamp '{value}': expected RFC 3339 timestamp")]
    InvalidTimestamp { field: &'static str, value: String },
    #[error("failed to read local run ledgers")]
    LedgerStoreUnreadable,
    #[error("failed to read local graph run state")]
    RunStateStoreUnreadable,
    #[error("terminal receipt for pending run {run_id} is unreadable")]
    TargetReceiptUnreadable { run_id: String },
}

pub fn list_local_history(
    store: &LocalReceiptStore,
    workspace_base: &Path,
    project_runx_dir: &Path,
    filter: &HistoryFilter,
) -> Result<LocalHistoryProjection, JournalProjectionError> {
    list_local_history_with_policy(
        store,
        workspace_base,
        project_runx_dir,
        filter,
        RuntimeReceiptSignaturePolicy::local_development(),
    )
}

pub fn list_local_history_with_policy(
    store: &LocalReceiptStore,
    workspace_base: &Path,
    project_runx_dir: &Path,
    filter: &HistoryFilter,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<LocalHistoryProjection, JournalProjectionError> {
    list_local_history_with_checkpoints_and_policy(
        store,
        workspace_base,
        project_runx_dir,
        filter,
        &[],
        signature_policy,
    )
}

pub fn list_local_history_with_checkpoints(
    store: &LocalReceiptStore,
    workspace_base: &Path,
    project_runx_dir: &Path,
    filter: &HistoryFilter,
    checkpoints: &[PausedRunCheckpoint],
) -> Result<LocalHistoryProjection, JournalProjectionError> {
    list_local_history_with_checkpoints_and_policy(
        store,
        workspace_base,
        project_runx_dir,
        filter,
        checkpoints,
        RuntimeReceiptSignaturePolicy::local_development(),
    )
}

pub fn list_local_history_with_checkpoints_and_policy(
    store: &LocalReceiptStore,
    workspace_base: &Path,
    project_runx_dir: &Path,
    filter: &HistoryFilter,
    checkpoints: &[PausedRunCheckpoint],
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> Result<LocalHistoryProjection, JournalProjectionError> {
    let label = safe_receipt_store_label(store.root(), workspace_base, project_runx_dir);
    let filter = ResolvedHistoryFilter::parse(filter)?;
    let all_rows = match store.list_without_proof_for_history() {
        Ok(receipts) => {
            let internal_ids = if filter.include_internal {
                BTreeSet::new()
            } else {
                internal_receipt_ids(&receipts)
            };
            receipts
                .iter()
                .filter(|receipt| filter.include_harness || !is_harness_receipt(receipt))
                .filter(|receipt| {
                    filter.include_internal || !internal_ids.contains(receipt.id.as_ref())
                })
                .map(|receipt| history_row_with_policy(receipt, signature_policy))
                .collect::<Vec<_>>()
        }
        Err(ReceiptStoreError::MissingStore { .. }) => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let mut terminal_ids = all_rows
        .iter()
        .flat_map(|row| [row.id.clone(), row.harness_id.clone()])
        .collect::<BTreeSet<_>>();
    terminal_ids.extend(terminal_graph_checkpoint_ids(store.root())?);
    let terminal_rows = all_rows.clone();
    let mut rows = all_rows
        .into_iter()
        .filter(|row| matches_history_filter(row, &filter))
        .collect::<Vec<_>>();
    let mut pending_runs = list_paused_runs(store.root(), &terminal_ids, checkpoints)?
        .into_iter()
        .filter(|pending| {
            !terminal_rows
                .iter()
                .any(|receipt| receipt_terminates_run(receipt, pending))
        })
        .filter(|row| matches_paused_history_filter(row, &filter))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    pending_runs.sort_by(|left, right| {
        compare_optional_timestamp_desc(&left.started_at, &right.started_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = filter.limit {
        truncate_combined_history(&mut rows, &mut pending_runs, limit);
    }
    Ok(LocalHistoryProjection {
        projector_id: HISTORY_PROJECTOR_ID.to_owned(),
        store_label: label.as_str().to_owned(),
        receipts: rows,
        pending_runs,
    })
}

pub fn project_journal_for_receipt(
    store: &LocalReceiptStore,
    receipt_reference: &str,
) -> Result<JournalProjection, JournalProjectionError> {
    let receipt_id = exact_receipt_id(receipt_reference);
    let receipt = store.read_exact(&receipt_id)?;
    Ok(project_receipt_journal(&receipt))
}

#[must_use]
// Function rationale: this projection assembles one sealed
// receipt into a deterministic row set; splitting it before CLI and
// paused-run sources land would obscure the ordering invariants.
pub fn project_receipt_journal(receipt: &Receipt) -> JournalProjection {
    project_receipt_journal_with_policy(receipt, RuntimeReceiptSignaturePolicy::local_development())
}

#[must_use]
// Function rationale: this projection assembles one sealed
// receipt into a deterministic row set; splitting it before CLI and
// paused-run sources land would obscure the ordering invariants.
pub fn project_receipt_journal_with_policy(
    receipt: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> JournalProjection {
    let watermark = receipt_watermark(receipt);
    let receipt_ref = receipt_uri(&receipt.id);
    let harness_ref = receipt.subject.reference.uri.clone().into_string();
    let verification = ReceiptVerificationProjection {
        status: verification_status(receipt, signature_policy),
    };
    let mut rows = receipt_lifecycle_records(
        receipt,
        &receipt_ref,
        &harness_ref,
        closure_status(&receipt.seal.disposition),
    )
    .into_iter()
    .map(|record| JournalProjectionRow {
        schema: JOURNAL_PROJECTION_SCHEMA.to_owned(),
        entry_id: format!("journal:{}:{}", receipt.id, record.entry_key),
        recorded_at: receipt.created_at.to_string(),
        projector_id: JOURNAL_PROJECTOR_ID.to_owned(),
        source_refs: record.source_refs,
        watermark: watermark.clone(),
        event_kind: record.event_kind.to_owned(),
        summary: record.summary,
        receipt_ref: Some(receipt_ref.clone()),
        harness_ref: record.harness_ref,
        act_ref: record.act_ref,
        decision_ref: record.decision_ref,
        artifact_refs: record.artifact_refs,
        status: record.status,
        verification: record.include_verification.then_some(verification.clone()),
    })
    .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        left.recorded_at
            .cmp(&right.recorded_at)
            .then_with(|| left.entry_id.cmp(&right.entry_id))
    });
    JournalProjection {
        schema: JOURNAL_PROJECTION_SCHEMA.to_owned(),
        projector_id: JOURNAL_PROJECTOR_ID.to_owned(),
        receipt_ref,
        watermark,
        rows,
    }
}

#[must_use]
pub fn receipt_uri(receipt_id: &str) -> String {
    format!("{RECEIPT_REF_PREFIX}{receipt_id}")
}

#[must_use]
pub fn exact_receipt_id(reference: &str) -> String {
    reference
        .strip_prefix(RECEIPT_REF_PREFIX)
        .unwrap_or(reference)
        .to_owned()
}

fn history_row_with_policy(
    receipt: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> LocalHistoryReceipt {
    let identity = signed_display_identity(receipt);
    LocalHistoryReceipt {
        id: receipt.id.to_string(),
        receipt_ref: receipt_uri(&receipt.id),
        name: identity.subject_ref.clone(),
        status: closure_status(&receipt.seal.disposition),
        created_at: receipt.created_at.to_string(),
        harness_id: identity.subject_ref,
        harness_state: subject_state(&receipt.subject.kind, &receipt.seal.disposition),
        summary: receipt.seal.summary.to_string(),
        source_type: Some(identity.source_type),
        actors: identity.actors,
        artifact_types: artifact_types(receipt),
        verification: ReceiptVerificationProjection {
            status: verification_status(receipt, signature_policy),
        },
    }
}

fn matches_history_filter(row: &LocalHistoryReceipt, filter: &ResolvedHistoryFilter) -> bool {
    filter.query.as_ref().is_none_or(|query| {
        row.name.to_lowercase().contains(query)
            || row.id.to_lowercase().contains(query)
            || row
                .source_type
                .as_ref()
                .is_some_and(|source| source.to_lowercase().contains(query))
            || row
                .actors
                .iter()
                .any(|actor| actor.to_lowercase().contains(query))
            || row
                .artifact_types
                .iter()
                .any(|artifact_type| artifact_type.to_lowercase().contains(query))
    }) && filter
        .skill
        .as_ref()
        .is_none_or(|skill| row.name.to_lowercase().contains(skill))
        && filter
            .status
            .as_ref()
            .is_none_or(|status| row.status.to_lowercase() == *status)
        && filter.source.as_ref().is_none_or(|source| {
            row.source_type
                .as_ref()
                .is_some_and(|candidate| candidate.to_lowercase() == *source)
        })
        && filter.actor.as_ref().is_none_or(|actor| {
            row.actors
                .iter()
                .any(|candidate| candidate.to_lowercase() == *actor)
        })
        && filter.artifact_type.as_ref().is_none_or(|artifact_type| {
            row.artifact_types
                .iter()
                .any(|candidate| candidate.to_lowercase() == *artifact_type)
        })
        && matches_timestamp_filter(row.created_at.as_str(), filter)
}

fn matches_paused_history_filter(row: &PausedRunSummary, filter: &ResolvedHistoryFilter) -> bool {
    filter.query.as_ref().is_none_or(|query| {
        row.name.to_lowercase().contains(query)
            || row.id.to_lowercase().contains(query)
            || row
                .selected_runner
                .as_ref()
                .is_some_and(|runner| runner.to_lowercase().contains(query))
    }) && filter
        .skill
        .as_ref()
        .is_none_or(|skill| row.name.to_lowercase().contains(skill))
        && filter
            .status
            .as_ref()
            .is_none_or(|status| row.status.to_lowercase() == *status)
        && filter.source.is_none()
        && filter.actor.is_none()
        && filter.artifact_type.is_none()
        && row.started_at.as_deref().map_or(
            filter.since.is_none() && filter.until.is_none(),
            |started_at| matches_timestamp_filter(started_at, filter),
        )
}

fn matches_timestamp_filter(timestamp: &str, filter: &ResolvedHistoryFilter) -> bool {
    let Some(parsed) = Timestamp::parse(timestamp) else {
        return filter.since.is_none() && filter.until.is_none();
    };
    filter.since.is_none_or(|since| parsed >= since)
        && filter.until.is_none_or(|until| parsed <= until)
}

fn normalized(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|entry| entry.trim().to_lowercase())
        .filter(|entry| !entry.is_empty())
}

fn verification_status(
    receipt: &Receipt,
    signature_policy: RuntimeReceiptSignaturePolicy<'_>,
) -> String {
    let proof_contexts = RuntimeReceiptProofContextProvider::new(signature_policy);
    let context = proof_contexts.proof_context(receipt);
    let verification = verify_receipt_proof(receipt, &context);
    // The decision -> act-id integrity property is checked inline against
    // `acts[]` by `verify_receipt`; no journal indirection remains.
    if verification.findings.is_empty() {
        if signature_policy.can_report_production_verified() {
            "verified".to_owned()
        } else {
            "unverified".to_owned()
        }
    } else if verification
        .findings
        .iter()
        .all(|finding| matches!(finding.code, ReceiptFindingCode::SignatureVerifierMissing))
    {
        "unverified".to_owned()
    } else {
        "invalid".to_owned()
    }
}

fn receipt_watermark(receipt: &Receipt) -> String {
    format!("{}@{}", receipt_uri(&receipt.id), receipt.created_at)
}

fn artifact_types(receipt: &Receipt) -> Vec<String> {
    let mut types = BTreeSet::new();
    for reference in receipt.acts.iter().flat_map(|act| act.artifact_refs.iter()) {
        if reference.reference_type == ReferenceType::Artifact {
            if let Some(label) = reference.label.as_ref().filter(|label| !label.is_empty()) {
                types.insert(label.clone());
            } else {
                types.insert("artifact".to_owned().into());
            }
        }
    }
    types.into_iter().map(|label| label.into_string()).collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResolvedHistoryFilter {
    query: Option<String>,
    skill: Option<String>,
    status: Option<String>,
    source: Option<String>,
    actor: Option<String>,
    artifact_type: Option<String>,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    limit: Option<usize>,
    include_harness: bool,
    include_internal: bool,
}

impl ResolvedHistoryFilter {
    fn parse(filter: &HistoryFilter) -> Result<Self, JournalProjectionError> {
        Ok(Self {
            query: normalized(&filter.query),
            skill: normalized(&filter.skill),
            status: normalized(&filter.status),
            source: normalized(&filter.source),
            actor: normalized(&filter.actor),
            artifact_type: normalized(&filter.artifact_type),
            since: parse_date_filter("since", &filter.since)?,
            until: parse_date_filter("until", &filter.until)?,
            limit: filter.limit,
            include_harness: filter.include_harness,
            include_internal: filter.include_internal,
        })
    }
}

fn internal_receipt_ids(receipts: &[Receipt]) -> BTreeSet<String> {
    receipts
        .iter()
        .flat_map(|receipt| receipt.lineage.as_ref())
        .flat_map(|lineage| lineage.children.iter())
        .filter_map(|reference| reference.uri.strip_prefix(RECEIPT_REF_PREFIX))
        .map(str::to_owned)
        .collect()
}

fn parse_date_filter(
    field: &'static str,
    value: &Option<String>,
) -> Result<Option<Timestamp>, JournalProjectionError> {
    let Some(value) = value
        .as_ref()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
    else {
        return Ok(None);
    };
    Timestamp::parse(value)
        .map(Some)
        .ok_or_else(|| JournalProjectionError::InvalidTimestamp {
            field,
            value: value.to_owned(),
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Timestamp {
    epoch_seconds: i64,
    nanos: u32,
}

impl Timestamp {
    fn parse(value: &str) -> Option<Self> {
        let (days, seconds_of_day, nanos) = parse_rfc3339_moment(value)?;
        let epoch_seconds = days.checked_mul(86_400)?.checked_add(seconds_of_day)?;
        Some(Self {
            epoch_seconds,
            nanos,
        })
    }
}

fn list_paused_runs(
    receipt_dir: &Path,
    terminal_ids: &BTreeSet<String>,
    checkpoints: &[PausedRunCheckpoint],
) -> Result<Vec<PausedRunSummary>, JournalProjectionError> {
    let mut summaries = Vec::new();
    summaries.extend(
        checkpoints
            .iter()
            .filter(|checkpoint| !terminal_ids.contains(checkpoint.id.as_str()))
            .map(paused_run_from_checkpoint),
    );
    let ledgers_dir = receipt_dir.join("ledgers");
    let entries = match fs::read_dir(&ledgers_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(summaries),
        Err(_) => return Err(JournalProjectionError::LedgerStoreUnreadable),
    };
    for entry in entries {
        let entry = entry.map_err(|_| JournalProjectionError::LedgerStoreUnreadable)?;
        let path = entry.path();
        let Some(run_id) = ledger_run_id(&path) else {
            continue;
        };
        if terminal_ids.contains(run_id.as_str())
            || summaries.iter().any(|summary| summary.id == run_id)
        {
            continue;
        }
        if let Some(summary) = paused_run_from_ledger(&run_id, &path)? {
            summaries.push(summary);
        }
    }
    Ok(summaries)
}

fn terminal_graph_checkpoint_ids(
    receipt_dir: &Path,
) -> Result<BTreeSet<String>, JournalProjectionError> {
    let runs_dir = receipt_dir.join("runs");
    let entries = match fs::read_dir(runs_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(_) => return Err(JournalProjectionError::RunStateStoreUnreadable),
    };
    let mut terminal = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|_| JournalProjectionError::RunStateStoreUnreadable)?;
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".graph-state.json"))
        {
            continue;
        }
        let raw = fs::read_to_string(path)
            .map_err(|_| JournalProjectionError::RunStateStoreUnreadable)?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            // A malformed state must not make a paused run disappear. The
            // actual resume path will reject it with the state-specific error.
            continue;
        };
        if value.get("schema").and_then(serde_json::Value::as_str)
            != Some("runx.graph_skill_state.v1")
        {
            continue;
        }
        let Some(run_id) = value.get("run_id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let status = value
            .pointer("/checkpoint/state/status")
            .and_then(serde_json::Value::as_str);
        if matches!(status, Some("succeeded" | "failed" | "blocked")) {
            terminal.insert(run_id.to_owned());
        }
    }
    Ok(terminal)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GraphCheckpointStatus {
    Missing,
    Active,
    Terminal,
}

fn graph_checkpoint_status_for_run(
    receipt_dir: &Path,
    run_id: &str,
) -> Result<GraphCheckpointStatus, JournalProjectionError> {
    let path = receipt_dir
        .join("runs")
        .join(format!("{run_id}.graph-state.json"));
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(GraphCheckpointStatus::Missing);
        }
        Err(_) => return Err(JournalProjectionError::RunStateStoreUnreadable),
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        // Resume owns the state-specific diagnostic. A malformed checkpoint
        // must not make the corresponding pending run disappear here.
        return Ok(GraphCheckpointStatus::Active);
    };
    if value.get("schema").and_then(serde_json::Value::as_str) != Some("runx.graph_skill_state.v1")
        || value.get("run_id").and_then(serde_json::Value::as_str) != Some(run_id)
    {
        return Ok(GraphCheckpointStatus::Active);
    }
    if value
        .get("completed_receipt_id")
        .and_then(serde_json::Value::as_str)
        .is_some()
    {
        return Ok(GraphCheckpointStatus::Terminal);
    }
    let status = value
        .pointer("/checkpoint/state/status")
        .and_then(serde_json::Value::as_str);
    Ok(
        if matches!(status, Some("succeeded" | "failed" | "blocked")) {
            GraphCheckpointStatus::Terminal
        } else {
            GraphCheckpointStatus::Active
        },
    )
}

fn terminal_receipt_exists_for_run(
    receipt_dir: &Path,
    pending: &PausedRunSummary,
) -> Result<bool, JournalProjectionError> {
    let entries = match fs::read_dir(receipt_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(JournalProjectionError::LedgerStoreUnreadable),
    };
    for entry in entries {
        let entry = entry.map_err(|_| JournalProjectionError::LedgerStoreUnreadable)?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let raw =
            fs::read_to_string(path).map_err(|_| JournalProjectionError::LedgerStoreUnreadable)?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let receipt_id = value.get("id").and_then(serde_json::Value::as_str);
        let harness_id = value
            .pointer("/subject/ref/uri")
            .and_then(serde_json::Value::as_str);
        if !receipt_identity_terminates_run(receipt_id, harness_id, pending) {
            continue;
        }
        serde_json::from_value::<Receipt>(value).map_err(|_| {
            JournalProjectionError::TargetReceiptUnreadable {
                run_id: pending.id.clone(),
            }
        })?;
        return Ok(true);
    }
    Ok(false)
}

fn paused_run_from_checkpoint(checkpoint: &PausedRunCheckpoint) -> PausedRunSummary {
    PausedRunSummary {
        id: checkpoint.id.clone(),
        name: checkpoint.name.clone(),
        kind: checkpoint.kind.clone(),
        status: "paused".to_owned(),
        started_at: checkpoint.started_at.clone(),
        resume_skill_ref: checkpoint.resume_skill_ref.clone(),
        selected_runner: checkpoint.selected_runner.clone(),
        credential_profile: checkpoint.credential_profile.clone(),
        package_digest: checkpoint.package_digest.clone(),
        execution_closure_digest: checkpoint.execution_closure_digest.clone(),
        step_ids: checkpoint.step_ids.clone(),
        step_labels: checkpoint.step_labels.clone(),
        ledger_verification: None,
    }
}

fn ledger_run_id(path: &Path) -> Option<String> {
    if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
        return None;
    }
    let run_id = path.file_stem()?.to_str()?;
    if !valid_run_id(run_id) {
        return None;
    }
    Some(run_id.to_owned())
}

fn valid_run_id(run_id: &str) -> bool {
    (run_id.starts_with("rx_") || run_id.starts_with("gx_") || run_id.starts_with("run_"))
        && run_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn paused_run_from_ledger(
    run_id: &str,
    path: &Path,
) -> Result<Option<PausedRunSummary>, JournalProjectionError> {
    let contents =
        fs::read_to_string(path).map_err(|_| JournalProjectionError::LedgerStoreUnreadable)?;
    if let Err(error) = verified_ledger_head(&contents, run_id) {
        return Ok(Some(invalid_paused_run(run_id, error.to_string())));
    }
    let mut events = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<LedgerLine>(line) {
            Ok(value) => value,
            Err(error) => {
                return Ok(Some(invalid_paused_run(
                    run_id,
                    format!("line {} is not valid JSON: {error}", index + 1),
                )));
            }
        };
        if let Some(event) = ledger_event(value) {
            events.push(event);
        }
    }
    Ok(paused_run_from_events(run_id, &events))
}

#[derive(Clone, Debug, Deserialize)]
struct LedgerLine {
    entry: LedgerEntry,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LedgerEntry {
    #[serde(rename = "type")]
    entry_type: String,
    data: LedgerEventData,
    meta: LedgerEventMeta,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LedgerEventData {
    kind: String,
    #[serde(default)]
    detail: LedgerEventDetail,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LedgerEventDetail {
    #[serde(default)]
    resume_skill_ref: Option<String>,
    #[serde(default)]
    selected_runner: Option<String>,
    #[serde(default)]
    credential_profile: Option<String>,
    #[serde(default)]
    package_digest: Option<String>,
    #[serde(default)]
    execution_closure_digest: Option<String>,
    #[serde(default)]
    step_ids: Vec<String>,
    #[serde(default)]
    step_labels: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LedgerEventMeta {
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    producer: Option<LedgerEventProducer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LedgerEventProducer {
    #[serde(default)]
    skill: Option<String>,
    #[serde(default)]
    runner: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LedgerRunEvent {
    kind: String,
    created_at: Option<String>,
    skill_name: Option<String>,
    runner: Option<String>,
    resume_skill_ref: Option<String>,
    selected_runner: Option<String>,
    credential_profile: Option<String>,
    package_digest: Option<String>,
    execution_closure_digest: Option<String>,
    step_ids: Vec<String>,
    step_labels: Vec<String>,
}

fn ledger_event(value: LedgerLine) -> Option<LedgerRunEvent> {
    let entry = value.entry;
    if entry.entry_type != "run_event" {
        return None;
    }
    let producer = entry.meta.producer;
    Some(LedgerRunEvent {
        kind: entry.data.kind,
        created_at: entry.meta.created_at,
        skill_name: producer.as_ref().and_then(|value| value.skill.clone()),
        runner: producer.and_then(|value| value.runner),
        resume_skill_ref: entry.data.detail.resume_skill_ref,
        selected_runner: entry.data.detail.selected_runner,
        credential_profile: entry.data.detail.credential_profile,
        package_digest: entry.data.detail.package_digest,
        execution_closure_digest: entry.data.detail.execution_closure_digest,
        step_ids: clean_string_array(entry.data.detail.step_ids),
        step_labels: clean_string_array(entry.data.detail.step_labels),
    })
}

fn paused_run_from_events(run_id: &str, events: &[LedgerRunEvent]) -> Option<PausedRunSummary> {
    let mut started_at = None;
    for event in events {
        if event.kind == "run_started" {
            started_at = event.created_at.clone();
        }
    }
    for event in events.iter().rev() {
        if matches!(
            event.kind.as_str(),
            "run_completed"
                | "run_failed"
                | "run_blocked"
                | "graph_completed"
                | "graph_failed"
                | "graph_blocked"
        ) {
            return None;
        }
        if matches!(
            event.kind.as_str(),
            "resolution_requested" | "step_waiting_resolution"
        ) {
            return Some(PausedRunSummary {
                id: run_id.to_owned(),
                name: event
                    .skill_name
                    .clone()
                    .unwrap_or_else(|| run_id.to_owned()),
                kind: RUN_KIND.to_owned(),
                status: "paused".to_owned(),
                started_at: started_at.or_else(|| event.created_at.clone()),
                resume_skill_ref: event.resume_skill_ref.clone(),
                selected_runner: event
                    .selected_runner
                    .clone()
                    .or_else(|| event.runner.clone()),
                credential_profile: event.credential_profile.clone(),
                package_digest: event.package_digest.clone(),
                execution_closure_digest: event.execution_closure_digest.clone(),
                step_ids: event.step_ids.clone(),
                step_labels: event.step_labels.clone(),
                ledger_verification: Some(LedgerVerificationProjection {
                    status: "valid".to_owned(),
                    reason: None,
                }),
            });
        }
    }
    None
}

fn is_harness_receipt(receipt: &Receipt) -> bool {
    receipt.created_at.as_ref() == crate::time::DEFAULT_CREATED_AT
        || matches!(receipt.subject.kind.as_ref(), "harness" | "trial")
}

fn receipt_terminates_run(receipt: &LocalHistoryReceipt, pending: &PausedRunSummary) -> bool {
    receipt_identity_terminates_run(Some(&receipt.id), Some(&receipt.harness_id), pending)
}

fn receipt_identity_terminates_run(
    receipt_id: Option<&str>,
    harness_id: Option<&str>,
    pending: &PausedRunSummary,
) -> bool {
    let run_id = receipt_identity_segment(&pending.id);
    if receipt_id == Some(format!("hrn_rcpt_{run_id}").as_str())
        || harness_id == Some(format!("hrn_{run_id}_graph").as_str())
    {
        return true;
    }
    let Some(runner) = pending.selected_runner.as_deref() else {
        return false;
    };
    let runner = receipt_identity_segment(runner);
    receipt_id == Some(format!("hrn_rcpt_{run_id}_{runner}").as_str())
        || harness_id == Some(format!("hrn_{run_id}_{runner}").as_str())
}

fn receipt_identity_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else if character == '.' {
                '-'
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches(['.', '_', '-'])
        .to_owned()
}

fn truncate_combined_history(
    receipts: &mut Vec<LocalHistoryReceipt>,
    pending: &mut Vec<PausedRunSummary>,
    limit: usize,
) {
    while receipts.len().saturating_add(pending.len()) > limit {
        if receipts.is_empty() {
            pending.pop();
            continue;
        }
        if pending.is_empty() {
            receipts.pop();
            continue;
        }

        let receipt_time = receipts
            .last()
            .and_then(|receipt| Timestamp::parse(&receipt.created_at));
        let pending_time = pending
            .last()
            .and_then(|run| run.started_at.as_deref())
            .and_then(Timestamp::parse);
        match (receipt_time, pending_time) {
            (None, None) => {
                pending.pop();
            }
            (None, Some(_)) => {
                receipts.pop();
            }
            (Some(_), None) => {
                pending.pop();
            }
            (Some(receipt_time), Some(pending_time)) if receipt_time <= pending_time => {
                receipts.pop();
            }
            (Some(_), Some(_)) => {
                pending.pop();
            }
        }
    }
}

fn invalid_paused_run(run_id: &str, reason: String) -> PausedRunSummary {
    PausedRunSummary {
        id: run_id.to_owned(),
        name: run_id.to_owned(),
        kind: RUN_KIND.to_owned(),
        status: "paused".to_owned(),
        started_at: None,
        resume_skill_ref: None,
        selected_runner: None,
        credential_profile: None,
        package_digest: None,
        execution_closure_digest: None,
        step_ids: Vec::new(),
        step_labels: Vec::new(),
        ledger_verification: Some(LedgerVerificationProjection {
            status: "invalid".to_owned(),
            reason: Some(reason),
        }),
    }
}

const RUN_KIND: &str = "runx.receipt.v1";

fn clean_string_array(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .filter(|item| !item.trim().is_empty())
        .collect()
}

fn compare_optional_timestamp_desc(
    left: &Option<String>,
    right: &Option<String>,
) -> std::cmp::Ordering {
    match (
        left.as_deref().and_then(Timestamp::parse),
        right.as_deref().and_then(Timestamp::parse),
    ) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn subject_state(_kind: &NonEmptyString, disposition: &ClosureDisposition) -> String {
    if matches!(disposition, ClosureDisposition::Closed) {
        return "sealed".to_owned();
    }
    closure_status(disposition)
}

fn closure_status(disposition: &ClosureDisposition) -> String {
    disposition.label().to_owned()
}
