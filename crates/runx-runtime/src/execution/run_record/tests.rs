#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use runx_contracts::{ApprovalGate, JsonObject, JsonValue, ResolutionRequest};
use runx_core::state_machine::StepAdmissionWitness;

use super::*;
use crate::adapter::InvocationOutput;
use crate::execution::runner::StepRun;
use crate::receipts::{RunScope, graph_receipt, step_receipt};

fn store() -> (tempfile::TempDir, Arc<KernelStore>) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = KernelStore::open(root.path()).expect("open");
    (root, store)
}

fn start(log: &RunLog, inputs: &JsonObject) {
    log.begin(StartRequest {
        skill: "triage",
        skill_ref: "skills/triage",
        runner: "default",
        package_digest: "sha256:package",
        execution_closure_digest: "sha256:closure",
        inputs,
        context: &RunContext::default(),
        run_authority: None,
        created_at: "2026-10-09T00:00:00.000Z",
    })
    .expect("begin");
}

fn step(run_id: &str, step_id: &str) -> StepRun {
    let output = InvocationOutput::runtime_success(
        JsonValue::Object(JsonObject::from([(
            "verdict".to_owned(),
            JsonValue::String("ok".to_owned()),
        )])),
        0,
        JsonObject::new(),
    );
    let claim = output.value.as_object().cloned().expect("object");
    let receipt = step_receipt(
        RunScope {
            run_id,
            graph_name: "triage",
        },
        step_id,
        1,
        &output,
        &claim,
        "2026-10-09T00:00:00.000Z",
    )
    .expect("receipt");
    StepRun {
        step_id: step_id.to_owned(),
        attempt: 1,
        skill: step_id.to_owned(),
        runner: None,
        contract: claim,
        ephemeral_contract: Default::default(),
        outcome: output.into(),
        admission_witness: StepAdmissionWitness::local_runtime(step_id, receipt.id.as_str()),
        receipt,
        nested_receipts: Vec::new(),
    }
}

/// The act an agent step asks for: what opens its turn.
fn agent_act(id: &str) -> ResolutionRequest {
    let envelope = runx_contracts::AgentContextEnvelope {
        run_id: "run_turn_1".into(),
        step_id: Some("answer".into()),
        skill: "turn-demo".into(),
        instructions_sha256: "sha256:instructions".into(),
        instructions: "Answer.".into(),
        inputs: JsonObject::new(),
        requirements: runx_contracts::AgentExecutionRequirements {
            declaration: runx_contracts::ExecutionRequirements::default(),
            environment: Vec::new(),
            execution_boundary: runx_contracts::ExecutionBoundaryObservation {
                kind: runx_contracts::ExecutionBoundaryKind::RemoteProvider,
            },
        },
        current_context: Vec::new(),
        historical_context: Vec::new(),
        provenance: Vec::new(),
        context: None,
        voice_profile: None,
        execution_location: None,
        output: None,
        output_schema: None,
        trust_boundary: "runtime-governed".into(),
    };
    ResolutionRequest::AgentAct {
        id: id.into(),
        invocation: Box::new(runx_contracts::AgentActInvocation {
            id: id.into(),
            role: "agent".into(),
            task: "answer".into(),
            envelope,
            allowance: Default::default(),
        }),
    }
}

fn approval(id: &str) -> ResolutionRequest {
    ResolutionRequest::Approval {
        id: id.into(),
        gate: ApprovalGate {
            id: id.into(),
            reason: "approve the send".into(),
            gate_type: None,
            summary: None,
        },
    }
}

#[test]
fn a_run_replays_its_steps_suspension_and_answers() {
    let (_root, store) = store();
    let log = RunLog::open(Arc::clone(&store), "run_triage_1").unwrap();
    let inputs = JsonObject::from([("issue".to_owned(), JsonValue::String("42".to_owned()))]);
    start(&log, &inputs);
    log.record_step(&step("run_triage_1", "read"), &[]).unwrap();
    log.record_skipped(vec!["notify".to_owned()]).unwrap();
    log.record_request("send", 1, &approval("send.approval"))
        .unwrap();

    let record = log.load().unwrap().unwrap();
    assert_eq!(record.inputs, inputs);
    assert_eq!(record.facts.len(), 2);
    let Some(Suspension { step_id, hold, .. }) = &record.suspension else {
        panic!("the run is suspended");
    };
    assert_eq!(step_id, "send");
    let Hold::Request(request) = hold else {
        panic!("the step waits on its own request");
    };
    assert_eq!(request.id, "send.approval");
    assert_eq!(
        request.digest,
        request_digest(&approval("send.approval")).unwrap()
    );
    assert_eq!(
        pending_request(&store, "run_triage_1")
            .unwrap()
            .map(|request| request.id),
        Some("send.approval".to_owned())
    );

    // The step that consumes the answer records it; the run then no longer
    // waits on the host.
    log.record_step(
        &step("run_triage_1", "send"),
        &[Answer {
            request_id: "send.approval".to_owned(),
            request_digest: request.digest.clone(),
            value: JsonValue::Bool(true),
            human: true,
        }],
    )
    .unwrap();
    let record = log.load().unwrap().unwrap();
    assert!(record.suspension.is_none());
    assert_eq!(record.answers.len(), 1);
    assert!(record.answers[0].human);
    assert_eq!(record.answers[0].request_digest, request.digest);
    assert_eq!(pending_request(&store, "run_triage_1").unwrap(), None);
}

