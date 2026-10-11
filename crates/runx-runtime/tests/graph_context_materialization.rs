use std::path::Path;
use std::sync::{Arc, Mutex};

use runx_contracts::{JsonObject, JsonValue, ProvenanceEntry};
use runx_runtime::{
    InvocationOutput, Runtime, RuntimeError, RuntimeOptions, SkillAdapter, SkillInvocation,
};

#[derive(Clone, Default)]
struct RecordingAdapter {
    calls: Arc<Mutex<Vec<RecordedInvocation>>>,
}

struct RecordedInvocation {
    skill_name: String,
    step_id: Option<String>,
    inputs: JsonObject,
    provenance: Vec<ProvenanceEntry>,
}

impl SkillAdapter for RecordingAdapter {
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        self.calls
            .lock()
            .map_err(|_| RuntimeError::ReceiptInvalid {
                message: "context regression adapter lock poisoned".to_owned(),
            })?
            .push(RecordedInvocation {
                skill_name: request.skill_name.clone(),
                step_id: request.step_id.clone(),
                inputs: request.inputs.clone(),
                provenance: request.provenance.clone(),
            });
        Ok(InvocationOutput::runtime_success(
            JsonValue::Object(request.inputs),
            0,
        ))
    }
}

#[test]
fn graph_context_materialization_reaches_the_target_invocation()
-> Result<(), Box<dyn std::error::Error>> {
    let adapter = RecordingAdapter::default();
    let (_run, run) = crate::support::scratch_run("run_context_materialization")?;
    let runtime = Runtime::new(
        adapter.clone(),
        RuntimeOptions::local_development(std::env::vars().collect(), run),
    );
    let graph_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/graphs/sequential/graph.yaml");

    let run = runtime.run_graph_file(&graph_path)?;

    assert_eq!(run.steps.len(), 2);
    let calls = adapter
        .calls
        .lock()
        .map_err(|_| "context regression adapter lock poisoned")?;
    assert_eq!(calls.len(), 2, "producer and consumer must both run");
    let consumer = calls.last().ok_or("consumer skill was not invoked")?;
    assert_eq!(
        consumer.inputs.get("message"),
        Some(&JsonValue::String("hello from graph".to_owned()))
    );
    assert!(!consumer.skill_name.is_empty());
    assert_eq!(consumer.step_id.as_deref(), Some("default"));
    assert_eq!(consumer.provenance.len(), 1);
    let edge = &consumer.provenance[0];
    assert_eq!(edge.input.as_ref(), "message");
    assert_eq!(edge.output.as_ref(), "result.message");
    assert_eq!(edge.from_step.as_deref(), Some("first"));
    assert_eq!(
        edge.receipt_id.as_deref(),
        Some(run.steps[0].receipt.id.as_str())
    );
    Ok(())
}
