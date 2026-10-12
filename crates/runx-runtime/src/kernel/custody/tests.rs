#![allow(clippy::expect_used, clippy::unwrap_used)]

use runx_contracts::JsonValue;

use super::*;
use crate::kernel::store::KernelStore;

fn digest(text: &str) -> Digest {
    Digest::of(text.as_bytes())
}

/// The intent of a post with request key `key` on `target`.
fn intent(target: &str, key: &str) -> OperationIntent {
    OperationIntent {
        connector: runx_contracts::connector::ConnectorRef::new("runx/slack").unwrap(),
        manifest_digest: digest("manifest"),
        connector_digest: digest("connector"),
        operation: runx_contracts::connector::OperationName::new("channel.post").unwrap(),
        operation_digest: digest("operation"),
        instance_id: runx_contracts::connector::InstanceId::new("slack-acme").unwrap(),
        target: target.into(),
        payload_digest: digest("payload"),
        request_key_digest: Some(digest(key)),
        amount: None,
        approval_digest: None,
        act_digest: None,
    }
}

fn custody() -> (tempfile::TempDir, EffectCustody) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = KernelStore::open(root.path()).expect("store");
    let custody = EffectCustody::for_act(
        store,
        &intent("slack://T123/C456", "deliver-1"),
        &digest("intent"),
        "run-1",
    );
    (root, custody)
}

/// Whether any attempt of the act was recorded.
fn attempted(custody: &EffectCustody) -> bool {
    custody.store.version::<EffectEvent>(&custody.key).unwrap() > 0
}

/// Attempt `number` of the act, admitted against the stream as it stands.
fn attempt(custody: &EffectCustody, number: u32) -> Attempt<'static> {
    Attempt {
        intent_digest: "sha256:intent",
        authority_term_id: "term-1",
        mandate_ref: None,
        idempotency_key: "runx:sha256:intent",
        instance_binding: "sha256:binding",
        attempt: number,
        approval: None,
        implementation_digest: None,
        admitted_at: custody.custody().unwrap().head,
    }
}

/// Settle the act as the run record does when it records the step.
fn finalize(custody: &EffectCustody, receipt_id: &str) {
    let store = &custody.store;
    let appends = custody
        .settlement(receipt_id.to_owned(), &[])
        .appends(store)
        .unwrap();
    store.append(appends).unwrap();
}

fn readback() -> Readback {
    Readback {
        provider: "slack".to_owned(),
        operation: "channel.post".to_owned(),
        target: "slack://T123/C456".to_owned(),
        result: JsonValue::Null,
        readback_ref: "runx:readback:1".to_owned(),
        operation_id: Some("m-1".to_owned()),
        idempotency_key: Some("runx:sha256:intent".to_owned()),
    }
}

#[test]
fn an_attempt_stays_pending_until_it_settles() {
    let (_root, custody) = custody();
    assert!(custody.pending().unwrap().is_none());
    custody.attempt(&attempt(&custody, 1), &[]).unwrap();
    custody.unknown("timed out".to_owned()).unwrap();
    let pending = custody.pending().unwrap().expect("pending");
    assert!(pending.unknown);
    assert_eq!(pending.attempt, 1);

    custody.attempt(&attempt(&custody, 2), &[]).unwrap();
    custody.read_back(readback()).unwrap();
    let pending = custody.pending().unwrap().expect("pending");
    assert_eq!(pending.attempt, 2);
    assert_eq!(pending.readback, Some(readback()));
    assert!(!pending.unknown);

    finalize(&custody, "receipt-1");
    assert!(custody.pending().unwrap().is_none());
}

#[test]
fn a_sequence_runs_from_its_first_attempt_to_its_settlement() {
    let (_root, custody) = custody();
    assert!(!attempted(&custody));
    assert_eq!(custody.custody().unwrap().sequence, 0);
    custody.attempt(&attempt(&custody, 1), &[]).unwrap();
    custody.unknown("timed out".to_owned()).unwrap();
    custody.attempt(&attempt(&custody, 2), &[]).unwrap();
    assert!(attempted(&custody));
    assert_eq!(custody.custody().unwrap().sequence, 0);
    finalize(&custody, "receipt-1");
    let settled = custody.custody().unwrap();
    assert!(settled.pending.is_none());
    assert_eq!(settled.sequence, 4);
    assert_ne!(
        custody.use_key(0, "sha256:intent"),
        custody.use_key(settled.sequence, "sha256:intent")
    );
    custody.attempt(&attempt(&custody, 1), &[]).unwrap();
    assert_eq!(custody.custody().unwrap().sequence, 4);
}

