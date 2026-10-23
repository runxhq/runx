#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use runx_contracts::connector::OperationAccess;
use runx_contracts::{JsonObject, JsonValue};

use super::Store;
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver};

/// Run one store operation through the driver and return its result, with
/// the readback the kernel would take of an append.
struct Harness {
    env: BTreeMap<String, String>,
    parameters: JsonObject,
}

impl Harness {
    fn workspace(root: &std::path::Path) -> Self {
        Self {
            env: BTreeMap::from([
                ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
                (
                    crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
                    root.join(".runx").to_string_lossy().into_owned(),
                ),
            ]),
            parameters: JsonObject::new(),
        }
    }

    fn at(root: &std::path::Path, url: &str) -> Self {
        let mut harness = Self::workspace(root);
        harness
            .parameters
            .insert("url".to_owned(), JsonValue::String(url.to_owned()));
        harness
    }

    /// An append's `idempotency_key` here is the key the kernel would derive
    /// for the call's intent; it travels beside the input, never in it.
    fn call(
        &self,
        operation: &'static str,
        mut arguments: serde_json::Value,
    ) -> Result<Outcome, String> {
        let key = arguments
            .as_object_mut()
            .and_then(|object| object.remove("idempotency_key"))
            .and_then(|key| key.as_str().map(str::to_owned));
        let arguments: JsonValue = serde_json::from_value(arguments).unwrap();
        let access = if operation == "events.append" {
            OperationAccess::Mutate
        } else {
            OperationAccess::Read
        };
        let mut call = AdmittedCall::for_tests(&arguments, &self.parameters, &self.env, access);
        call.connector = "store";
        call.operation = operation;
        call.implementation = "store";
        call.idempotency_key = key.as_deref();
        let driver = Store { call: &call };
        let observed = driver.perform(None).map_err(|error| error.reason())?;
        let landed = (operation == "events.append")
            .then(|| {
                driver
                    .check(
                        Check::Readback {
                            readback: 0,
                            item: None,
                        },
                        &observed.value,
                    )
                    .map(|observed| observed.value)
            })
            .transpose()?;
        let (result, _) = driver.result(&observed.value, &[])?;
        Ok(Outcome {
            acknowledged: observed.value,
            landed,
            result,
        })
    }
}

fn number(value: u64) -> JsonValue {
    JsonValue::Number(runx_contracts::JsonNumber::U64(value))
}

struct Outcome {
    acknowledged: JsonValue,
    landed: Option<JsonValue>,
    result: JsonValue,
}

impl Outcome {
    fn field(&self, name: &str) -> &JsonValue {
        self.result.as_object().unwrap().get(name).unwrap()
    }

    fn text(&self, name: &str) -> &str {
        self.field(name).as_str().unwrap()
    }

    /// The readback agrees with the acknowledgement, as the kernel requires.
    fn confirmed(&self) -> bool {
        let landed = |value: &JsonValue| value.as_object().unwrap().get("landed").cloned();
        self.landed.as_ref().map(landed) == Some(landed(&self.acknowledged))
    }
}

fn append(key: &str, aggregate: &str, expected: u64, label: &str, at: &str) -> serde_json::Value {
    serde_json::json!({
        "data_source_uri": "local://board",
        "resource": "items",
        "aggregate_id": aggregate,
        "expected_version": expected,
        "idempotency_key": key,
        "event": {"type": "item.labelled", "label": label},
        "observed_at": at,
    })
}

