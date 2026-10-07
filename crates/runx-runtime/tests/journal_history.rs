use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::{Receipt, ReceiptIssuerType, Reference, ReferenceType};
use runx_runtime::journal::{
    HISTORY_PROJECTOR_ID, HistoryFilter, JOURNAL_PROJECTOR_ID, JournalProjectionError,
    PausedRunCheckpoint, RECEIPT_REF_PREFIX, append_paused_run_checkpoint, exact_receipt_id,
    find_paused_run, inspect_local_receipt, list_local_history,
    list_local_history_with_checkpoints, list_local_history_with_policy,
    project_journal_for_receipt, project_receipt_journal, project_receipt_journal_with_policy,
    receipt_uri,
};
use runx_runtime::receipts::{
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, RuntimeReceiptSignaturePolicy,
    step_receipt_with_signature_policy,
};
use runx_runtime::{InvocationOutput, InvocationStatus, LocalReceiptStore};
use serde_json::json;

const JOURNAL_ORACLE: &str = include_str!("../../../fixtures/journal/history-oracle.json");
const SIGNED_LOCAL_ACTOR: &str = "runx:principal:local_runtime";
const SIGNED_RUNTIME_SUBJECT: &str = "hrn_journal-history_strict-proof";

#[test]
fn missing_history_store_projects_empty_safe_result() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
    )?;

    assert_eq!(history.projector_id, HISTORY_PROJECTOR_ID);
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
        "hrn_rcpt_old",
        "2026-05-18T00:00:01Z",
        "Revision Skill",
        "local",
        "runner-a",
    )?;
    let new_receipt = receipt_with_metadata(
        InvocationStatus::Success,
        "hrn_rcpt_new",
        "2026-05-19T00:00:00Z",
        "Deploy Skill",
        "local",
        "runner-b",
    )?;
    let expected_order = vec![receipt_uri(&new_receipt.id), receipt_uri(&old_receipt.id)];
    store.write_receipt(&old_receipt)?;
    store.write_receipt(&new_receipt)?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            query: Some("artifact".to_owned()),
            source: Some("LOCAL".to_owned()),
            since: Some("2026-05-18T00:00:00Z".to_owned()),
            limit: Some(2),
            ..HistoryFilter::default()
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
    assert_eq!(history.receipts[0].name, SIGNED_RUNTIME_SUBJECT);
    assert_eq!(history.receipts[0].source_type.as_deref(), Some("local"));
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
        "hrn_rcpt_forged_metadata",
        "2026-05-18T00:00:01Z",
        "Forged Skill",
        "forged-source",
        "forged-runner",
    )?)?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
    )?;

    assert_eq!(history.receipts[0].name, SIGNED_RUNTIME_SUBJECT);
    assert_eq!(history.receipts[0].harness_id, SIGNED_RUNTIME_SUBJECT);
    assert_eq!(history.receipts[0].source_type.as_deref(), Some("local"));
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
        "hrn_rcpt_revision",
        "2026-05-18T00:01:00Z",
        "Revision Skill",
        "local",
        "runner-a",
    )?;
    let revision_id = revision.id.to_string();
    store.write_receipt(&revision)?;
    store.write_receipt(&receipt_with_metadata(
        InvocationStatus::Failure,
        "hrn_rcpt_deploy",
        "2026-05-19T00:01:00Z",
        "Deploy Skill",
        "local",
        "runner-b",
    )?)?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            skill: Some("journal-history".to_owned()),
            status: Some("closed".to_owned()),
            actor: Some(SIGNED_LOCAL_ACTOR.to_owned()),
            until: Some("2026-05-18T23:59:59Z".to_owned()),
            ..HistoryFilter::default()
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
        "hrn_rcpt_matching",
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
        "hrn_rcpt_wrong_artifact",
        "2026-05-18T12:30:00Z",
        "Deploy Skill",
        "local",
        "runner-a",
    )?;
    set_artifact_label(&mut wrong_artifact, "diagnostic-log")?;
    store.write_receipt(&wrong_artifact)?;

    let mut outside_window = receipt_with_metadata(
        InvocationStatus::Success,
        "hrn_rcpt_outside_window",
        "2026-05-19T00:00:01Z",
        "Deploy Skill",
        "local",
        "runner-a",
    )?;
    set_artifact_label(&mut outside_window, "deploy-bundle")?;
    store.write_receipt(&outside_window)?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            skill: Some("journal-history".to_owned()),
            status: Some("CLOSED".to_owned()),
            source: Some("LOCAL".to_owned()),
            artifact_type: Some("deploy-bundle".to_owned()),
            since: Some("2026-05-18T00:00:00Z".to_owned()),
            until: Some("2026-05-19T00:00:00Z".to_owned()),
            ..HistoryFilter::default()
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
    let oracle: serde_json::Value = serde_json::from_str(JOURNAL_ORACLE)?;
    let invalid_date = oracle
        .get("invalid_date_filter")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing invalid_date_filter"))?;
    let field = invalid_date
        .get("field")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing invalid date field"))?;
    let value = invalid_date
        .get("value")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing invalid date value"))?;

    let result = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            since: Some(value.to_owned()),
            ..HistoryFilter::default()
        },
    );

    assert!(matches!(
        result,
        Err(JournalProjectionError::InvalidTimestamp { field: actual, .. }) if actual == field
    ));
    Ok(())
}

