#![allow(clippy::expect_used, clippy::unwrap_used)]

use runx_contracts::JsonValue;

use super::*;
use crate::kernel::store::KernelStore;

fn custody() -> (tempfile::TempDir, EffectCustody) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = KernelStore::open(root.path()).expect("store");
    (
        root,
        EffectCustody::open(store, "run-1", "/skills/slack", "deliver"),
    )
}

fn attempt(number: u32) -> Attempt<'static> {
    Attempt {
        intent_digest: "sha256:intent",
        authority_term_id: "term-1",
        mandate_ref: None,
        idempotency_key: "runx:sha256:intent",
        instance_id: "slack-acme",
        attempt: number,
        approval: None,
        implementation_digest: None,
    }
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
    custody.attempt(&attempt(1), None, &[]).unwrap();
    custody.unknown("timed out".to_owned()).unwrap();
    let pending = custody.pending().unwrap().expect("pending");
    assert!(pending.unknown);
    assert_eq!(pending.attempt, 1);

    custody.attempt(&attempt(2), None, &[]).unwrap();
    custody.read_back(readback()).unwrap();
    let pending = custody.pending().unwrap().expect("pending");
    assert_eq!(pending.attempt, 2);
    assert_eq!(pending.readback, Some(readback()));
    assert!(!pending.unknown);

    custody
        .finalized("receipt-1".to_owned(), None, &[])
        .unwrap();
    assert!(custody.pending().unwrap().is_none());
}

#[test]
fn an_acknowledgement_survives_a_retry() {
    let (_root, custody) = custody();
    custody.attempt(&attempt(1), None, &[]).unwrap();
    custody
        .acknowledged(Acknowledgement {
            response: JsonObject::new(),
            write_result: JsonValue::String("m-1".to_owned()),
        })
        .unwrap();
    custody.attempt(&attempt(2), None, &[]).unwrap();
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
    custody.attempt(&attempt(1), None, &[]).unwrap();
    let other = Attempt {
        intent_digest: "sha256:other",
        ..attempt(2)
    };
    assert!(matches!(
        custody.attempt(&other, None, &[]),
        Err(CustodyError::Mismatch(_))
    ));
    custody
        .rejected("invalid_input".to_owned(), None, &[], false)
        .unwrap();
    assert!(custody.pending().unwrap().is_none());
    custody.attempt(&other, None, &[]).unwrap();
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
                ..attempt(1)
            },
            None,
            &[],
        )
        .unwrap();
    let swapped = ApprovalRecord {
        actor: ApprovalActor::Standing,
        key: "approval-2".to_owned(),
        recorded_at: "2026-10-07T00:00:00Z".to_owned(),
    };
    assert!(
        custody
            .attempt(
                &Attempt {
                    approval: Some(&swapped),
                    ..attempt(2)
                },
                None,
                &[],
            )
            .is_err()
    );
}

#[test]
fn a_recovered_attempt_keeps_its_execution_term() {
    let (_root, custody) = custody();
    custody.attempt(&attempt(1), None, &[]).unwrap();
    let changed = Attempt {
        authority_term_id: "term-2",
        ..attempt(2)
    };
    assert!(matches!(
        custody.attempt(&changed, None, &[]),
        Err(CustodyError::Mismatch(_))
    ));
}
