use std::fs;
use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use runx_core::state_machine::{
    GraphStatus, SequentialGraphEvent, SequentialGraphPlan, SequentialGraphStepDefinition,
    SequentialGraphStepIndex, StepAdmissionWitness, apply_sequential_graph_event_owned_indexed,
    create_sequential_graph_state, create_sequential_graph_step_index,
    plan_sequential_graph_transition_indexed_from, start_sequential_graph_step_indexed,
    succeed_sequential_graph_step_indexed,
};
use runx_runtime::{
    EphemeralValue, InvocationOutput, RuntimeReceiptSignatureConfig, StepRun,
    receipts::{graph_receipt_with_signature_policy, step_receipt_with_signature_policy},
};
use tempfile::TempDir;

#[path = "graph_throughput/runtime_paths.rs"]
mod runtime_paths;
#[path = "graph_throughput/volume_paths.rs"]
mod volume_paths;

const CREATED_AT: &str = "2026-05-26T00:00:00Z";
const SCOPE: runx_runtime::receipts::RunScope<'static> = runx_runtime::receipts::RunScope {
    run_id: "run_throughput",
    graph_name: "throughput_graph",
};
const RECEIPT_STORE_SCALE_SMALL: usize = 16;
const RECEIPT_STORE_SCALE_LARGE: usize = 128;

fn bench_graph_throughput(c: &mut Criterion) {
    runtime_paths::register(c);
    volume_paths::register(c);

    c.bench_function("graph_planning", |b| {
        let steps = sequential_steps(192);
        let step_index = create_sequential_graph_step_index(&steps);
        b.iter(|| drive_state_machine(black_box(&steps), black_box(&step_index)))
    });

    c.bench_function("graph_receipt_sealing", |b| {
        let signature = RuntimeReceiptSignatureConfig::local_development();
        let template = synthetic_step_runs(&signature, 32);
        b.iter(|| {
            let steps = black_box(template.clone());
            graph_receipt_with_signature_policy(
                SCOPE,
                &steps,
                CREATED_AT,
                signature.signature_policy(),
            )
            .map(|receipt| receipt.digest)
        })
    });

    register_receipt_store_append(c, "receipt_store_append", 12);
    register_receipt_store_append(
        c,
        "receipt_store_append_scale_small",
        RECEIPT_STORE_SCALE_SMALL,
    );
    register_receipt_store_append(
        c,
        "receipt_store_append_scale_large",
        RECEIPT_STORE_SCALE_LARGE,
    );
    register_receipt_store_index(c, "receipt_store_index", 12);
    register_receipt_store_index(
        c,
        "receipt_store_index_scale_small",
        RECEIPT_STORE_SCALE_SMALL,
    );
    register_receipt_store_index(
        c,
        "receipt_store_index_scale_large",
        RECEIPT_STORE_SCALE_LARGE,
    );
}

fn session_metric(stats: runx_runtime::adapters::javascript::JavaScriptSessionStats) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        (
            "spawn_count".to_owned(),
            JsonValue::Number(JsonNumber::U64(stats.spawned_process_count)),
        ),
        (
            "peak_in_flight".to_owned(),
            JsonValue::Number(JsonNumber::U64(
                u64::try_from(stats.peak_in_flight).unwrap_or(u64::MAX),
            )),
        ),
    ]))
}

fn record_resource_metrics(metrics: &JsonObject) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::var_os("RUNX_PERF_RESOURCE_METRICS_PATH") else {
        return Ok(());
    };
    fs::write(path, serde_json::to_vec(metrics)?)?;
    Ok(())
}

fn record_resource_metric(name: &str, metric: JsonValue) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::var_os("RUNX_PERF_RESOURCE_METRICS_PATH") else {
        return Ok(());
    };
    let mut metrics = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<JsonObject>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => JsonObject::new(),
        Err(error) => return Err(error.into()),
    };
    metrics.insert(name.to_owned(), metric);
    record_resource_metrics(&metrics)
}

fn register_receipt_store_append(c: &mut Criterion, name: &str, count: usize) {
    c.bench_function(name, |b| {
        let signature = RuntimeReceiptSignatureConfig::local_development();
        let mut receipts = synthetic_receipts(&signature, count.saturating_add(1));
        let pending = receipts.pop();
        b.iter_batched(
            || prepare_receipt_append(&receipts),
            |prepared| match (prepared, pending.as_ref()) {
                (Ok(fixture), Some(receipt)) => fixture
                    .store
                    .write_receipt(black_box(receipt))
                    .map(|()| black_box(fixture.directory.path().to_path_buf()))
                    .map_err(|error| error.to_string()),
                (Err(message), _) => Err(message),
                (_, None) => Err("receipt append benchmark has no pending receipt".to_owned()),
            },
            BatchSize::PerIteration,
        )
    });
}

struct ReceiptAppendFixture {
    directory: TempDir,
    store: runx_runtime::LocalReceiptStore,
}

fn prepare_receipt_append(
    history: &[runx_contracts::Receipt],
) -> Result<ReceiptAppendFixture, String> {
    let directory = TempDir::new().map_err(|source| source.to_string())?;
    let store = runx_runtime::LocalReceiptStore::new(directory.path().join("receipts"));
    store
        .write_receipts(history)
        .map_err(|error| error.to_string())?;
    Ok(ReceiptAppendFixture { directory, store })
}