#[test]
fn history_merges_paused_ledgers_and_checkpoints() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    write_paused_ledger(
        store.root(),
        "gx_paused0000000000000000000000ab",
        "sourcey",
        "2026-04-28T01:00:00.000Z",
    )?;

    let history = list_local_history_with_checkpoints(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            status: Some("paused".to_owned()),
            since: Some("2026-04-28T00:00:00Z".to_owned()),
            until: Some("2026-04-28T02:00:00+01:00".to_owned()),
            ..HistoryFilter::default()
        },
        &[PausedRunCheckpoint {
            id: "rx_checkpoint00000000000000000001".to_owned(),
            name: "checkpoint-skill".to_owned(),
            kind: "runx.receipt.v1".to_owned(),
            started_at: Some("2026-04-28T00:30:00Z".to_owned()),
            resume_skill_ref: None,
            selected_runner: Some("agent-task".to_owned()),
            package_digest: Some("sha256:checkpoint-package".to_owned()),
            execution_closure_digest: Some("sha256:checkpoint-closure".to_owned()),
            step_ids: vec!["plan".to_owned()],
            step_labels: vec!["plan work".to_owned()],
        }],
    )?;
    let oracle: serde_json::Value = serde_json::from_str(JOURNAL_ORACLE)?;
    let paused = oracle
        .get("paused_run")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing paused_run"))?;
    let expected_id = paused
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing paused id"))?;
    let expected_name = paused
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing paused name"))?;
    let expected_runner = paused
        .get("selected_runner")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing paused runner"))?;

    assert!(history.receipts.is_empty());
    assert_eq!(
        history
            .pending_runs
            .iter()
            .map(|run| run.id.as_str())
            .collect::<Vec<_>>(),
        vec![expected_id, "rx_checkpoint00000000000000000001",]
    );
    assert_eq!(history.pending_runs[0].name, expected_name);
    assert_eq!(
        history.pending_runs[0].selected_runner.as_deref(),
        Some(expected_runner)
    );
    assert_eq!(history.pending_runs[0].step_ids, vec!["discover"]);
    assert_eq!(history.pending_runs[1].step_labels, vec!["plan work"]);
    assert_eq!(
        history.pending_runs[1].package_digest.as_deref(),
        Some("sha256:checkpoint-package")
    );
    assert_eq!(
        history.pending_runs[1].execution_closure_digest.as_deref(),
        Some("sha256:checkpoint-closure")
    );
    assert_no_local_paths(&serde_json::to_string(&history)?);
    Ok(())
}

#[test]
fn paused_checkpoint_ledger_chains_appends_and_fails_closed_after_tamper()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let run_id = "run_chained_checkpoint";
    write_paused_ledger(store.root(), run_id, "sourcey", "2026-04-28T01:00:00.000Z")?;
    write_paused_ledger(store.root(), run_id, "sourcey", "2026-04-28T01:01:00.000Z")?;

    let ledger_path = store.root().join("ledgers").join(format!("{run_id}.jsonl"));
    let contents = fs::read_to_string(&ledger_path)?;
    let mut records = contents
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["chain"]["index"], 0);
    assert_eq!(records[1]["chain"]["index"], 1);
    assert_eq!(
        records[1]["chain"]["previous_hash"],
        records[0]["chain"]["entry_hash"]
    );

    records[0]["entry"]["data"]["detail"]["step_labels"] = json!(["tampered"]);
    fs::write(
        &ledger_path,
        records
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?
            .join("\n")
            + "\n",
    )?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
    )?;
    assert_eq!(history.pending_runs.len(), 1);
    assert_eq!(
        history.pending_runs[0]
            .ledger_verification
            .as_ref()
            .map(|verification| verification.status.as_str()),
        Some("invalid")
    );
    Ok(())
}