#[test]
fn an_act_is_named_by_its_request_key_on_its_target() {
    let (root, custody) = custody();
    let store = KernelStore::open(root.path()).unwrap();
    let named = |target: &str, key: &str| {
        EffectCustody::for_act(
            Arc::clone(&store),
            &intent(target, key),
            &digest("intent"),
            "run-1",
        )
    };
    assert!(named("slack://T123/C456", "deliver-1") == custody);
    assert!(named("slack://T123/C789", "deliver-1") != custody);
    assert!(named("slack://T123/C456", "deliver-2") != custody);
}

#[test]
fn an_acknowledgement_survives_a_retry() {
    let (_root, custody) = custody();
    custody.attempt(&attempt(&custody, 1), &[]).unwrap();
    custody
        .acknowledged(Acknowledgement {
            response: JsonObject::new(),
            write_result: JsonValue::String("m-1".to_owned()),
        })
        .unwrap();
    custody.attempt(&attempt(&custody, 2), &[]).unwrap();
    assert!(
        custody
            .pending()
            .unwrap()
            .unwrap()
            .acknowledgement
            .is_some()
    );
}

#[test]
fn a_rejection_settles_and_a_different_intent_is_refused_while_pending() {
    let (_root, custody) = custody();
    custody.attempt(&attempt(&custody, 1), &[]).unwrap();
    let other = Attempt {
        intent_digest: "sha256:other",
        ..attempt(&custody, 2)
    };
    assert!(matches!(
        custody.attempt(&other, &[]),
        Err(CustodyError::Mismatch(_))
    ));
    custody
        .rejected("invalid_input".to_owned(), &[], false)
        .unwrap();
    assert!(custody.pending().unwrap().is_none());
    let other = Attempt {
        intent_digest: "sha256:other",
        ..attempt(&custody, 2)
    };
    custody.attempt(&other, &[]).unwrap();
}

#[test]
fn a_recovered_attempt_keeps_its_approval() {
    let (_root, custody) = custody();
    let human = ApprovalRecord {
        actor: ApprovalActor::Human,
        key: "approval-1".to_owned(),
        recorded_at: "2026-10-07T00:00:00Z".to_owned(),
    };
    custody
        .attempt(
            &Attempt {
                approval: Some(&human),
                ..attempt(&custody, 1)
            },
            &[],
        )
        .unwrap();
    let swapped = ApprovalRecord {
        actor: ApprovalActor::PaidExternalJob,
        key: "approval-2".to_owned(),
        recorded_at: "2026-10-07T00:00:00Z".to_owned(),
    };
    assert!(
        custody
            .attempt(
                &Attempt {
                    approval: Some(&swapped),
                    ..attempt(&custody, 2)
                },
                &[],
            )
            .is_err()
    );
}

#[test]
fn a_recovered_attempt_keeps_its_execution_term() {
    let (_root, custody) = custody();
    custody.attempt(&attempt(&custody, 1), &[]).unwrap();
    let changed = Attempt {
        authority_term_id: "term-2",
        ..attempt(&custody, 2)
    };
    assert!(matches!(
        custody.attempt(&changed, &[]),
        Err(CustodyError::Mismatch(_))
    ));
}

/// Two calls admitted against the same stream: the first attempt is recorded
/// and the second is refused before anything is sent, whatever it holds.
#[test]
fn a_call_admitted_before_another_attempt_is_refused() {
    let (_root, custody) = custody();
    let first = attempt(&custody, 1);
    let second = attempt(&custody, 1);
    custody.attempt(&first, &[]).unwrap();
    assert!(matches!(
        custody.attempt(&second, &[]),
        Err(CustodyError::Mismatch(reason)) if reason.contains("after this one was admitted")
    ));
    assert_eq!(custody.custody().unwrap().head, 1);
}

/// Settling is idempotent: recording the step again after a settlement
/// committed settles nothing twice and opens no new sequence.
#[test]
fn a_settlement_commits_once() {
    let (_root, custody) = custody();
    custody.attempt(&attempt(&custody, 1), &[]).unwrap();
    finalize(&custody, "receipt-1");
    let settled = custody.custody().unwrap();
    assert!(
        custody
            .settlement("receipt-1".to_owned(), &[])
            .appends(&custody.store)
            .unwrap()
            .is_empty()
    );
    assert_eq!(custody.custody().unwrap(), settled);
}

/// A recovered attempt keeps the instance binding it was made under: an
/// instance confirmed again with another credential is another attempt.
#[test]
fn a_recovered_attempt_keeps_its_instance_binding() {
    let (_root, custody) = custody();
    custody.attempt(&attempt(&custody, 1), &[]).unwrap();
    custody.unknown("timed out".to_owned()).unwrap();
    let rebound = Attempt {
        instance_binding: "sha256:rebound",
        ..attempt(&custody, 2)
    };
    assert!(matches!(
        custody.attempt(&rebound, &[]),
        Err(CustodyError::Mismatch(_))
    ));
}
