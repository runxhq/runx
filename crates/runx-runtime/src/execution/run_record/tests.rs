#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use runx_contracts::{ApprovalGate, JsonObject, JsonValue, ResolutionRequest};
use runx_core::state_machine::StepAdmissionWitness;

use super::*;
use crate::adapter::InvocationOutput;
use crate::execution::runner::StepRun;
use crate::receipts::{RunScope, RuntimeReceiptSignatureConfig, graph_receipt, step_receipt};

fn signature() -> RuntimeReceiptSignatureConfig {
    RuntimeReceiptSignatureConfig::fixture().expect("fixture signer")
}

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
        act: None,
        driver: RunDriver::Attached,
        prepared: None,
        created_at: "2026-10-09T00:00:00.000Z",
    })
    .expect("begin");
}

fn waiting_ids(store: &KernelStore, run_id: &str) -> Vec<String> {
    waiting_requests(store, run_id)
        .unwrap()
        .into_iter()
        .map(|request| request.id)
        .collect()
}

fn step(run_id: &str, step_id: &str) -> StepRun {
    let output = InvocationOutput::runtime_success(
        JsonValue::Object(JsonObject::from([(
            "verdict".to_owned(),
            JsonValue::String("ok".to_owned()),
        )])),
        0,
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
        signature().signature_policy(),
    )
    .expect("receipt");
    StepRun {
        step_id: step_id.to_owned(),
        attempt: 1,
        skill: step_id.to_owned(),
        runner: None,
        contract: claim,
        artifact_outputs: Default::default(),
        ephemeral_contract: Default::default(),
        outcome: output.into(),
        admission_witness: StepAdmissionWitness::local_runtime(step_id, receipt.id.as_str()),
        receipt,
        nested_receipts: Vec::new(),
        settlement: None,
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
    assert_eq!(waiting_ids(&store, "run_triage_1"), ["send.approval"]);

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
    assert!(waiting_ids(&store, "run_triage_1").is_empty());
}

#[test]
fn a_replayed_step_knows_which_of_its_outputs_are_artifacts() {
    let (_root, store) = store();
    let log = RunLog::open(Arc::clone(&store), "run_triage_1").unwrap();
    start(&log, &JsonObject::new());
    let mut read = step("run_triage_1", "read");
    read.artifact_outputs = BTreeSet::from(["verdict".to_owned()]);
    log.record_step(&read, &[]).unwrap();

    let record = log.load().unwrap().unwrap();
    let Some(Replayed::Step(replayed)) = record.facts.first() else {
        panic!("the step replays");
    };
    assert_eq!(replayed.artifact_outputs, read.artifact_outputs);
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
                act: None,
                driver: RunDriver::Attached,
                prepared: None,
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
        signature().signature_policy(),
    )
    .unwrap();
    log.record_sealed(&receipt, None).unwrap();

    let record = log.load().unwrap().unwrap();
    let sealed = record.sealed.expect("sealed");
    assert_eq!(sealed.receipt, receipt);
    assert_eq!(
        sealed.receipt.subject.reference.uri.as_str(),
        "runx:run:run_triage_3"
    );
    assert!(waiting_ids(&store, "run_triage_3").is_empty());
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
                None,
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

    assert_eq!(waiting_ids(&store, "run_triage_4"), ["review.approval"]);
    // A child continues only under the parent step that started it.
    assert!(matches!(
        child.start_child(
            "run_triage_4",
            "other",
            "review",
            &JsonObject::new(),
            None,
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
    for invalid in ["", "run/child", "run id", "run\nid", "sha256:abc"] {
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
            None,
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
    let turn_request = agent_act("answer");
    log.record_request("answer", 1, &turn_request).unwrap();
    let turn = waiting_requests(&store, "run_turn_1").unwrap().remove(0);
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
            call_id: None,
            target: &target,
            input: &input,
            outcome,
            outputs: &[],
            answers: &[],
        })
        .unwrap();
    }

    let listing = list_runs(&env, workspace.path(), &RunFilter::default()).unwrap();
    assert_eq!(listing.runs[0].state, RunState::Paused);
    assert_eq!(
        listing.runs[0].pending_request_id.as_deref(),
        Some("answer")
    );
    let record = log.load().unwrap().unwrap();
    assert_eq!(
        record.suspension.map(|held| held.step_id),
        Some("answer".to_owned())
    );
    assert_eq!(record.calls.len(), 2);
    assert_eq!(log.calls("answer", 1).unwrap().len(), 2);
    assert_eq!(
        waiting_ids(&store, "run_turn_1"),
        ["answer", "answer/1"],
        "a paused turn waits on its act and on each pending call"
    );

    // A later event for the same index settles the pending call.
    log.record_call(&CallRecord {
        step_id: "answer",
        attempt: 1,
        turn: &turn,
        index: 1,
        call_id: None,
        target: &target,
        input: &input,
        outcome: &CallOutcome::Completed(Box::new(step("run_turn_1", "answer/1"))),
        outputs: &[],
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
    assert_eq!(waiting_ids(&store, "run_turn_1"), ["answer"]);

    // A reader that has not seen the run's latest event is contended.
    let stale = RunLog::open(Arc::clone(&store), "run_turn_1").unwrap();
    log.record_call(&CallRecord {
        step_id: "answer",
        attempt: 1,
        turn: &turn,
        index: 2,
        call_id: None,
        target: &target,
        input: &input,
        outcome: &CallOutcome::Refused("not in the turn's allowance".to_owned()),
        outputs: &[],
        answers: &[],
    })
    .unwrap();
    assert!(matches!(
        stale.calls("answer", 1),
        Err(RunRecordError::Contended { .. })
    ));
}

#[test]
fn a_paused_run_waits_on_every_open_request_in_its_tree() {
    let (_root, store) = store();
    let log = RunLog::open(Arc::clone(&store), "run_tree_1").unwrap();
    start(&log, &JsonObject::new());
    log.record_request("answer", 1, &agent_act("answer"))
        .unwrap();
    let turn = waiting_requests(&store, "run_tree_1").unwrap().remove(0);

    // Call 0 runs a child skill: its own turn is open, and its ask waits.
    let child = log.child("answer/0").unwrap();
    assert_eq!(child.address("report"), "answer/0#report");
    assert_eq!(local_request_id("answer/0#report/0"), "report/0");
    assert_eq!(local_request_id("answer"), "answer");
    child
        .start_child(
            "run_tree_1",
            "answer/0",
            "report",
            &JsonObject::new(),
            None,
            "2026-10-09T00:00:00.000Z",
        )
        .unwrap();
    child
        .record_request("report", 1, &agent_act(&child.address("report")))
        .unwrap();
    let child_turn = waiting_requests(&store, child.run_id()).unwrap().remove(0);
    let ask = approval(&child.address("report/0"));
    child
        .record_call(&CallRecord {
            step_id: "report",
            attempt: 1,
            turn: &child_turn,
            index: 0,
            call_id: None,
            target: &TurnTarget::Ask,
            input: &JsonObject::new(),
            outcome: &CallOutcome::Pending(PendingRequest {
                id: child.address("report/0"),
                value: serde_json::from_value(serde_json::to_value(&ask).unwrap()).unwrap(),
                digest: request_digest(&ask).unwrap(),
            }),
            outputs: &[],
            answers: &[],
        })
        .unwrap();
    log.record_call(&CallRecord {
        step_id: "answer",
        attempt: 1,
        turn: &turn,
        index: 0,
        call_id: None,
        target: &TurnTarget::Skill {
            skill: "./report".into(),
        },
        input: &JsonObject::new(),
        outcome: &CallOutcome::Child(child.run_id().to_owned()),
        outputs: &[],
        answers: &[],
    })
    .unwrap();

    assert_eq!(
        waiting_ids(&store, "run_tree_1"),
        ["answer", "answer/0#report", "answer/0#report/0"],
        "the run waits on its turn, its child's turn, and the child's ask"
    );
}

#[test]
fn a_run_whose_child_sealed_before_its_step_was_recorded_stays_paused() {
    let workspace = tempfile::tempdir().unwrap();
    let env = BTreeMap::from([(
        crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
        workspace.path().join(".runx").display().to_string(),
    )]);
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, workspace.path()))
        .unwrap();
    let parent = RunLog::open(Arc::clone(&store), "run_held_1").unwrap();
    start(&parent, &JsonObject::new());
    let child = parent.child("review").unwrap();
    child
        .start_child(
            "run_held_1",
            "review",
            "review",
            &JsonObject::new(),
            None,
            "2026-10-09T00:00:00.000Z",
        )
        .unwrap();
    child
        .record_request("approve", 1, &approval(&child.address("review.approval")))
        .unwrap();
    parent
        .record_child_hold("review", 1, child.run_id())
        .unwrap();
    assert_eq!(
        waiting_ids(&store, "run_held_1"),
        ["review#review.approval"]
    );

    // The child seals; the process stops before the parent records its step.
    let read = step(child.run_id(), "approve");
    child.record_step(&read, &[]).unwrap();
    let receipt = graph_receipt(
        RunScope {
            run_id: child.run_id(),
            graph_name: "review",
        },
        &[read],
        "2026-10-09T00:00:00.000Z",
        signature().signature_policy(),
    )
    .unwrap();
    child.record_sealed(&receipt, None).unwrap();

    assert!(waiting_ids(&store, "run_held_1").is_empty());
    let paused = paused_run(&env, workspace.path(), "run_held_1")
        .unwrap()
        .expect("the parent is still paused, so a continuation can take it further");
    assert!(paused.waiting.is_empty());
}

/// A step and the effect it settles commit together: a step record that is
/// refused leaves the act unsettled, so a replay recovers its attempt rather
/// than sending it again, and the record that lands settles it.
#[test]
fn a_step_settles_its_effect_in_the_same_transaction() {
    use crate::kernel::custody::{Attempt, EffectCustody};

    let (_root, store) = store();
    let digest = |text: &str| runx_contracts::connector::Digest::of(text.as_bytes());
    let intent = runx_contracts::OperationIntent {
        connector: runx_contracts::connector::ConnectorRef::new("runx/slack").unwrap(),
        manifest_digest: digest("manifest"),
        connector_digest: digest("connector"),
        operation: runx_contracts::connector::OperationName::new("channel.post").unwrap(),
        operation_digest: digest("operation"),
        instance_id: runx_contracts::connector::InstanceId::new("slack-acme").unwrap(),
        target: "slack://T123/C456".into(),
        payload_digest: digest("payload"),
        request_key_digest: Some(digest("deliver-1")),
        amount: None,
        approval_digest: None,
        act_digest: None,
    };
    let custody = EffectCustody::for_act(Arc::clone(&store), &intent, &digest("intent"), "run-1");
    custody
        .attempt(
            &Attempt {
                intent_digest: "sha256:intent",
                authority_term_id: "term-1",
                mandate_ref: None,
                idempotency_key: "runx:sha256:intent",
                instance_binding: "sha256:binding",
                attempt: 1,
                approval: None,
                implementation_digest: None,
                admitted_at: 0,
            },
            &[],
        )
        .unwrap();
    let first = RunLog::open(Arc::clone(&store), "run_triage_9").unwrap();
    start(&first, &JsonObject::new());
    let stale = RunLog::open(Arc::clone(&store), "run_triage_9").unwrap();
    first.record_skipped(vec!["other".to_owned()]).unwrap();

    let mut run = step("run_triage_9", "deliver");
    run.settlement = Some(custody.settlement(run.receipt.id.to_string(), &[]));
    assert!(matches!(
        stale.record_step(&run, &[]),
        Err(RunRecordError::Contended { .. })
    ));
    assert!(
        custody.pending().unwrap().is_some(),
        "a refused step settles nothing"
    );

    first.record_step(&run, &[]).unwrap();
    assert!(
        custody.pending().unwrap().is_none(),
        "the recorded step settled its act"
    );
}

#[test]
fn a_claimed_step_no_step_event_settled_waits_on_its_retry() {
    let (_root, store) = store();
    let log = RunLog::open(Arc::clone(&store), "run_triage_9").unwrap();
    start(&log, &JsonObject::new());
    let retry = crate::execution::runner::unsettled_retry(
        log.address("send/retry"),
        "step send",
        runx_contracts::RetryResend::Safe,
    );
    log.record_claim("send", 1, &retry, &[]).unwrap();

    // The driver stopped after the claim: the step may have run.
    let record = log.load().unwrap().unwrap();
    let Some(Suspension { step_id, hold, .. }) = &record.suspension else {
        panic!("an unsettled claim holds the run");
    };
    assert_eq!(step_id, "send");
    let Hold::Claimed(request) = hold else {
        panic!("it waits on its retry");
    };
    assert_eq!(request.id, log.address("send/retry"));
    assert_eq!(record.retries, 0, "a claim is not a readback retry");
    assert_eq!(
        waiting_ids(&store, "run_triage_9"),
        [log.address("send/retry")]
    );

    // A step that settles after its claim leaves nothing waiting.
    log.record_step(&step("run_triage_9", "send"), &[]).unwrap();
    assert!(log.load().unwrap().unwrap().suspension.is_none());
}

#[test]
fn an_admitted_answer_waits_in_the_record_until_a_step_uses_it() {
    let (_root, store) = store();
    let log = RunLog::open(Arc::clone(&store), "run_triage_10").unwrap();
    start(&log, &JsonObject::new());
    log.record_request("send", 1, &approval("send.approval"))
        .unwrap();
    let digest = request_digest(&approval("send.approval")).unwrap();
    let answer = Answer {
        request_id: "send.approval".to_owned(),
        request_digest: digest.clone(),
        value: JsonValue::Bool(true),
        human: true,
    };
    log.record_answer(&answer).unwrap();

    // The answer leaves the run where it waits, and any later driver finds
    // it in the record.
    assert_eq!(waiting_ids(&store, "run_triage_10"), ["send.approval"]);
    assert_eq!(log.admitted().unwrap(), [answer.clone()]);
    let record = log.load().unwrap().unwrap();
    assert!(matches!(
        record.suspension,
        Some(Suspension {
            hold: Hold::Request(_),
            ..
        })
    ));

    // The step that uses it consumes it.
    log.record_step(&step("run_triage_10", "send"), &[answer])
        .unwrap();
    assert!(log.admitted().unwrap().is_empty());
}

#[test]
fn a_bounded_retry_records_when_it_is_due() {
    let (_root, store) = store();
    let log = RunLog::open(Arc::clone(&store), "run_triage_11").unwrap();
    start(&log, &JsonObject::new());
    let retry = ResolutionRequest::Retry {
        id: "provider-readback-send-1".into(),
        reason: "Independent provider readback is pending".into(),
        retry_after_seconds: 300,
        resend: runx_contracts::RetryResend::Safe,
    };
    let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_760_000_000);
    log.record_retry("send", &retry, now).unwrap();
    let record = log.load().unwrap().unwrap();
    let Some(Suspension {
        hold: Hold::Retry { due_at, .. },
        ..
    }) = &record.suspension
    else {
        panic!("the run waits on its bounded retry");
    };
    assert_eq!(
        due_at.as_deref(),
        Some(crate::time::iso8601_from_unix_seconds(1_760_000_300).as_str())
    );
    assert_eq!(record.retries, 1);
}