#[test]
fn a_second_driver_of_the_same_run_is_refused() {
    let (_root, store) = store();
    let first = RunLog::open(Arc::clone(&store), "run_triage_2").unwrap();
    start(&first, &JsonObject::new());
    let second = RunLog::open(Arc::clone(&store), "run_triage_2").unwrap();
    first
        .record_step(&step("run_triage_2", "read"), &[])
        .unwrap();
    assert!(matches!(
        second.record_step(&step("run_triage_2", "read"), &[]),
        Err(RunRecordError::Contended { .. })
    ));
    assert!(matches!(
        RunLog::open(Arc::clone(&store), "run_triage_2")
            .unwrap()
            .begin(StartRequest {
                skill: "triage",
                skill_ref: "skills/triage",
                runner: "default",
                package_digest: "sha256:package",
                execution_closure_digest: "sha256:closure",
                inputs: &JsonObject::new(),
                context: &RunContext::default(),
                run_authority: None,
                created_at: "2026-10-09T00:00:00.000Z",
            }),
        Err(RunRecordError::Contended { .. })
    ));
}

#[test]
fn a_sealed_run_holds_its_receipt_and_waits_on_nothing() {
    let (_root, store) = store();
    let log = RunLog::open(Arc::clone(&store), "run_triage_3").unwrap();
    start(&log, &JsonObject::new());
    let read = step("run_triage_3", "read");
    log.record_step(&read, &[]).unwrap();
    let receipt = graph_receipt(
        RunScope {
            run_id: "run_triage_3",
            graph_name: "triage",
        },
        &[read],
        "2026-10-09T00:00:00.000Z",
    )
    .unwrap();
    log.record_sealed(&receipt, None, None).unwrap();

    let record = log.load().unwrap().unwrap();
    let sealed = record.sealed.expect("sealed");
    assert_eq!(sealed.receipt, receipt);
    assert_eq!(
        sealed.receipt.subject.reference.uri.as_str(),
        "runx:run:run_triage_3"
    );
    assert_eq!(pending_request(&store, "run_triage_3").unwrap(), None);
}

#[test]
fn a_child_run_holds_the_request_its_parent_step_waits_on() {
    let (_root, store) = store();
    let parent = RunLog::open(Arc::clone(&store), "run_triage_4").unwrap();
    start(&parent, &JsonObject::new());
    let child = parent.child("review").unwrap();
    assert_eq!(child.run_id(), "run_triage_4/review");
    assert!(
        child
            .start_child(
                "run_triage_4",
                "review",
                "review",
                &JsonObject::new(),
                "2026-10-09T00:00:00.000Z"
            )
            .unwrap()
            .is_none()
    );
    child
        .record_request("approve", 1, &approval("review.approval"))
        .unwrap();
    parent
        .record_child_hold("review", 1, child.run_id())
        .unwrap();

    assert_eq!(
        pending_request(&store, "run_triage_4")
            .unwrap()
            .map(|request| request.id),
        Some("review.approval".to_owned())
    );
    // A child continues only under the parent step that started it.
    assert!(matches!(
        child.start_child(
            "run_triage_4",
            "other",
            "review",
            &JsonObject::new(),
            "2026-10-09T00:00:00.000Z"
        ),
        Err(RunRecordError::IdentityMismatch { .. })
    ));
}

#[test]
fn run_ids_are_minted_unique_and_validated() {
    let first = mint_run_id("Triage Issues").unwrap();
    let second = mint_run_id("Triage Issues").unwrap();
    assert_ne!(first, second);
    assert!(first.starts_with("run_triage-issues_"));
    assert!(validate_run_id(&first).is_ok());
    assert!(validate_run_id("hosted:job.42-a").is_ok());
    for invalid in ["", "run/child", "run id", "run\nid"] {
        assert!(
            validate_run_id(invalid).is_err(),
            "{invalid:?} must be refused"
        );
    }
}