#[test]
fn paused_run_point_lookup_ignores_unrelated_receipt_archive_shape()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let receipt_dir = temp.path().join("receipts");
    let run_id = "run_resume_target";
    write_paused_ledger(
        &receipt_dir,
        run_id,
        "github-sync",
        "2026-08-26T01:00:00.000Z",
    )?;
    fs::write(
        receipt_dir.join("sha256-unrelated.json"),
        br#"{"schema":"runx.receipt.v1","id":"sha256:unrelated"}"#,
    )?;

    let pending = find_paused_run(&receipt_dir, run_id)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing paused run"))?;

    assert_eq!(pending.id, run_id);
    assert_eq!(pending.name, "github-sync");
    assert_eq!(pending.selected_runner.as_deref(), Some("agent-task"));
    Ok(())
}

#[test]
fn paused_run_point_lookup_stops_at_its_terminal_receipt() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let run_id = "run_resume_terminal";
    write_paused_ledger(
        store.root(),
        run_id,
        "github-sync",
        "2026-08-26T01:00:00.000Z",
    )?;
    let mut terminal = receipt_with_metadata(
        InvocationStatus::Success,
        run_id,
        "2026-08-26T01:01:00.000Z",
        "GitHub sync",
        "local",
        "agent-task",
    )?;
    terminal.subject.reference.uri = format!("hrn_{run_id}_agent-task").into();
    reseal_receipt(&mut terminal)?;
    store.write_receipt(&terminal)?;

    assert!(find_paused_run(store.root(), run_id)?.is_none());
    Ok(())
}

#[test]
fn history_does_not_double_list_paused_ledger_with_terminal_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    write_paused_ledger(
        store.root(),
        "gx_paused_terminal",
        "sourcey",
        "2026-04-28T01:00:00.000Z",
    )?;
    let mut terminal = receipt_with_metadata(
        InvocationStatus::Success,
        "gx_paused_terminal",
        "2026-04-28T02:00:00Z",
        "Terminal Skill",
        "local",
        "runner-a",
    )?;
    terminal.subject.reference.uri = "gx_paused_terminal".into();
    reseal_receipt(&mut terminal)?;
    store.write_receipt(&terminal)?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
    )?;

    assert_eq!(history.receipts.len(), 1);
    assert!(history.pending_runs.is_empty());
    Ok(())
}

#[test]
fn terminal_graph_checkpoint_supersedes_a_stale_resolution_ledger()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let run_id = "run_spend_completed";
    write_paused_ledger(store.root(), run_id, "spend", "2026-05-18T01:00:00.000Z")?;
    let runs_dir = store.root().join("runs");
    fs::create_dir_all(&runs_dir)?;
    fs::write(
        runs_dir.join(format!("{run_id}.graph-state.json")),
        serde_json::to_vec(&json!({
            "schema": "runx.graph_skill_state.v1",
            "run_id": run_id,
            "checkpoint": {"state": {"status": "succeeded"}}
        }))?,
    )?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
    )?;

    assert!(history.pending_runs.is_empty());
    Ok(())
}

#[test]
fn journal_history_limit_bounds_receipts_and_pending_runs_together()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    store.write_receipt(&receipt_with_metadata(
        InvocationStatus::Success,
        "terminal-old",
        "2026-05-18T01:00:00Z",
        "Old terminal",
        "local",
        "runner-a",
    )?)?;
    store.write_receipt(&receipt_with_metadata(
        InvocationStatus::Success,
        "terminal-new",
        "2026-05-18T04:00:00Z",
        "New terminal",
        "local",
        "runner-a",
    )?)?;
    write_paused_ledger(
        store.root(),
        "run_pending_old",
        "old-pending",
        "2026-05-18T02:00:00Z",
    )?;
    write_paused_ledger(
        store.root(),
        "run_pending_new",
        "new-pending",
        "2026-05-18T03:00:00Z",
    )?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            limit: Some(2),
            ..HistoryFilter::default()
        },
    )?;

    assert_eq!(history.receipts.len() + history.pending_runs.len(), 2);
    assert_eq!(history.receipts.len(), 1);
    assert_eq!(history.pending_runs.len(), 1);
    assert_eq!(history.pending_runs[0].id, "run_pending_new");
    Ok(())
}

