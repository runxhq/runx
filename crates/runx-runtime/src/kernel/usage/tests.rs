#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;

#[test]
fn a_retry_reuses_its_reservation_and_a_rejection_releases_it() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    let ledger = UsageLedger::new(Arc::clone(&store));
    let limit = AuthorityUseLimit {
        total: Some(1),
        per_day: Some(1),
    };
    let first = ledger
        .cover("term:one", &limit, "call-1", "intent-1", "run-1", 86_400)
        .unwrap();
    store
        .append(vec![first.reservation().unwrap().unwrap()])
        .unwrap();
    assert!(
        ledger
            .cover("term:one", &limit, "call-1", "intent-1", "run-1", 86_401)
            .unwrap()
            .recorded
    );
    assert!(matches!(
        ledger.cover("term:one", &limit, "call-2", "intent-2", "run-2", 86_401),
        Err(UsageError::TotalExhausted)
    ));
    store.append(vec![first.release(&store).unwrap()]).unwrap();
    let second = ledger
        .cover("term:one", &limit, "call-2", "intent-2", "run-2", 86_401)
        .unwrap();
    assert!(!second.recorded);
}

#[test]
fn another_run_recovering_the_sequence_reuses_its_use() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    let ledger = UsageLedger::new(Arc::clone(&store));
    let limit = AuthorityUseLimit {
        total: Some(1),
        per_day: None,
    };
    let first = ledger
        .cover("term:one", &limit, "call-1", "intent-1", "run-1", 86_400)
        .unwrap();
    store
        .append(vec![first.reservation().unwrap().unwrap()])
        .unwrap();
    assert!(
        ledger
            .cover("term:one", &limit, "call-1", "intent-1", "run-2", 86_400)
            .unwrap()
            .recorded
    );
}

#[test]
fn consumption_keeps_the_use() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    let ledger = UsageLedger::new(Arc::clone(&store));
    let limit = AuthorityUseLimit {
        total: Some(1),
        per_day: None,
    };
    let cover = ledger
        .cover("term:one", &limit, "call-1", "intent-1", "run-1", 86_400)
        .unwrap();
    store
        .append(vec![cover.reservation().unwrap().unwrap()])
        .unwrap();
    assert_eq!(ledger.status("term:one").unwrap().open_total, 1);
    store.append(vec![cover.consume(&store).unwrap()]).unwrap();
    let after = ledger.status("term:one").unwrap();
    assert_eq!(after.open_total, 0);
    assert_eq!(after.active_total, 1);
}