fn register_receipt_store_index(c: &mut Criterion, name: &str, count: usize) {
    c.bench_function(name, |b| {
        let signature = RuntimeReceiptSignatureConfig::local_development();
        let receipts = synthetic_receipts(&signature, count);
        let temp_dir = TempDir::new().map_err(|source| source.to_string());
        let temp_dir = match temp_dir {
            Ok(temp_dir) => temp_dir,
            Err(message) => return b.iter(|| Err::<usize, String>(message.clone())),
        };
        let store = runx_runtime::LocalReceiptStore::new(temp_dir.path().join("receipts"));
        if let Err(error) = store.write_receipts(&receipts) {
            return b.iter(|| Err::<usize, String>(error.to_string()));
        }
        b.iter(|| {
            store
                .rebuild_index()
                .map(|index| black_box(index.entries.len()))
                .map_err(|error| error.to_string())
        })
    });
}

fn sequential_steps(count: usize) -> Vec<SequentialGraphStepDefinition> {
    (0..count)
        .map(|index| SequentialGraphStepDefinition {
            id: format!("step_{index}"),
            context_from: (index > 0).then(|| vec![format!("step_{}", index - 1)]),
        })
        .collect()
}

fn drive_state_machine(
    steps: &[SequentialGraphStepDefinition],
    step_index: &SequentialGraphStepIndex,
) -> usize {
    let mut state = create_sequential_graph_state("throughput_graph", steps);
    let mut completed = 0usize;
    let mut planning_cursor = 0usize;
    loop {
        while state.steps.get(planning_cursor).is_some_and(|step| {
            matches!(
                step.status,
                runx_core::state_machine::GraphStepStatus::Succeeded
                    | runx_core::state_machine::GraphStepStatus::Skipped
            )
        }) {
            planning_cursor += 1;
        }
        let plan = plan_sequential_graph_transition_indexed_from(
            &state,
            steps,
            step_index,
            planning_cursor,
        );
        match plan {
            SequentialGraphPlan::RunStep {
                step_id, attempt, ..
            } => {
                state = start_step(state, &step_id, step_index);
                state = succeed_step(state, &step_id, attempt, step_index);
                completed += 1;
            }
            SequentialGraphPlan::Complete => {
                apply_sequential_graph_event_owned_indexed(
                    &mut state,
                    SequentialGraphEvent::Complete,
                    step_index,
                );
                return completed + usize::from(state.status == GraphStatus::Succeeded);
            }
            SequentialGraphPlan::Blocked { .. } | SequentialGraphPlan::Failed { .. } => {
                return completed;
            }
        }
    }
}

fn start_step(
    mut state: runx_core::state_machine::SequentialGraphState,
    step_id: &str,
    step_index: &SequentialGraphStepIndex,
) -> runx_core::state_machine::SequentialGraphState {
    start_sequential_graph_step_indexed(&mut state, step_id, CREATED_AT.to_owned(), step_index);
    state
}

fn succeed_step(
    mut state: runx_core::state_machine::SequentialGraphState,
    step_id: &str,
    attempt: u32,
    step_index: &SequentialGraphStepIndex,
) -> runx_core::state_machine::SequentialGraphState {
    let receipt_id = format!("sha256:{step_id}_{attempt}");
    succeed_sequential_graph_step_indexed(
        &mut state,
        CREATED_AT.to_owned(),
        StepAdmissionWitness::local_runtime(step_id, receipt_id),
        Some(object([(
            "value",
            JsonValue::String(format!("{step_id}:{attempt}")),
        )])),
        step_index,
    );
    state
}

fn synthetic_step_runs(signature: &RuntimeReceiptSignatureConfig, count: usize) -> Vec<StepRun> {
    (0..count)
        .map(|index| {
            let step_id = format!("step_{index}");
            let contract = object([(
                "nested",
                JsonValue::Object(object([("value", JsonValue::String(index.to_string()))])),
            )]);
            let output = skill_output(&format!(
                r#"{{"nested":{{"value":{index}}},"status":"ok"}}"#
            ));
            let receipt = match step_receipt_with_signature_policy(
                SCOPE,
                &step_id,
                1,
                &output,
                &contract,
                CREATED_AT,
                signature.signature_policy(),
            ) {
                Ok(receipt) => receipt,
                Err(_error) => std::process::exit(2),
            };
            StepRun {
                step_id: step_id.clone(),
                attempt: 1,
                skill: step_id.clone(),
                runner: None,
                contract,
                artifact_outputs: Default::default(),
                ephemeral_contract: EphemeralValue::default(),
                outcome: output.into(),
                admission_witness: StepAdmissionWitness::local_runtime(
                    &step_id,
                    receipt.id.as_str(),
                ),
                receipt,
                nested_receipts: Vec::new(),
            }
        })
        .collect()
}

fn synthetic_receipts(
    signature: &RuntimeReceiptSignatureConfig,
    count: usize,
) -> Vec<runx_contracts::Receipt> {
    synthetic_step_runs(signature, count)
        .into_iter()
        .map(|run| run.receipt)
        .collect()
}

fn skill_output(value: &str) -> InvocationOutput {
    let value = serde_json::from_str(value).unwrap_or_else(|_| JsonValue::String(value.to_owned()));
    InvocationOutput::runtime_success(value, 0, JsonObject::new())
}

fn object(entries: impl IntoIterator<Item = (&'static str, JsonValue)>) -> JsonObject {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

criterion_group!(benches, bench_graph_throughput);
criterion_main!(benches);