#[test]
fn journal_history_limit_bounds_terminal_receipts_when_no_runs_are_pending()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    for (id, created_at) in [
        ("terminal-old", "2026-05-18T01:00:00Z"),
        ("terminal-middle", "2026-05-18T02:00:00Z"),
        ("terminal-new", "2026-05-18T03:00:00Z"),
    ] {
        store.write_receipt(&receipt_with_metadata(
            InvocationStatus::Success,
            id,
            created_at,
            id,
            "local",
            "runner-a",
        )?)?;
    }

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            limit: Some(1),
            ..HistoryFilter::default()
        },
    )?;

    assert_eq!(history.receipts.len(), 1);
    assert!(history.pending_runs.is_empty());
    Ok(())
}

#[test]
fn journal_history_limit_bounds_pending_runs_when_no_receipts_exist()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    for (run_id, started_at) in [
        ("run_pending_old", "2026-05-18T01:00:00Z"),
        ("run_pending_middle", "2026-05-18T02:00:00Z"),
        ("run_pending_new", "2026-05-18T03:00:00Z"),
    ] {
        write_paused_ledger(store.root(), run_id, run_id, started_at)?;
    }

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            limit: Some(1),
            ..HistoryFilter::default()
        },
    )?;

    assert!(history.receipts.is_empty());
    assert_eq!(history.pending_runs.len(), 1);
    Ok(())
}

#[test]
fn journal_terminal_graph_receipt_supersedes_stale_resolution_request()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let run_id = "run_issue_442";
    write_paused_ledger(store.root(), run_id, "issue-to-pr", "2026-05-18T01:00:00Z")?;
    let mut terminal = receipt_with_metadata(
        InvocationStatus::Success,
        "terminal-graph",
        "2026-05-18T02:00:00Z",
        "issue-to-pr",
        "local",
        "graph",
    )?;
    terminal.subject.reference.uri = format!("hrn_{run_id}_graph").into();
    reseal_receipt(&mut terminal)?;
    store.write_receipt(&terminal)?;

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
    )?;

    assert_eq!(history.receipts.len(), 1);
    assert!(history.pending_runs.is_empty());
    Ok(())
}

#[test]
fn journal_history_excludes_harness_receipts_unless_requested()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let workspace = temp.path().join("workspace");
    let project_runx_dir = workspace.join(".runx");
    let store = LocalReceiptStore::new(project_runx_dir.join("receipts"));
    let harness = receipt_with_metadata(
        InvocationStatus::Success,
        "harness-only",
        "2026-05-18T00:00:00Z",
        "fixture",
        "local",
        "harness",
    )?;
    store.write_receipt(&harness)?;

    let default_history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
    )?;
    let harness_history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter {
            include_harness: true,
            ..HistoryFilter::default()
        },
    )?;

    assert!(default_history.receipts.is_empty());
    assert_eq!(harness_history.receipts.len(), 1);
    Ok(())
}

