#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;
use crate::kernel::store::KernelStore;

const DELEGATE: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_DELEGATE: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn approvals() -> (
    tempfile::TempDir,
    std::sync::Arc<KernelStore>,
    StandingApprovals,
) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = KernelStore::open(root.path()).expect("store");
    let approvals = StandingApprovals::new(std::sync::Arc::clone(&store));
    (root, store, approvals)
}

fn approval(total: u32, daily: u32) -> StandingApproval {
    StandingApproval {
        approval_id: "assistant-notify".to_owned(),
        connector: ConnectorId::new("slack").unwrap(),
        operation: OperationName::new("channel.post").unwrap(),
        instance_id: InstanceId::new("slack-acme").unwrap(),
        target: "slack://T123/C456".to_owned(),
        delegate_digest: DELEGATE.to_owned(),
        expires_at_unix_seconds: now_unix_seconds() + 3_600,
        max_total: total,
        max_per_day: daily,
        max_text_bytes: 200,
        allowed_mention: Some("<@U999>".to_owned()),
    }
}

fn call<'a>(key: &'a str, text: &'a str) -> StandingCall<'a> {
    StandingCall {
        connector: "slack",
        operation: "channel.post",
        instance_id: "slack-acme",
        target: "slack://T123/C456",
        delegate_digest: DELEGATE,
        text,
        intent_digest: key,
        idempotency_key: key,
        run_id: "run-1",
    }
}

/// Spend a use the way custody does: in its own commit.
fn reserve(store: &KernelStore, cover: &StandingCover) {
    if let Some(append) = cover.use_append().expect("append") {
        store.append(vec![append]).expect("commit");
    }
}

#[test]
fn usage_is_bounded_per_day_and_in_total() {
    let (_root, store, approvals) = approvals();
    approvals.grant(approval(3, 2)).expect("grant");
    let first = approvals
        .cover("assistant-notify", &call("k1", "ship it"))
        .expect("cover");
    reserve(&store, &first);
    let second = approvals
        .cover("assistant-notify", &call("k2", "ship it"))
        .expect("cover");
    reserve(&store, &second);
    assert!(matches!(
        approvals.cover("assistant-notify", &call("k3", "ship it")),
        Err(StandingError::Denied("the daily quota is exhausted"))
    ));
    assert_eq!(
        approvals
            .status("assistant-notify")
            .unwrap()
            .unwrap()
            .used_total,
        2
    );
    assert!(approvals.was_used("assistant-notify", "k1").unwrap());
    assert!(!approvals.was_used("assistant-notify", "k3").unwrap());
}

#[test]
fn a_recovered_call_reuses_its_recorded_use() {
    let (_root, store, approvals) = approvals();
    approvals.grant(approval(1, 1)).expect("grant");
    let cover = approvals
        .cover("assistant-notify", &call("k1", "ship it"))
        .expect("cover");
    reserve(&store, &cover);
    let recovered = approvals
        .cover("assistant-notify", &call("k1", "ship it"))
        .expect("recovery");
    assert!(recovered.use_append().unwrap().is_none());
    assert_eq!(recovered.approval_key, cover.approval_key);
}

#[test]
fn a_concurrent_use_cannot_exceed_the_quota() {
    let (_root, store, approvals) = approvals();
    approvals.grant(approval(1, 1)).expect("grant");
    let first = approvals
        .cover("assistant-notify", &call("k1", "one"))
        .expect("cover");
    let second = approvals
        .cover("assistant-notify", &call("k2", "two"))
        .expect("cover");
    reserve(&store, &first);
    let append = second.use_append().unwrap().unwrap();
    assert!(store.append(vec![append]).is_err());
}

#[test]
fn a_release_frees_the_use() {
    let (_root, store, approvals) = approvals();
    approvals.grant(approval(1, 1)).expect("grant");
    let cover = approvals
        .cover("assistant-notify", &call("k1", "ship it"))
        .expect("cover");
    reserve(&store, &cover);
    store
        .append(vec![cover.release_append(&store).unwrap()])
        .unwrap();
    assert!(
        approvals
            .cover("assistant-notify", &call("k2", "again"))
            .is_ok()
    );
}

#[test]
fn the_payload_rule_and_terms_are_enforced() {
    let (_root, _store, approvals) = approvals();
    approvals.grant(approval(5, 5)).expect("grant");
    for text in ["", "hey @channel", "<@U999> and <@U999>", "<@U111> hi"] {
        assert!(
            approvals
                .cover("assistant-notify", &call("k", text))
                .is_err(),
            "{text:?}"
        );
    }
    assert!(
        approvals
            .cover("assistant-notify", &call("k", "<@U999> deploy done"))
            .is_ok()
    );
    let mut elsewhere = call("k", "hi");
    elsewhere.target = "slack://T123/C999";
    assert!(approvals.cover("assistant-notify", &elsewhere).is_err());
}

#[test]
fn renewing_keeps_terms_and_revoking_refuses() {
    let (_root, _store, approvals) = approvals();
    approvals.grant(approval(5, 5)).expect("grant");
    approvals
        .grant(StandingApproval {
            delegate_digest: OTHER_DELEGATE.to_owned(),
            ..approval(5, 5)
        })
        .expect("renew");
    assert_eq!(
        approvals
            .status("assistant-notify")
            .unwrap()
            .unwrap()
            .approval
            .delegate_digest,
        OTHER_DELEGATE
    );
    assert!(approvals.grant(approval(9, 5)).is_err());
    approvals.revoke("assistant-notify").expect("revoke");
    let mut renewed = call("k", "hi");
    renewed.delegate_digest = OTHER_DELEGATE;
    assert!(matches!(
        approvals.cover("assistant-notify", &renewed),
        Err(StandingError::Denied("the standing approval is revoked"))
    ));
}

#[test]
fn renewal_waits_for_an_open_use() {
    let (_root, store, approvals) = approvals();
    approvals.grant(approval(5, 5)).expect("grant");
    let cover = approvals
        .cover("assistant-notify", &call("k1", "ship it"))
        .expect("cover");
    reserve(&store, &cover);
    let renewed = StandingApproval {
        delegate_digest: OTHER_DELEGATE.to_owned(),
        ..approval(5, 5)
    };
    assert!(matches!(
        approvals.grant(renewed.clone()),
        Err(StandingError::Denied(
            "a use of this standing approval is still open"
        ))
    ));
    store
        .append(vec![cover.consume_append(&store).unwrap()])
        .unwrap();
    approvals
        .grant(renewed)
        .expect("renewal after the use is consumed");
    assert_eq!(
        approvals
            .status("assistant-notify")
            .unwrap()
            .unwrap()
            .used_total,
        1
    );
}