/// Every backend runs the same streams to the same digests.
fn scenario(store: &Harness) -> Vec<String> {
    let mut digests = Vec::new();
    let first = store
        .call(
            "events.append",
            append("k1", "a", 0, "one", "2026-10-07T00:00:01Z"),
        )
        .unwrap();
    assert_eq!(first.text("status"), "committed");
    assert_eq!(first.field("after_version"), &number(1));
    assert!(first.confirmed());
    assert!(first.result.as_object().unwrap().get("landed").is_none());
    digests.push(first.text("projection_digest").to_owned());

    let replay = store
        .call(
            "events.append",
            append("k1", "a", 0, "one", "2026-10-07T00:00:01Z"),
        )
        .unwrap();
    assert_eq!(replay.text("status"), "idempotent_replay");
    assert_eq!(replay.text("event_digest"), first.text("event_digest"));
    assert!(replay.confirmed());

    let reused = store
        .call(
            "events.append",
            append("k1", "a", 1, "other", "2026-10-07T00:00:02Z"),
        )
        .unwrap();
    assert_eq!(reused.text("status"), "conflict");
    assert!(reused.confirmed(), "the key still holds the first event");

    let stale = store
        .call(
            "events.append",
            append("k2", "a", 0, "two", "2026-10-07T00:00:02Z"),
        )
        .unwrap();
    assert_eq!(stale.text("status"), "conflict");
    assert_eq!(
        stale
            .landed
            .as_ref()
            .unwrap()
            .as_object()
            .unwrap()
            .get("landed"),
        Some(&JsonValue::String("none".to_owned())),
        "nothing landed under a conflicting append's key"
    );
    assert!(stale.confirmed());

    for (key, aggregate, expected, label, at) in [
        ("k2", "a", 1, "two", "2026-10-07T00:00:02Z"),
        ("k3", "a", 2, "three", "2026-10-07T00:00:03Z"),
        ("k4", "b", 0, "one", "2026-10-07T00:00:04Z"),
    ] {
        let appended = store
            .call("events.append", append(key, aggregate, expected, label, at))
            .unwrap();
        assert_eq!(appended.text("status"), "committed");
        assert!(appended.confirmed());
        digests.push(appended.text("projection_digest").to_owned());
    }

    let stream = serde_json::json!({
        "data_source_uri": "local://board", "resource": "items", "aggregate_id": "a",
    });
    let tail = store
        .call("events.read", serde_json::json!({
            "data_source_uri": "local://board", "resource": "items", "aggregate_id": "a", "limit": 2,
        }))
        .unwrap();
    let versions = |outcome: &Outcome| {
        outcome
            .field("events")
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event.as_object().unwrap().get("version").cloned().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(versions(&tail), vec![number(2), number(3)]);
    let page = store
        .call(
            "events.read",
            serde_json::json!({
                "data_source_uri": "local://board", "resource": "items", "aggregate_id": "a",
                "limit": 2, "after_version": 0,
            }),
        )
        .unwrap();
    assert_eq!(versions(&page), vec![number(1), number(2)]);
    assert_eq!(page.field("has_more"), &JsonValue::Bool(true));

    let projection = store.call("projection.read", stream).unwrap();
    assert_eq!(projection.text("projection_digest"), digests[2]);
    digests.push(projection.text("result_digest").to_owned());

    let first_page = store
        .call(
            "heads.list",
            serde_json::json!({
                "data_source_uri": "local://board", "resource": "items", "limit": 1,
            }),
        )
        .unwrap();
    let projection = first_page.field("projection").as_object().unwrap();
    assert_eq!(projection.get("has_more"), Some(&JsonValue::Bool(true)));
    let cursor = projection
        .get("next_cursor")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    let newest = |outcome: &Outcome| {
        outcome.field("rows").as_array().unwrap()[0]
            .as_object()
            .unwrap()
            .get("aggregate_id")
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(newest(&first_page), "b");
    let second_page = store
        .call("heads.list", serde_json::json!({
            "data_source_uri": "local://board", "resource": "items", "limit": 1, "cursor": cursor,
        }))
        .unwrap();
    assert_eq!(newest(&second_page), "a");
    digests.push(first_page.text("result_digest").to_owned());
    digests.push(second_page.text("result_digest").to_owned());
    digests
}

#[test]
fn the_workspace_store_keeps_streams_beside_the_kernel() {
    let root = tempfile::tempdir().unwrap();
    scenario(&Harness::workspace(root.path()));
    let paths = crate::paths::RunxPaths::resolve(&BTreeMap::new(), root.path());
    assert!(paths.workspace_store().is_file());
}

/// What a store answers of the scenario's streams once they are written.
fn reads(store: &Harness) -> Vec<JsonValue> {
    [
        (
            "events.read",
            serde_json::json!({
                "data_source_uri": "local://board", "resource": "items", "aggregate_id": "a",
                "limit": 10, "after_version": 0,
            }),
        ),
        (
            "projection.read",
            serde_json::json!({
                "data_source_uri": "local://board", "resource": "items", "aggregate_id": "a",
            }),
        ),
        (
            "heads.list",
            serde_json::json!({
                "data_source_uri": "local://board", "resource": "items", "limit": 10,
            }),
        ),
    ]
    .into_iter()
    .map(|(operation, arguments)| store.call(operation, arguments).unwrap().result)
    .collect()
}

/// A version 1 workspace store, which named the `data_source_uri` and
/// `event_id` columns `data_source_ref` and `event_ref`, reads after its one
/// migration as it read before.
#[test]
fn a_version_1_sqlite_store_reads_as_it_did_after_its_migration() {
    let root = tempfile::tempdir().unwrap();
    let store = Harness::workspace(root.path());
    scenario(&store);
    let before = reads(&store);
    let path = crate::paths::RunxPaths::resolve(&BTreeMap::new(), root.path()).workspace_store();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE runx_events RENAME COLUMN data_source_uri TO data_source_ref;
             ALTER TABLE runx_events RENAME COLUMN event_id TO event_ref;
             ALTER TABLE runx_stream_heads RENAME COLUMN data_source_uri TO data_source_ref;
             ALTER TABLE runx_stream_heads RENAME COLUMN event_id TO event_ref;
             PRAGMA user_version = 1;",
        )
        .unwrap();

    assert_eq!(reads(&store), before);
    let version: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
}

#[test]
fn a_sqlite_store_refuses_a_file_that_is_not_a_store() {
    let root = tempfile::tempdir().unwrap();
    crate::KernelStore::open(&root.path().join(".runx")).unwrap();
    let store = Harness::at(
        root.path(),
        &format!(
            "sqlite:{}",
            root.path()
                .join(".runx")
                .join(crate::kernel::store::KERNEL_STORE_FILE)
                .display()
        ),
    );
    let refused = store
        .call(
            "projection.read",
            serde_json::json!({
                "data_source_uri": "local://board", "resource": "items", "aggregate_id": "a",
            }),
        )
        .err()
        .unwrap();
    assert!(refused.contains("not a runx store"), "{refused}");
}

#[test]
fn a_store_url_never_carries_a_password() {
    let root = tempfile::tempdir().unwrap();
    let store = Harness::at(root.path(), "postgres://app:secret@db.example/board");
    let refused = store
        .call(
            "projection.read",
            serde_json::json!({
                "data_source_uri": "local://board", "resource": "items", "aggregate_id": "a",
            }),
        )
        .err()
        .unwrap();
    assert!(refused.contains("credential"), "{refused}");
}

/// Postgres and Redis keep the same streams to the same digests as SQLite, and
/// a version 1 store of each reads after its migration as it read before.
/// They run against `RUNX_TEST_POSTGRES_URL` and `RUNX_TEST_REDIS_URL`, each a
/// database the test may write to, when set.
#[cfg(feature = "store")]
#[test]
fn every_backend_keeps_the_same_streams() {
    let root = tempfile::tempdir().unwrap();
    let reference = scenario(&Harness::workspace(root.path()));
    for variable in ["RUNX_TEST_POSTGRES_URL", "RUNX_TEST_REDIS_URL"] {
        let Ok(url) = std::env::var(variable) else {
            continue;
        };
        let isolated = tempfile::tempdir().unwrap();
        let store = Harness::at(isolated.path(), &url);
        clear(&url);
        assert_eq!(scenario(&store), reference, "{variable}");
        // A version 1 store reads after its one migration as it read before.
        let before = reads(&store);
        to_version_1(&url);
        assert_eq!(reads(&store), before, "{variable}");
        clear(&url);
    }
}

/// Rewrite a store the scenario wrote under the names version 1 kept.
#[cfg(feature = "store")]
fn to_version_1(url: &str) {
    if url.starts_with("redis") {
        let mut connection = redis::Client::open(url).unwrap().get_connection().unwrap();
        let keys = redis::Commands::scan_match::<_, String>(&mut connection, "runx:store:{*")
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for key in keys {
            let rest = key.split_once("}:").unwrap().1;
            let back =
                |text: &str, to: &str, from: &str| super::redis::renamed(text, to, from).unwrap();
            match rest.split(':').next() {
                Some("events") => {
                    let records: Vec<String> =
                        redis::Commands::lrange(&mut connection, &key, 0, -1).unwrap();
                    let records = records
                        .iter()
                        .map(|record| back(record, "event_id", "event_ref"))
                        .collect::<Vec<_>>();
                    redis::pipe()
                        .atomic()
                        .del(&key)
                        .rpush(&key, records)
                        .query::<()>(&mut connection)
                        .unwrap();
                }
                Some("keys" | "heads") => {
                    let entries: Vec<(String, String)> =
                        redis::Commands::hgetall(&mut connection, &key).unwrap();
                    for (field, record) in entries {
                        let record = back(&record, "event_id", "event_ref");
                        redis::Commands::hset::<_, _, _, ()>(&mut connection, &key, field, record)
                            .unwrap();
                    }
                }
                Some("projection") => {
                    let projection: String = redis::Commands::get(&mut connection, &key).unwrap();
                    let projection = back(&projection, "last_event_id", "last_event_ref");
                    redis::Commands::set::<_, _, ()>(&mut connection, &key, projection).unwrap();
                }
                _ => {}
            }
        }
        redis::Commands::del::<_, ()>(&mut connection, "runx:store:schema").unwrap();
    } else {
        let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
        client
            .batch_execute(
                "ALTER TABLE runx_store_events RENAME COLUMN data_source_uri TO data_source_ref;
                 ALTER TABLE runx_store_events RENAME COLUMN event_id TO event_ref;
                 ALTER TABLE runx_store_heads RENAME COLUMN data_source_uri TO data_source_ref;
                 ALTER TABLE runx_store_heads RENAME COLUMN event_id TO event_ref;
                 UPDATE runx_store_schema SET version = 1;",
            )
            .unwrap();
    }
}

#[cfg(feature = "store")]
fn clear(url: &str) {
    if url.starts_with("redis") {
        let mut connection = redis::Client::open(url).unwrap().get_connection().unwrap();
        let _: () = redis::cmd("FLUSHDB").query(&mut connection).unwrap();
    } else {
        let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
        client
            .batch_execute(
                "DROP TABLE IF EXISTS runx_store_events, runx_store_heads, runx_store_schema",
            )
            .unwrap();
    }
}