#[test]
fn malformed_history_store_remains_typed_error() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path());
    fs::create_dir_all(temp.path())?;
    fs::write(store.receipt_path(&valid_content_receipt_id())?, "{")?;

    let result = list_local_history(
        &store,
        temp.path(),
        &temp.path().join(".runx"),
        &HistoryFilter::default(),
    );

    assert!(matches!(
        result,
        Err(JournalProjectionError::ReceiptStore(
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

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
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

    let history = list_local_history(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
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

    let history = list_local_history_with_policy(
        &store,
        &workspace,
        &project_runx_dir,
        &HistoryFilter::default(),
        RuntimeReceiptSignaturePolicy::production(&verifier),
    )?;
    let journal = project_receipt_journal_with_policy(
        &receipt,
        RuntimeReceiptSignaturePolicy::production(&verifier),
    );

    assert_eq!(history.receipts[0].verification.status, "verified");
    assert_eq!(
        receipt_journal_verification_status(&journal),
        Some("verified")
    );
    Ok(())
}

#[test]
fn journal_projection_uses_exact_refs_and_reprojects_deterministically()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path().join("receipts"));
    let receipt = receipt_with_metadata(
        InvocationStatus::Success,
        "hrn_rcpt_123",
        "2026-05-18T00:00:01Z",
        "Journal Skill",
        "local",
        "runner-a",
    )?;
    let receipt_id = receipt.id.to_string();
    let expected_ref = receipt_uri(&receipt.id);
    store.write_receipt(&receipt)?;

    let direct = project_journal_for_receipt(&store, &receipt_id)?;
    let typed = project_journal_for_receipt(&store, &expected_ref)?;
    let reprojected = project_receipt_journal(&receipt);
    let oracle: serde_json::Value = serde_json::from_str(JOURNAL_ORACLE)?;
    let expected_projector = oracle
        .get("projector_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing projector_id"))?;

    assert_eq!(direct, typed);
    assert_eq!(direct, reprojected);
    assert_eq!(direct.projector_id, JOURNAL_PROJECTOR_ID);
    assert_eq!(direct.projector_id, expected_projector);
    assert_eq!(direct.receipt_ref, expected_ref);
    assert!(direct.rows.iter().all(|row| {
        row.source_refs
            .iter()
            .any(|source_ref| source_ref == &expected_ref)
    }));
    assert!(
        direct
            .rows
            .iter()
            .all(|row| row.watermark == direct.watermark)
    );
    assert_no_local_paths(&serde_json::to_string(&direct)?);
    Ok(())
}

#[test]
fn journal_lookup_does_not_use_suffix_matching() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDir::new()?;
    let store = LocalReceiptStore::new(temp.path().join("receipts"));
    let receipt = receipt_with_metadata(
        InvocationStatus::Success,
        "hrn_rcpt_123",
        "2026-05-18T00:00:01Z",
        "Journal Skill",
        "local",
        "runner-a",
    )?;
    let receipt_id = receipt.id.to_string();
    let receipt_ref = receipt_uri(&receipt.id);
    store.write_receipt(&receipt)?;

    let result = project_journal_for_receipt(&store, "123");

    assert!(matches!(
        result,
        Err(JournalProjectionError::ReceiptStore(
            runx_runtime::ReceiptStoreError::MissingReceipt { .. }
        ))
    ));
    assert_eq!(exact_receipt_id(&receipt_ref), receipt_id);
    assert_eq!(receipt_uri(&receipt.id), receipt_ref);
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
        "hrn_rcpt_journal-history_strict-proof",
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
        "journal-history",
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

const FIXTURE_KID: &str = "runx-runtime-prod-fixture-key";
const FIXTURE_SEED: [u8; 32] = [0x42; 32];

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
        "journal-history",
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

fn receipt_journal_verification_status(
    projection: &runx_runtime::journal::JournalProjection,
) -> Option<&str> {
    projection
        .rows
        .iter()
        .find(|row| row.event_kind == "receipt_sealed")
        .and_then(|row| row.verification.as_ref())
        .map(|verification| verification.status.as_str())
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

fn write_paused_ledger(
    receipt_dir: &Path,
    run_id: &str,
    skill_name: &str,
    created_at: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    append_paused_run_checkpoint(
        receipt_dir,
        &PausedRunCheckpoint {
            id: run_id.to_owned(),
            name: skill_name.to_owned(),
            kind: "graph".to_owned(),
            started_at: Some(created_at.to_owned()),
            resume_skill_ref: Some("registry:sourcey@sha256:test".to_owned()),
            selected_runner: Some("agent-task".to_owned()),
            package_digest: Some("sha256:package".to_owned()),
            execution_closure_digest: Some("sha256:closure".to_owned()),
            step_ids: vec!["discover".to_owned()],
            step_labels: vec!["inspect repo".to_owned()],
        },
    )?;
    Ok(())
}

fn assert_no_local_paths(text: &str) {
    assert!(!text.contains("/Users/"));
    assert!(!text.contains("/private/"));
    assert!(!text.contains("runx-runtime-journal-history"));
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
            "runx-runtime-journal-history-{}-{serial}-{nanos}",
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
