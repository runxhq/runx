use std::path::PathBuf;

use runx_contracts::JsonObject;
use runx_core::state_machine::GraphStatus;
use runx_runtime::adapters::cli_tool::CliToolAdapter;
use runx_runtime::{Runtime, RuntimeOptions};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedSummary {
    graph_name: String,
    state: String,
    step_ids: Vec<String>,
    contracts: Vec<JsonObject>,
    created_at: String,
    graph_seal_digest: String,
    child_seal_digests: Vec<String>,
    enforcement_profile_hash: String,
    graph_receipt_id: String,
    child_receipt_ids: Vec<String>,
}

#[test]
fn hello_graph_matches_post_cutover_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let expected: ExpectedSummary = serde_json::from_str(include_str!(
        "../../../../fixtures/runtime/hello-graph/summary.json"
    ))?;
    let runtime = Runtime::new(
        CliToolAdapter,
        RuntimeOptions {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            created_at: expected.created_at.clone(),
            ..RuntimeOptions::local_development(std::env::vars().collect())
        },
    );
    let graph_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/hello-graph/graph.yaml")
        .canonicalize()?;
    let run = runtime.run_graph_file(&graph_path)?;

    assert_eq!(run.graph.name, expected.graph_name);
    assert_eq!(status_name(&run.state.status), expected.state);
    assert_eq!(
        run.steps
            .iter()
            .map(|step| step.step_id.clone())
            .collect::<Vec<_>>(),
        expected.step_ids
    );
    assert_eq!(
        run.steps
            .iter()
            .map(|step| step.contract.clone())
            .collect::<Vec<_>>(),
        expected.contracts
    );
    assert_eq!(run.receipt.created_at, expected.created_at);
    if std::env::var("RUNX_REGEN_FIXTURES").is_ok() {
        eprintln!(
            "REGEN-HELLO graph_seal_digest={} child_seal_digests={:?} enforcement_profile_hash={} graph_receipt_id={} child_receipt_ids={:?}",
            run.receipt.digest,
            run.steps
                .iter()
                .map(|s| s.receipt.digest.clone())
                .collect::<Vec<_>>(),
            run.receipt.authority.enforcement.profile_hash,
            run.receipt.id,
            run.steps
                .iter()
                .map(|s| s.receipt.id.clone())
                .collect::<Vec<_>>(),
        );
        return Ok(());
    }
    assert_eq!(run.receipt.digest, expected.graph_seal_digest);
    assert_eq!(
        run.receipt.authority.enforcement.profile_hash,
        expected.enforcement_profile_hash
    );
    for step in &run.steps {
        assert_eq!(step.receipt.created_at, expected.created_at);
        assert_eq!(
            step.receipt.authority.enforcement.profile_hash,
            expected.enforcement_profile_hash
        );
    }
    assert_eq!(
        run.steps
            .iter()
            .map(|step| step.receipt.digest.clone())
            .collect::<Vec<_>>(),
        expected.child_seal_digests
    );
    assert_eq!(run.receipt.id, expected.graph_receipt_id);
    assert_eq!(
        run.steps
            .iter()
            .map(|step| step.receipt.id.clone())
            .collect::<Vec<_>>(),
        expected.child_receipt_ids
    );
    Ok(())
}

fn status_name(status: &GraphStatus) -> &'static str {
    match status {
        GraphStatus::Pending => "pending",
        GraphStatus::Running => "running",
        GraphStatus::Succeeded => "succeeded",
        GraphStatus::Failed => "failed",
        GraphStatus::Paused => "paused",
        GraphStatus::Escalated => "escalated",
    }
}