fn project_env(workspace: &tempfile::TempDir) -> (BTreeMap<String, String>, Arc<KernelStore>) {
    let env = BTreeMap::from([(
        crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
        workspace.path().join(".runx").display().to_string(),
    )]);
    let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(&env, workspace.path()))
        .unwrap();
    (env, store)
}

fn kinds(read: &runx_contracts::RunRead) -> Vec<(String, u64, String)> {
    read.events
        .iter()
        .map(|record| {
            let kind = serde_json::to_value(&record.event).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned();
            (record.stream.as_str().to_owned(), record.version, kind)
        })
        .collect()
}

#[test]
fn a_read_after_its_cursor_returns_only_new_events_in_every_stream() {
    let workspace = tempfile::tempdir().unwrap();
    let (env, store) = project_env(&workspace);
    let parent = RunLog::open(Arc::clone(&store), "run_read_1").unwrap();
    start(&parent, &JsonObject::new());
    parent
        .record_request("send", 1, &approval("send.approval"))
        .unwrap();

    let first = read_run(&env, workspace.path(), "run_read_1", None).unwrap();
    assert_eq!(
        kinds(&first),
        [
            ("run_read_1".to_owned(), 1, "started".to_owned()),
            ("run_read_1".to_owned(), 2, "suspended".to_owned()),
        ]
    );
    assert_eq!(first.run.state, RunState::Paused);
    assert!(!first.run.driven);
    assert!(!first.more);
    assert_eq!(first.waiting.len(), 1);
    assert_eq!(first.waiting[0].address, "send.approval");

    // A child that starts after the first read is read from its beginning.
    let child = parent.child("review").unwrap();
    child
        .start_child(
            "run_read_1",
            "review",
            "review",
            &JsonObject::new(),
            None,
            "2026-10-09T00:00:00.000Z",
        )
        .unwrap();
    parent
        .record_step(&step("run_read_1", "send"), &[])
        .unwrap();

    let second = read_run(
        &env,
        workspace.path(),
        "run_read_1",
        Some(first.cursor.as_str()),
    )
    .unwrap();
    assert_eq!(
        kinds(&second),
        [
            ("run_read_1".to_owned(), 3, "step".to_owned()),
            (
                "run_read_1/review".to_owned(),
                1,
                "child_started".to_owned()
            ),
        ]
    );
    let third = read_run(
        &env,
        workspace.path(),
        "run_read_1",
        Some(second.cursor.as_str()),
    )
    .unwrap();
    assert!(third.events.is_empty(), "nothing is read twice");
    assert_eq!(third.run.state, RunState::Running);
}