#[test]
fn the_run_listing_shows_top_level_runs_newest_first() {
    let workspace = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([(
        crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
        workspace.path().join(".runx").display().to_string(),
    )]);
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, workspace.path()))
        .unwrap();
    for run_id in ["run_triage_a", "run_triage_b"] {
        let log = RunLog::open(Arc::clone(&store), run_id).unwrap();
        start(&log, &JsonObject::new());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let paused = RunLog::open(Arc::clone(&store), "run_triage_a").unwrap();
    paused.load().unwrap();
    paused
        .record_request("send", 1, &approval("send.approval"))
        .unwrap();
    let child = paused.child("review").unwrap();
    child
        .start_child(
            "run_triage_a",
            "review",
            "review",
            &JsonObject::new(),
            "2026-10-09T00:00:00.000Z",
        )
        .unwrap();

    let listing = list_runs(&env, workspace.path(), &RunFilter::default()).unwrap();
    let ids = listing
        .runs
        .iter()
        .map(|run| run.run_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["run_triage_a", "run_triage_b"]);
    assert_eq!(listing.runs[0].state, RunState::Paused);
    assert_eq!(
        listing.runs[0].pending_request_id.as_deref(),
        Some("send.approval")
    );
    assert_eq!(listing.runs[1].state, RunState::Running);

    let paused_only = list_runs(
        &env,
        workspace.path(),
        &RunFilter {
            state: Some("paused".to_owned()),
            ..RunFilter::default()
        },
    )
    .unwrap();
    assert_eq!(paused_only.runs.len(), 1);
    assert!(matches!(
        list_runs(
            &env,
            workspace.path(),
            &RunFilter {
                since: Some("yesterday".to_owned()),
                ..RunFilter::default()
            },
        ),
        Err(RunRecordError::InvalidFilter { .. })
    ));
}

#[test]
fn a_call_leaves_the_run_paused_at_its_turn() {
    let workspace = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([(
        crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
        workspace.path().join(".runx").display().to_string(),
    )]);
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, workspace.path()))
        .unwrap();
    let log = RunLog::open(Arc::clone(&store), "run_turn_1").unwrap();
    start(&log, &JsonObject::new());
    let turn_request = agent_act("agent.answer.output");
    log.record_request("answer", 1, &turn_request).unwrap();
    let turn = pending_request(&store, "run_turn_1").unwrap().unwrap();
    let target = TurnTarget::Tool {
        tool: "data.digest".into(),
    };
    let input = JsonObject::from([("value".to_owned(), JsonValue::String("a".to_owned()))]);
    let completed = CallOutcome::Completed(Box::new(step("run_turn_1", "answer/0")));
    let pending = CallOutcome::Pending(PendingRequest {
        id: "answer/1".to_owned(),
        value: serde_json::from_value(serde_json::to_value(approval("answer/1")).unwrap()).unwrap(),
        digest: "sha256:ask".to_owned(),
    });
    for (index, outcome) in [(0, &completed), (1, &pending)] {
        log.record_call(&CallRecord {
            step_id: "answer",
            attempt: 1,
            turn: &turn,
            index,
            target: &target,
            input: &input,
            outcome,
            answers: &[],
        })
        .unwrap();
    }

    assert_eq!(
        pending_request(&store, "run_turn_1")
            .unwrap()
            .map(|request| request.id),
        Some("agent.answer.output".to_owned())
    );
    let listing = list_runs(&env, workspace.path(), &RunFilter::default()).unwrap();
    assert_eq!(listing.runs[0].state, RunState::Paused);
    assert_eq!(
        listing.runs[0].pending_request_id.as_deref(),
        Some("agent.answer.output")
    );
    let record = log.load().unwrap().unwrap();
    assert_eq!(
        record.suspension.map(|held| held.step_id),
        Some("answer".to_owned())
    );
    assert_eq!(record.calls.len(), 2);
    assert_eq!(log.calls("answer", 1).unwrap().len(), 2);
    assert_eq!(
        waiting_requests(&store, "run_turn_1")
            .unwrap()
            .into_iter()
            .map(|request| request.id)
            .collect::<Vec<_>>(),
        ["agent.answer.output", "answer/1"],
        "a paused turn waits on its act and on each pending call"
    );

    // A later event for the same index settles the pending call.
    log.record_call(&CallRecord {
        step_id: "answer",
        attempt: 1,
        turn: &turn,
        index: 1,
        target: &target,
        input: &input,
        outcome: &CallOutcome::Completed(Box::new(step("run_turn_1", "answer/1"))),
        answers: &[],
    })
    .unwrap();
    let calls = log.calls("answer", 1).unwrap();
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|call| matches!(call.outcome, CallOutcome::Completed(_)))
    );
    assert_eq!(
        waiting_requests(&store, "run_turn_1")
            .unwrap()
            .into_iter()
            .map(|request| request.id)
            .collect::<Vec<_>>(),
        ["agent.answer.output"]
    );

    // A reader that has not seen the run's latest event is contended.
    let stale = RunLog::open(Arc::clone(&store), "run_turn_1").unwrap();
    log.record_call(&CallRecord {
        step_id: "answer",
        attempt: 1,
        turn: &turn,
        index: 2,
        target: &target,
        input: &input,
        outcome: &CallOutcome::Refused("not in the turn's allowance".to_owned()),
        answers: &[],
    })
    .unwrap();
    assert!(matches!(
        stale.calls("answer", 1),
        Err(RunRecordError::Contended { .. })
    ));
}
