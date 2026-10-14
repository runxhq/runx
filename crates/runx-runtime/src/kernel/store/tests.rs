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
        crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
        workspace
            .path()
            .join("first-receipts")
            .display()
            .to_string(),
    );
    let first = KernelStore::for_project(&crate::paths::RunxPaths::resolve(
        &first_env,
        workspace.path(),
    ))
    .unwrap();
    first
        .append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 }).unwrap(),
        ])
        .unwrap();

    let mut second_env = first_env;
    second_env.insert(
        crate::paths::RUNX_RECEIPT_DIR_ENV.to_owned(),
        workspace
            .path()
            .join("second-receipts")
            .display()
            .to_string(),
    );
    let second = KernelStore::for_project(&crate::paths::RunxPaths::resolve(
        &second_env,
        workspace.path(),
    ))
    .unwrap();
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

#[test]
fn blobs_commit_with_their_event_and_are_stored_once() {
    let (_root, store) = store();
    let blob = Blob::json(&serde_json::json!({"step": "plan", "output": [1, 2, 3]})).unwrap();
    let digest = blob.digest().to_owned();
    store
        .append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 })
                .unwrap()
                .with_blobs([blob.clone()]),
            Append::kernel("b", Expected::Absent, &Ledger::Moved { amount: 2 })
                .unwrap()
                .with_blobs([blob]),
        ])
        .unwrap();
    let value: serde_json::Value = store.blob_json(&digest).unwrap();
    assert_eq!(value["step"], "plan");
    let count: i64 = store
        .lock()
        .query_row("SELECT count(*) FROM kernel_blobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn a_refused_batch_stores_no_blob() {
    let (_root, store) = store();
    store
        .append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 }).unwrap(),
        ])
        .unwrap();
    let blob = Blob::new(b"orphan".to_vec()).unwrap();
    let digest = blob.digest().to_owned();
    assert!(matches!(
        store.append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 2 })
                .unwrap()
                .with_blobs([blob]),
        ]),
        Err(KernelStoreError::Conflict { .. })
    ));
    assert!(matches!(
        store.blob(&digest),
        Err(KernelStoreError::BlobIntegrity { .. })
    ));
}

#[test]
fn a_tampered_blob_fails_its_digest() {
    let (_root, store) = store();
    let blob = Blob::new(b"original".to_vec()).unwrap();
    let digest = blob.digest().to_owned();
    store
        .append(vec![
            Append::kernel("a", Expected::Absent, &Ledger::Moved { amount: 1 })
                .unwrap()
                .with_blobs([blob]),
        ])
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE kernel_blobs SET data = ?1 WHERE digest = ?2",
            params![b"tampered".to_vec(), digest],
        )
        .unwrap();
    assert!(matches!(
        store.blob(&digest),
        Err(KernelStoreError::BlobIntegrity { .. })
    ));
}

#[test]
fn recent_heads_page_newest_first() {
    let (_root, store) = store();
    for key in ["a", "b", "c"] {
        store
            .append(vec![
                Append::kernel(key, Expected::Absent, &Ledger::Moved { amount: 1 }).unwrap(),
            ])
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    store
        .append(vec![
            Append::kernel("a", Expected::Version(1), &Ledger::Moved { amount: 2 }).unwrap(),
        ])
        .unwrap();
    let first = store.recent_heads::<Ledger>(None, 2).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|head| head.key.as_str())
            .collect::<Vec<_>>(),
        ["a", "c"]
    );
    let rest = store.recent_heads::<Ledger>(first.last(), 2).unwrap();
    assert_eq!(
        rest.iter()
            .map(|head| head.key.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
}

#[test]
fn a_store_closes_when_its_last_holder_drops_it() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = KernelStore::open(root.path())?;
    let shared = KernelStore::open(root.path())?;
    assert!(Arc::ptr_eq(&store, &shared), "holders share one connection");
    let closed = Arc::downgrade(&store);
    drop((store, shared));
    assert!(closed.upgrade().is_none(), "the cache keeps no store alive");
    let reopened = KernelStore::open(root.path())?;
    assert_eq!(reopened.path(), closed_path(&root)?);
    Ok(())
}

fn closed_path(root: &tempfile::TempDir) -> Result<PathBuf, std::io::Error> {
    Ok(root.path().canonicalize()?.join(KERNEL_STORE_FILE))
}

fn moves(store: &KernelStore, key: &str, count: u64) {
    for amount in 1..=count {
        let expected = match amount {
            1 => Expected::Absent,
            _ => Expected::Version(amount - 1),
        };
        store
            .append(vec![
                Append::kernel(key, expected, &Ledger::Moved { amount }).unwrap(),
            ])
            .unwrap();
    }
}

#[test]
fn a_ranged_read_returns_what_follows_its_anchor() {
    let (_root, store) = store();
    moves(&store, "a", 5);
    let versions = |events: Vec<Recorded<Ledger>>| {
        events.iter().map(|event| event.version).collect::<Vec<_>>()
    };
    assert_eq!(
        versions(store.read_after::<Ledger>("a", 0, 10).unwrap()),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(
        versions(store.read_after::<Ledger>("a", 2, 10).unwrap()),
        [3, 4, 5]
    );
    assert_eq!(
        versions(store.read_after::<Ledger>("a", 2, 2).unwrap()),
        [3, 4]
    );
    assert!(store.read_after::<Ledger>("a", 5, 10).unwrap().is_empty());
    assert!(
        store
            .read_after::<Ledger>("missing", 0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store.read_after::<Ledger>("a", 9, 10),
        Err(KernelStoreError::Integrity { version: 9, .. })
    ));
}

#[test]
fn a_ranged_read_fails_a_rewritten_event_after_its_anchor() {
    let (_root, store) = store();
    moves(&store, "a", 3);
    store
        .lock()
        .execute(
            "UPDATE kernel_events SET data = ?1 WHERE key = 'a' AND version = 3",
            params![br#"{"type":"moved","amount":999}"#.to_vec()],
        )
        .unwrap();
    assert!(matches!(
        store.read_after::<Ledger>("a", 1, 10),
        Err(KernelStoreError::Integrity { version: 3, .. })
    ));
}

#[test]
fn a_tree_holds_its_root_and_descendants_only() {
    let (_root, store) = store();
    for key in [
        "run", "run/a", "run/a/0", "run/b", "run-x", "run.x", "run0", "runx", "ru",
    ] {
        moves(&store, key, 1);
    }
    let keys = store
        .tree_heads::<Ledger>("run")
        .unwrap()
        .into_iter()
        .map(|head| head.key)
        .collect::<Vec<_>>();
    assert_eq!(keys, ["run", "run/a", "run/a/0", "run/b"]);
    assert!(store.tree_heads::<Ledger>("missing").unwrap().is_empty());
}