#[test]
fn a_read_refuses_another_runs_cursor_a_damaged_one_and_an_unknown_run() {
    let workspace = tempfile::tempdir().unwrap();
    let (env, store) = project_env(&workspace);
    for run_id in ["run_read_a", "run_read_b"] {
        start(
            &RunLog::open(Arc::clone(&store), run_id).unwrap(),
            &JsonObject::new(),
        );
    }
    let other = read_run(&env, workspace.path(), "run_read_b", None).unwrap();
    assert!(matches!(
        read_run(
            &env,
            workspace.path(),
            "run_read_a",
            Some(other.cursor.as_str())
        ),
        Err(RunRecordError::CursorRunMismatch { .. })
    ));
    assert!(matches!(
        read_run(&env, workspace.path(), "run_read_a", Some("not a cursor")),
        Err(RunRecordError::InvalidCursor)
    ));
    assert!(matches!(
        read_run(&env, workspace.path(), "run_missing", None),
        Err(RunRecordError::UnknownRun { .. })
    ));
}

#[test]
fn a_long_record_is_read_in_pages() {
    let workspace = tempfile::tempdir().unwrap();
    let (env, store) = project_env(&workspace);
    let log = RunLog::open(Arc::clone(&store), "run_read_long").unwrap();
    start(&log, &JsonObject::new());
    for index in 0..520 {
        log.record_skipped(vec![format!("step-{index}")]).unwrap();
    }
    let first = read_run(&env, workspace.path(), "run_read_long", None).unwrap();
    assert_eq!(first.events.len(), 500);
    assert!(first.more);
    let rest = read_run(
        &env,
        workspace.path(),
        "run_read_long",
        Some(first.cursor.as_str()),
    )
    .unwrap();
    assert_eq!(rest.events.len(), 21);
    assert!(!rest.more);
    assert_eq!(rest.events[0].version, 501);
}

#[test]
fn a_driving_process_reads_as_driven() {
    let workspace = tempfile::tempdir().unwrap();
    let (env, store) = project_env(&workspace);
    start(
        &RunLog::open(Arc::clone(&store), "run_read_driven").unwrap(),
        &JsonObject::new(),
    );
    let paths = crate::paths::RunxPaths::resolve(&env, workspace.path());
    let _driver = DriverLock::acquire(&paths, "run_read_driven").unwrap();
    assert!(
        read_run(&env, workspace.path(), "run_read_driven", None)
            .unwrap()
            .run
            .driven
    );
}
