#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde::{Deserialize, Serialize};

use super::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Ledger {
    Opened { owner: String },
    Moved { amount: u64 },
}

impl StreamEvent for Ledger {
    const KIND: &'static str = "test-ledger";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Opened { .. } => "opened",
            Self::Moved { .. } => "moved",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Note {
    Written { text: String },
}

impl StreamEvent for Note {
    const KIND: &'static str = "note";

    fn event_type(&self) -> &'static str {
        "written"
    }
}

fn store() -> (tempfile::TempDir, Arc<KernelStore>) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = KernelStore::open(root.path()).expect("open");
    (root, store)
}

#[test]
fn appends_read_back_in_order_with_versions() {
    let (_root, store) = store();
    let opened = Ledger::Opened {
        owner: "kam".to_owned(),
    };
    let moved = Ledger::Moved { amount: 5 };
    assert_eq!(
        store
            .append(vec![
                Append::kernel("a", Expected::Absent, &opened).unwrap()
            ])
            .unwrap(),
        vec![1]
    );
    assert_eq!(
        store
            .append(vec![
                Append::kernel("a", Expected::Version(1), &moved).unwrap()
            ])
            .unwrap(),
        vec![2]
    );
    let events = store.read::<Ledger>("a").unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event.clone())
            .collect::<Vec<_>>(),
        vec![opened, moved.clone()]
    );
    assert_eq!(store.version::<Ledger>("a").unwrap(), 2);
    assert_eq!(store.version::<Ledger>("missing").unwrap(), 0);
    assert_eq!(store.latest::<Ledger>("a").unwrap().unwrap().event, moved);
    assert!(store.latest::<Ledger>("missing").unwrap().is_none());
}

#[test]
fn a_conflicting_stream_refuses_the_whole_batch() {
    let (_root, store) = store();
    store
        .append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 }).unwrap(),
        ])
        .unwrap();
    let error = store
        .append(vec![
            Append::kernel("b", Expected::Absent, &Ledger::Moved { amount: 2 }).unwrap(),
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 3 }).unwrap(),
        ])
        .expect_err("stale expectation");
    assert!(matches!(
        error,
        KernelStoreError::Conflict {
            actual: 1,
            expected: 0,
            ..
        }
    ));
    assert_eq!(
        store.version::<Ledger>("b").unwrap(),
        0,
        "nothing committed"
    );
}

#[test]
fn a_batch_spans_streams_atomically() {
    let (_root, store) = store();
    let versions = store
        .append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 }).unwrap(),
            Append::kernel("b", Expected::Absent, &Ledger::Moved { amount: 2 }).unwrap(),
        ])
        .unwrap();
    assert_eq!(versions, vec![1, 1]);
    let heads = store.heads::<Ledger>(None, 10).unwrap();
    assert_eq!(
        heads
            .iter()
            .map(|head| head.key.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

#[test]
fn rewritten_history_fails_the_chain() {
    let (_root, store) = store();
    store
        .append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 }).unwrap(),
        ])
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE kernel_events SET data = ?1 WHERE key = 'a'",
            params![br#"{"type":"moved","amount":999}"#.to_vec()],
        )
        .unwrap();
    assert!(matches!(
        store.read::<Ledger>("a"),
        Err(KernelStoreError::Integrity { version: 1, .. })
    ));
    assert!(matches!(
        store.latest::<Ledger>("a"),
        Err(KernelStoreError::Integrity { version: 1, .. })
    ));
}

#[test]
fn an_event_type_must_match_its_typed_payload() {
    let (_root, store) = store();
    let mut append = Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 }).unwrap();
    append.event_type = "opened";
    store.append(vec![append]).unwrap();
    assert!(matches!(
        store.latest::<Ledger>("a"),
        Err(KernelStoreError::Integrity { version: 1, .. })
    ));
    assert!(matches!(
        store.read::<Ledger>("a"),
        Err(KernelStoreError::Integrity { version: 1, .. })
    ));
}

#[test]
fn hosts_are_confined_to_their_namespace() {
    let (_root, store) = store();
    let assistant = store.host("assistant").unwrap();
    let append = assistant
        .append_of(
            "n1",
            Expected::Absent,
            &Note::Written {
                text: "hi".to_owned(),
            },
        )
        .unwrap();
    assistant.commit(vec![append]).unwrap();
    assert_eq!(assistant.version::<Note>("n1").unwrap(), 1);
    // The kernel's own `note` kind is a different stream.
    assert_eq!(store.version::<Note>("n1").unwrap(), 0);
    let kernel_append = Append::kernel(
        "n2",
        Expected::Absent,
        &Note::Written {
            text: "kernel".to_owned(),
        },
    )
    .unwrap();
    assert!(matches!(
        assistant.commit(vec![kernel_append]),
        Err(KernelStoreError::InvalidBatch)
    ));
    assert!(store.host("Assistant").is_err());
}

#[test]
fn host_snapshot_does_not_mix_atomic_commits() {
    let (_root, store) = store();
    let host = store.host("assistant").unwrap();
    host.commit(vec![
        host.append_of(
            "control",
            Expected::Absent,
            &Note::Written {
                text: "before".to_owned(),
            },
        )
        .unwrap(),
    ])
    .unwrap();
    let writer = Arc::new(KernelStore {
        path: store.path.clone(),
        connection: Mutex::new(Connection::open(store.path()).unwrap()),
    });
    let writer = writer.host("assistant").unwrap();
    let observed = host
        .snapshot(|snapshot| {
            let first = snapshot.latest::<Note>("control").unwrap().unwrap();
            writer
                .commit(vec![
                    writer
                        .append_of(
                            "control",
                            Expected::Version(1),
                            &Note::Written {
                                text: "after".to_owned(),
                            },
                        )
                        .unwrap(),
                ])
                .unwrap();
            let second = snapshot.latest::<Note>("control").unwrap().unwrap();
            (first.event, second.event)
        })
        .unwrap();
    assert_eq!(observed.0, observed.1);
    assert_eq!(
        host.latest::<Note>("control").unwrap().unwrap().event,
        Note::Written {
            text: "after".to_owned()
        }
    );
}

#[test]
fn a_foreign_database_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(KERNEL_STORE_FILE);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE runx_events (id INTEGER PRIMARY KEY);")
        .unwrap();
    drop(connection);
    assert!(matches!(
        KernelStore::open(root.path()),
        Err(KernelStoreError::Foreign { .. })
    ));
}

#[test]
fn receipt_destination_does_not_split_workspace_state() {
    let workspace = tempfile::tempdir().unwrap();
    let mut first_env = std::collections::BTreeMap::new();
    first_env.insert(
        crate::receipts::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
        workspace
            .path()
            .join("first-receipts")
            .display()
            .to_string(),
    );
    let first = KernelStore::for_workspace(&first_env, workspace.path()).unwrap();
    first
        .append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 }).unwrap(),
        ])
        .unwrap();

    let mut second_env = first_env;
    second_env.insert(
        crate::receipts::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
        workspace
            .path()
            .join("second-receipts")
            .display()
            .to_string(),
    );
    let second = KernelStore::for_workspace(&second_env, workspace.path()).unwrap();
    assert_eq!(first.path(), second.path());
    assert_eq!(second.version::<Ledger>("a").unwrap(), 1);
    assert_eq!(
        first.path(),
        workspace
            .path()
            .canonicalize()
            .unwrap()
            .join(".runx/kernel.sqlite")
    );
}
