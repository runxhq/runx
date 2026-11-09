#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use runx_contracts::connector::OperationAccess;
use runx_contracts::{JsonObject, JsonValue};

use super::Store;
use crate::identity::ChainLink;
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver};

/// The package the scenario's calls are made from.
const OWNER: &str = "project:board";

/// Run one store operation through the driver and return its result, with
/// the readback the kernel would take of an append.
struct Test {
    env: BTreeMap<String, String>,
    parameters: JsonObject,
    connector: &'static str,
    writer: ChainLink,
}

impl Test {
    fn workspace(root: &std::path::Path) -> Self {
        Self {
            env: BTreeMap::from([
                ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
                (
                    crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
                    root.join(".runx").to_string_lossy().into_owned(),
                ),
                (
                    "RUNX_HOME".to_owned(),
                    root.join("home").to_string_lossy().into_owned(),
                ),
            ]),
            parameters: JsonObject::new(),
            connector: "store",
            writer: link(
                OWNER,
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        }
    }

    fn at(root: &std::path::Path, url: &str) -> Self {
        let mut test = Self::workspace(root);
        test.parameters
            .insert("url".to_owned(), JsonValue::String(url.to_owned()));
        test
    }

    fn private(root: &std::path::Path) -> Self {
        Self {
            connector: super::PRIVATE,
            ..Self::workspace(root)
        }
    }

    /// `operation` on `stream`, rendered under the package the call is made
    /// from, or the package a read names. An append's `idempotency_key` here
    /// is the key the kernel would derive for the call's intent; it travels
    /// beside the input, never in it.
    fn call(
        &self,
        operation: &'static str,
        mut arguments: serde_json::Value,
    ) -> Result<Outcome, String> {
        let key = arguments
            .as_object_mut()
            .and_then(|object| object.remove("idempotency_key"))
            .and_then(|key| key.as_str().map(str::to_owned));
        let owner = arguments
            .get("package")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&self.writer.owner)
            .to_owned();
        let target = format!(
            "{owner}/{}",
            arguments
                .get("stream")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
        );
        let arguments: JsonValue = serde_json::from_value(arguments).unwrap();
        let access = if operation == "events.append" {
            OperationAccess::Mutate
        } else {
            OperationAccess::Read
        };
        let mut call = AdmittedCall::for_tests(&arguments, &self.parameters, &self.env, access);
        call.connector = self.connector;
        call.operation = operation;
        call.implementation = "store";
        call.target = &target;
        call.idempotency_key = key.as_deref();
        call.writer = Some(&self.writer);
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
        let (result, operation_id) = driver.result(&observed.value, &[])?;
        conforms(&result);
        Ok(Outcome {
            acknowledged: observed.value,
            landed,
            result,
            operation_id,
        })
    }
}

/// Every answer is one shape of `runx.store.result.v1`, as its generated
/// schema states it.
fn conforms(result: &JsonValue) {
    use runx_contracts::schema::RunxSchema as _;
    let schema = runx_contracts::StoreResult::json_schema();
    let validator = jsonschema::draft202012::options().build(&schema).unwrap();
    let instance = serde_json::to_value(result).unwrap();
    let errors = validator
        .iter_errors(&instance)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "{instance}: {errors:?}");
    serde_json::from_value::<runx_contracts::StoreResult>(instance).unwrap();
}

fn link(package: &str, digest: &str) -> ChainLink {
    ChainLink {
        package: runx_contracts::PackagePattern::new(package).unwrap(),
        owner: package.to_owned(),
        root: std::path::PathBuf::from("/packages").join(package),
        digest: digest.to_owned(),
    }
}

fn number(value: u64) -> JsonValue {
    JsonValue::Number(runx_contracts::JsonNumber::U64(value))
}

struct Outcome {
    acknowledged: JsonValue,
    landed: Option<JsonValue>,
    result: JsonValue,
    operation_id: Option<String>,
}

impl Outcome {
    fn field(&self, name: &str) -> &JsonValue {
        self.result
            .as_object()
            .unwrap()
            .get(name)
            .ok_or_else(|| format!("no {name} in {:?}", self.result))
            .unwrap()
    }

    fn text(&self, name: &str) -> &str {
        self.field(name).as_str().unwrap()
    }

    fn has(&self, name: &str) -> bool {
        self.result.as_object().unwrap().contains_key(name)
    }

    /// The readback agrees with the acknowledgement, as the kernel requires.
    fn confirmed(&self) -> bool {
        let landed = |value: &JsonValue| value.as_object().unwrap().get("landed").cloned();
        self.landed.as_ref().map(landed) == Some(landed(&self.acknowledged))
    }

    fn versions(&self) -> Vec<JsonValue> {
        self.field("events")
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event.as_object().unwrap().get("version").cloned().unwrap())
            .collect()
    }

    fn keys(&self, field: &str) -> Vec<String> {
        self.field(field)
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                entry.as_object().unwrap()["key"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
}

fn append(idempotency: &str, key: &str, expected: u64, label: &str) -> serde_json::Value {
    serde_json::json!({
        "stream": "items",
        "key": key,
        "expected_version": expected,
        "idempotency_key": idempotency,
        "type": "item.labelled",
        "data": {"label": label},
    })
}

/// Every backend runs the same keys to the same digests. Commit times are
/// the store's own, so what's compared is what doesn't depend on them.
fn scenario(store: &Test) -> Vec<String> {
    let mut digests = Vec::new();
    let first = store
        .call("events.append", append("k1", "a", 0, "one"))
        .unwrap();
    assert_eq!(first.text("schema"), "runx.store.result.v1");
    assert_eq!(first.text("stream"), "project:board/items");
    assert_eq!(first.text("status"), "committed");
    assert_eq!(
        (first.field("before_version"), first.field("version")),
        (&number(0), &number(1))
    );
    assert!(first.confirmed());
    assert!(!first.has("landed"));
    let event = first.field("event").as_object().unwrap();
    assert_eq!(
        event["id"],
        JsonValue::String(super::model::event_id("project:board/items", "a", 1))
    );
    assert_eq!(first.operation_id.as_deref(), event["id"].as_str());
    digests.push(first.text("chain_digest").to_owned());

    // The same key with the same event answers as the first time.
    let replay = store
        .call("events.append", append("k1", "a", 0, "one"))
        .unwrap();
    assert_eq!(replay.text("status"), "replayed");
    assert_eq!(replay.field("event"), first.field("event"));
    assert_eq!(
        (replay.field("before_version"), replay.field("version")),
        (&number(0), &number(1))
    );
    assert!(replay.confirmed());

    // The key used for another event conflicts, checked before the version,
    // and appends nothing.
    let reused = store
        .call("events.append", append("k1", "a", 1, "other"))
        .unwrap();
    assert_eq!(reused.text("status"), "idempotency_conflict");
    assert!(!reused.has("event"));
    assert!(reused.confirmed(), "the key still holds the first event");

    let stale = store
        .call("events.append", append("k2", "a", 0, "two"))
        .unwrap();
    assert_eq!(stale.text("status"), "version_conflict");
    assert!(!stale.has("event"));
    assert_eq!(stale.field("version"), &number(1));
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

    for (idempotency, key, expected, label) in [
        ("k2", "a", 1, "two"),
        ("k3", "a", 2, "three"),
        ("k4", "b", 0, "one"),
    ] {
        let appended = store
            .call("events.append", append(idempotency, key, expected, label))
            .unwrap();
        assert_eq!(appended.text("status"), "committed");
        assert!(appended.confirmed());
        digests.push(appended.text("chain_digest").to_owned());
    }

    // Without after_version, the latest page, in version order.
    let tail = store
        .call(
            "events.read",
            serde_json::json!({"stream": "items", "key": "a", "limit": 2}),
        )
        .unwrap();
    assert_eq!(tail.versions(), vec![number(2), number(3)]);
    assert_eq!(tail.field("last_version"), &number(3));
    let page = store
        .call(
            "events.read",
            serde_json::json!({"stream": "items", "key": "a", "limit": 2, "after_version": 0}),
        )
        .unwrap();
    assert_eq!(page.versions(), vec![number(1), number(2)]);
    assert_eq!(page.field("has_more"), &JsonValue::Bool(true));
    assert_eq!(page.field("last_version"), &number(2));
    let past = store
        .call(
            "events.read",
            serde_json::json!({"stream": "items", "key": "a", "after_version": 3}),
        )
        .unwrap();
    assert!(past.versions().is_empty());
    assert_eq!(
        past.field("last_version"),
        &number(3),
        "with no events, the version read after"
    );
    assert_eq!(past.field("version"), &number(3));

    let inspected = store
        .call(
            "key.inspect",
            serde_json::json!({"stream": "items", "key": "a"}),
        )
        .unwrap();
    assert_eq!(inspected.text("chain_digest"), digests[2]);
    assert_eq!(inspected.field("version"), &number(3));
    let last = inspected.field("last_event").as_object().unwrap();
    assert_eq!(
        last["data"],
        serde_json::from_value::<JsonValue>(serde_json::json!({"label": "three"})).unwrap()
    );
    assert_eq!(
        last["writer"],
        serde_json::from_value::<JsonValue>(
            serde_json::json!({"package": OWNER, "digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"})
        )
        .unwrap()
    );
    let empty = store
        .call(
            "key.inspect",
            serde_json::json!({"stream": "items", "key": "none"}),
        )
        .unwrap();
    assert_eq!(empty.field("version"), &number(0));
    assert_eq!(empty.field("last_event"), &JsonValue::Null);

    // Newest commit first, a page at a time, and no cursor past the last.
    let first_page = store
        .call(
            "keys.list",
            serde_json::json!({"stream": "items", "limit": 1}),
        )
        .unwrap();
    assert_eq!(first_page.keys("keys"), vec!["b"]);
    let cursor = first_page.text("next_cursor").to_owned();
    let second_page = store
        .call(
            "keys.list",
            serde_json::json!({"stream": "items", "limit": 1, "cursor": cursor}),
        )
        .unwrap();
    assert_eq!(second_page.keys("keys"), vec!["a"]);
    let end = store
        .call(
            "keys.list",
            serde_json::json!({"stream": "items", "limit": 5}),
        )
        .unwrap();
    assert_eq!(end.keys("keys"), vec!["b", "a"]);
    assert_eq!(end.field("next_cursor"), &JsonValue::Null);
    // Only the keys whose newest event has one of the types.
    let typed = store
        .call(
            "keys.list",
            serde_json::json!({"stream": "items", "types": ["item.removed"]}),
        )
        .unwrap();
    assert!(typed.keys("keys").is_empty());
    // An event's digest covers what it says, never when it was committed or
    // where, so every backend holds the same one.
    digests.push(last["digest"].as_str().unwrap().to_owned());
    digests
}

#[test]
fn the_workspace_store_keeps_streams_beside_the_kernel() {
    let root = tempfile::tempdir().unwrap();
    scenario(&Test::workspace(root.path()));
    let paths = crate::paths::RunxPaths::resolve(&BTreeMap::new(), root.path());
    assert!(paths.workspace_store().is_file());
}

/// A write never names a package: the stream it reaches is its own
/// package's, and another package's can't be spelled.
#[test]
fn an_append_reaches_only_the_stream_admission_rendered() {
    let root = tempfile::tempdir().unwrap();
    let store = Test::workspace(root.path());
    let mut named = append("k1", "a", 0, "one");
    named
        .as_object_mut()
        .unwrap()
        .insert("package".to_owned(), "runx/reddit".into());
    assert!(store.call("events.append", named).is_err());
    let mut slashed = append("k1", "a", 0, "one");
    slashed["stream"] = "runx/reddit/accounts".into();
    assert!(store.call("events.append", slashed).is_err());
    let mut untyped = append("k1", "a", 0, "one");
    untyped["type"] = "labelled".into();
    assert!(
        store.call("events.append", untyped).is_err(),
        "a type is dotted words"
    );
}

/// An event's digest covers its writer, its type and its data, so the same
/// data from other bytes is another event; one written before writers were
/// recorded covers its data alone, as it was written.
#[test]
fn an_events_digest_covers_its_writer() {
    use super::model::{EventRecord, Writer, event_digest};
    let data = JsonObject::from([("label".to_owned(), JsonValue::String("one".to_owned()))]);
    let event = |writer: Option<Writer>, event_type: &str| EventRecord {
        id: String::new(),
        version: 1,
        event_type: event_type.to_owned(),
        data: data.clone(),
        digest: String::new(),
        idempotency_key: "k".to_owned(),
        committed_at: "2026-10-07T00:00:00.000Z".to_owned(),
        writer,
    };
    let writer = |digest: &str| Writer {
        package: OWNER.to_owned(),
        digest: digest.to_owned(),
    };
    let one = event_digest(&event(
        Some(writer(&format!("sha256:{}", "1".repeat(64)))),
        "item.labelled",
    ))
    .unwrap();
    assert_ne!(
        one,
        event_digest(&event(
            Some(writer(&format!("sha256:{}", "2".repeat(64)))),
            "item.labelled"
        ))
        .unwrap()
    );
    assert_ne!(
        one,
        event_digest(&event(
            Some(writer(&format!("sha256:{}", "1".repeat(64)))),
            "item.renamed"
        ))
        .unwrap()
    );
    assert_eq!(
        event_digest(&event(None, "item.labelled")).unwrap(),
        super::model::digest(&JsonValue::Object(data.clone())).unwrap()
    );
}

/// An event's id delimits its stream and key by length, so a `:` in either
/// never makes two events one.
#[test]
fn an_event_id_never_collides_across_a_colon() {
    use super::model::event_id;
    assert_ne!(event_id("a:b", "c", 1), event_id("a", "b:c", 1));
    assert_ne!(event_id("s", "k:1", 2), event_id("s", "k", 1));
    assert_eq!(
        event_id("runx/reddit/accounts", "alice", 3),
        "20:runx/reddit/accounts,5:alice,3"
    );
}

/// A store an earlier runx wrote moves to the streams and keys 0.12 names
/// when it's first opened, keeping every event's digest and each key's
/// chain; version 1's names are carried over first.
#[test]
fn a_version_1_or_2_sqlite_store_migrates_to_owned_streams() {
    for version in [1, 2] {
        let root = tempfile::tempdir().unwrap();
        let path =
            crate::paths::RunxPaths::resolve(&BTreeMap::new(), root.path()).workspace_store();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let (digests, chain) = old::write_sqlite(&path, version);
        let store = Test {
            writer: link(
                "runx/reddit",
                "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            ),
            ..Test::workspace(root.path())
        };
        let read = store
            .call(
                "events.read",
                serde_json::json!({"stream": "accounts", "key": "alice", "after_version": 0}),
            )
            .unwrap();
        assert_eq!(read.text("stream"), "runx/reddit/accounts");
        assert_eq!(read.text("chain_digest"), chain, "version {version}");
        let events = read.field("events").as_array().unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.as_object().unwrap()["digest"]
                    .as_str()
                    .unwrap()
                    .to_owned())
                .collect::<Vec<_>>(),
            digests
        );
        assert!(
            events
                .iter()
                .all(|event| !event.as_object().unwrap().contains_key("writer"))
        );
        // The migrated key takes its next event from its owner.
        let next = store
            .call(
                "events.append",
                serde_json::json!({"stream": "accounts", "key": "alice", "expected_version": 2,
                    "idempotency_key": "k-next", "type": "reddit.account", "data": {}}),
            )
            .unwrap();
        assert_eq!(next.text("status"), "committed");
        let unowned = Test {
            writer: link(
                OWNER,
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
            ..Test::workspace(root.path())
        };
        let other = unowned
            .call(
                "events.read",
                serde_json::json!({"package": "unowned", "stream": "ops_projection", "key": "workspace"}),
            )
            .unwrap();
        assert_eq!(other.versions(), vec![number(1)]);
        let connection = rusqlite::Connection::open(&path).unwrap();
        let stored: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, 3);
        let old_tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name LIKE 'runx_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_tables, 0);
    }
}

#[test]
fn a_sqlite_store_refuses_a_file_that_is_not_a_store() {
    let root = tempfile::tempdir().unwrap();
    crate::KernelStore::open(&root.path().join(".runx")).unwrap();
    let store = Test::at(
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
            "key.inspect",
            serde_json::json!({"stream": "items", "key": "a"}),
        )
        .err()
        .unwrap();
    assert!(refused.contains("not a runx store"), "{refused}");
}

/// `store` and `private` share the driver and the tables and keep their
/// own files: a `store` instance naming the private file is refused, so the
/// person's own data is reached only through `private`, and the reverse.
#[test]
fn store_and_private_never_open_each_others_file() {
    let root = tempfile::tempdir().unwrap();
    let private = Test::private(root.path());
    private
        .call("events.append", append("k1", "a", 0, "one"))
        .unwrap();
    let file = crate::paths::RunxPaths::resolve(&private.env, root.path()).private_store();
    assert!(file.is_file());
    let store = Test::at(root.path(), &format!("sqlite:{}", file.display()));
    let refused = store
        .call(
            "key.inspect",
            serde_json::json!({"stream": "items", "key": "a"}),
        )
        .err()
        .unwrap();
    assert!(refused.contains("not a runx store"), "{refused}");
    Test::workspace(root.path())
        .call("events.append", append("k1", "a", 0, "one"))
        .unwrap();
    let workspace = crate::paths::RunxPaths::resolve(&private.env, root.path()).workspace_store();
    let mut reversed = Test::private(root.path());
    reversed.parameters.insert(
        "url".to_owned(),
        JsonValue::String(format!("sqlite:{}", workspace.display())),
    );
    let refused = reversed
        .call(
            "key.inspect",
            serde_json::json!({"stream": "items", "key": "a"}),
        )
        .err()
        .unwrap();
    assert!(refused.contains("not a private store"), "{refused}");
}

/// `keys.search` ranks one stream's keys by their newest event, quotes its
/// query, pages in score order with ties by key, filters by type, and is
/// `private`'s alone.
#[test]
fn private_searches_one_streams_current_entries() {
    let root = tempfile::tempdir().unwrap();
    let private = Test::private(root.path());
    let keep = |idempotency: &str, key: &str, expected: u64, text: &str| {
        let mut call = append(idempotency, key, expected, text);
        call["data"] = serde_json::json!({"text": text});
        let kept = private.call("events.append", call).unwrap();
        assert_eq!(kept.text("status"), "committed");
    };
    keep("k1", "tea", 0, "tea black");
    keep("k2", "chai", 0, "tea spiced");
    keep("k3", "tea", 1, "tea with milk");
    keep("k4", "coffee", 0, "coffee before noon");
    let search = |query: &str, extra: serde_json::Value| {
        let mut call = serde_json::json!({"stream": "items", "query": query});
        call.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        private.call("keys.search", call).unwrap()
    };
    let teas = search("tea", serde_json::json!({}));
    let mut found = teas.keys("matches");
    found.sort();
    assert_eq!(found, vec!["chai", "tea"]);
    assert!(
        search("black", serde_json::json!({}))
            .keys("matches")
            .is_empty(),
        "history is never searched"
    );
    // One page at a time, and the pages are stable.
    let first = search("tea", serde_json::json!({"limit": 1}));
    let cursor = first.text("next_cursor").to_owned();
    let second = search("tea", serde_json::json!({"limit": 1, "cursor": cursor}));
    assert_eq!(second.field("next_cursor"), &JsonValue::Null);
    let mut paged = first.keys("matches");
    paged.extend(second.keys("matches"));
    assert_eq!(paged, teas.keys("matches"));
    assert!(
        search("tea", serde_json::json!({"types": ["item.removed"]}))
            .keys("matches")
            .is_empty()
    );
    // FTS5's own syntax is words.
    assert_eq!(
        search("coffee) OR \"", serde_json::json!({})).keys("matches"),
        vec!["coffee"]
    );
    // Another stream holds none of it.
    let other = private
        .call(
            "keys.search",
            serde_json::json!({"stream": "other", "query": "tea"}),
        )
        .unwrap();
    assert!(other.keys("matches").is_empty());
    // store never searches.
    let store = Test::workspace(root.path());
    assert!(
        store
            .call(
                "keys.search",
                serde_json::json!({"stream": "items", "query": "tea"})
            )
            .is_err()
    );
}

#[test]
fn a_store_url_never_carries_a_password() {
    let root = tempfile::tempdir().unwrap();
    let store = Test::at(root.path(), "postgres://app:secret@db.example/board");
    let refused = store
        .call(
            "key.inspect",
            serde_json::json!({"stream": "items", "key": "a"}),
        )
        .err()
        .unwrap();
    assert!(refused.contains("credential"), "{refused}");
}

/// A `private` instance serves every operation of `private`, so one on a
/// backend that can't search is refused when it's configured.
#[test]
fn a_private_instance_is_configured_only_where_it_can_search() {
    let url =
        |value: &str| JsonObject::from([("url".to_owned(), JsonValue::String(value.to_owned()))]);
    for refused in [
        "postgres://app@db.example/board",
        "redis://cache.example:6379",
    ] {
        let reason = super::configurable(super::PRIVATE, &url(refused)).unwrap_err();
        assert!(reason.contains("keys.search"), "{reason}");
        assert!(super::configurable("store", &url(refused)).is_ok());
    }
    assert!(super::configurable(super::PRIVATE, &url("sqlite:notes.sqlite")).is_ok());
    assert!(super::configurable(super::PRIVATE, &JsonObject::new()).is_ok());
}

/// Postgres and Redis keep the same keys to the same digests as SQLite, and
/// a version 1 or 2 store of each migrates as SQLite's does. They run
/// against `RUNX_TEST_POSTGRES_URL` and `RUNX_TEST_REDIS_URL`, each a
/// database the test may write to, when set.
#[cfg(feature = "store")]
#[test]
fn every_backend_keeps_the_same_streams() {
    let root = tempfile::tempdir().unwrap();
    let reference = scenario(&Test::workspace(root.path()));
    for variable in ["RUNX_TEST_POSTGRES_URL", "RUNX_TEST_REDIS_URL"] {
        let Ok(url) = std::env::var(variable) else {
            continue;
        };
        let isolated = tempfile::tempdir().unwrap();
        let store = Test::at(isolated.path(), &url);
        old::clear(&url);
        assert_eq!(scenario(&store), reference, "{variable}");
        for version in [1, 2] {
            old::clear(&url);
            let (digests, chain) = old::write(&url, version);
            let reddit = Test {
                writer: link(
                    "runx/reddit",
                    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                ),
                ..Test::at(isolated.path(), &url)
            };
            let read = reddit
                .call(
                    "events.read",
                    serde_json::json!({"stream": "accounts", "key": "alice", "after_version": 0}),
                )
                .unwrap();
            assert_eq!(
                read.text("chain_digest"),
                chain,
                "{variable} version {version}"
            );
            assert_eq!(
                read.field("events")
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|event| event.as_object().unwrap()["digest"]
                        .as_str()
                        .unwrap()
                        .to_owned())
                    .collect::<Vec<_>>(),
                digests,
                "{variable} version {version}"
            );
            let other = Test::at(isolated.path(), &url)
                .call(
                    "events.read",
                    serde_json::json!({"package": "unowned", "stream": "ops_projection", "key": "workspace"}),
                )
                .unwrap();
            assert_eq!(
                other.versions(),
                vec![number(1)],
                "{variable} version {version}"
            );
        }
        old::clear(&url);
    }
}

/// Stores as runx 0.11 and earlier wrote them: streams addressed by data
/// source, resource and aggregate, events with no writer. Version 1 named
/// `data_source_uri` and `event_id` `data_source_ref` and `event_ref`.
mod old {
    use runx_contracts::{JsonObject, JsonValue};

    use super::super::model;

    /// reddit's account for alice, two events, and a stream no first-party
    /// manifest names.
    pub(super) fn streams() -> Vec<(&'static str, &'static str, &'static str, Vec<JsonObject>)> {
        let event = |value: serde_json::Value| serde_json::from_value::<JsonObject>(value).unwrap();
        vec![
            (
                "local://reddit",
                "reddit_accounts",
                "alice",
                vec![
                    event(serde_json::json!({"type": "reddit.account", "voice": "plain"})),
                    event(serde_json::json!({"type": "reddit.account", "voice": "warm"})),
                ],
            ),
            (
                "tenant://ops",
                "ops_projection",
                "workspace",
                vec![event(serde_json::json!({"event_type": "ops.snapshot"}))],
            ),
        ]
    }

    /// Each old event as a row: its address, version, type, body, digest,
    /// key, commit time and id, and its stream's chain after it.
    pub(super) struct Row {
        pub source: &'static str,
        pub resource: &'static str,
        pub aggregate: &'static str,
        pub version: u64,
        pub event_type: String,
        pub body: String,
        pub digest: String,
        pub key: String,
        pub committed_at: String,
        pub id: String,
        pub chain: String,
    }

    pub(super) fn rows() -> Vec<Row> {
        let mut rows = Vec::new();
        for (source, resource, aggregate, events) in streams() {
            let mut chain = model::empty_chain().unwrap();
            for (index, event) in events.into_iter().enumerate() {
                let version = index as u64 + 1;
                let digest = model::digest(&JsonValue::Object(event.clone())).unwrap();
                chain = model::chain(&chain, version, &digest).unwrap();
                rows.push(Row {
                    source,
                    resource,
                    aggregate,
                    version,
                    event_type: event
                        .get("type")
                        .or_else(|| event.get("event_type"))
                        .and_then(JsonValue::as_str)
                        .unwrap()
                        .to_owned(),
                    body: serde_json::to_string(&event).unwrap(),
                    digest,
                    key: format!("runx:sha256:{resource}-{version}"),
                    committed_at: format!("2026-10-0{version}T00:00:00.000Z"),
                    id: format!("{resource}:{aggregate}:{version}"),
                    chain: chain.clone(),
                });
            }
        }
        rows
    }

    /// alice's event digests and chain, which the migrated stream keeps.
    fn alice() -> (Vec<String>, String) {
        let rows = rows();
        let alice = rows
            .iter()
            .filter(|row| row.aggregate == "alice")
            .collect::<Vec<_>>();
        (
            alice.iter().map(|row| row.digest.clone()).collect(),
            alice.last().unwrap().chain.clone(),
        )
    }

    fn sql(prefix: &str, version: u8, integer: &str) -> String {
        let (source, id) = if version == 1 {
            ("data_source_ref", "event_ref")
        } else {
            ("data_source_uri", "event_id")
        };
        format!(
            "CREATE TABLE {prefix}events ({source} TEXT NOT NULL, resource TEXT NOT NULL, aggregate_id TEXT NOT NULL, version {integer} NOT NULL, idempotency_key TEXT NOT NULL, {id} TEXT NOT NULL, event_type TEXT NOT NULL, event_digest TEXT NOT NULL, event_json TEXT NOT NULL, committed_at TEXT NOT NULL, PRIMARY KEY ({source}, resource, aggregate_id, version));
             CREATE TABLE {prefix}heads ({source} TEXT NOT NULL, resource TEXT NOT NULL, aggregate_id TEXT NOT NULL, version {integer} NOT NULL, {id} TEXT NOT NULL, event_type TEXT NOT NULL, event_digest TEXT NOT NULL, idempotency_key TEXT NOT NULL, event_json TEXT NOT NULL, committed_at TEXT NOT NULL, projection_digest TEXT NOT NULL, PRIMARY KEY ({source}, resource, aggregate_id));"
        )
    }

    pub(super) fn write_sqlite(path: &std::path::Path, version: u8) -> (Vec<String>, String) {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(
                &sql("runx_", version, "INTEGER").replace("runx_heads", "runx_stream_heads"),
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", 0x5255_4e53_i64)
            .unwrap();
        connection
            .pragma_update(None, "user_version", i64::from(version))
            .unwrap();
        for row in rows() {
            connection
                .execute(
                    "INSERT INTO runx_events VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    rusqlite::params![
                        row.source,
                        row.resource,
                        row.aggregate,
                        row.version as i64,
                        row.key,
                        row.id,
                        row.event_type,
                        row.digest,
                        row.body,
                        row.committed_at
                    ],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT OR REPLACE INTO runx_stream_heads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        row.source, row.resource, row.aggregate, row.version as i64, row.id,
                        row.event_type, row.digest, row.key, row.body, row.committed_at, row.chain
                    ],
                )
                .unwrap();
        }
        alice()
    }

    /// Write the old streams to the Postgres or Redis store at `url`.
    #[cfg(feature = "store")]
    pub(super) fn write(url: &str, version: u8) -> (Vec<String>, String) {
        if url.starts_with("redis") {
            write_redis(url, version);
        } else {
            let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
            client
                .batch_execute(&format!(
                    "{} CREATE TABLE runx_store_schema (version INTEGER NOT NULL); INSERT INTO runx_store_schema VALUES ({version});",
                    sql("runx_store_", version, "BIGINT")
                ))
                .unwrap();
            for row in rows() {
                let version = row.version as i64;
                client
                    .execute(
                        "INSERT INTO runx_store_events VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                        &[&row.source, &row.resource, &row.aggregate, &version, &row.key, &row.id,
                          &row.event_type, &row.digest, &row.body, &row.committed_at],
                    )
                    .unwrap();
                client
                    .execute(
                        "DELETE FROM runx_store_heads WHERE resource = $1 AND aggregate_id = $2",
                        &[&row.resource, &row.aggregate],
                    )
                    .unwrap();
                client
                    .execute(
                        "INSERT INTO runx_store_heads VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                        &[&row.source, &row.resource, &row.aggregate, &version, &row.id,
                          &row.event_type, &row.digest, &row.key, &row.body, &row.committed_at, &row.chain],
                    )
                    .unwrap();
            }
        }
        alice()
    }

    /// Version 2's Redis layout: each resource's keys under one hash tag of
    /// its data source and resource, each stream's under a hash of all
    /// three, and no version key before version 2.
    #[cfg(feature = "store")]
    fn write_redis(url: &str, version: u8) {
        use redis::Commands;
        let mut connection = redis::Client::open(url).unwrap().get_connection().unwrap();
        fn hex(value: &impl serde::Serialize) -> String {
            runx_contracts::sha256_prefixed(&serde_json::to_vec(value).unwrap())
                .trim_start_matches("sha256:")
                .to_owned()
        }
        let id = if version == 1 {
            "event_ref"
        } else {
            "event_id"
        };
        for row in rows() {
            let prefix = format!("runx:store:{{{}}}", hex(&(row.source, row.resource)));
            let stream = hex(&(row.source, row.resource, row.aggregate));
            let record = serde_json::json!({
                id: row.id, "version": row.version, "event_type": row.event_type,
                "event": serde_json::from_str::<serde_json::Value>(&row.body).unwrap(),
                "event_digest": row.digest, "idempotency_key": row.key, "committed_at": row.committed_at,
            });
            let mut head = record.clone();
            head["aggregate_id"] = row.aggregate.into();
            let projection =
                serde_json::json!({"version": row.version, "projection_digest": row.chain});
            let _: () = connection
                .rpush(format!("{prefix}:events:{stream}"), record.to_string())
                .unwrap();
            let _: () = connection
                .hset(
                    format!("{prefix}:keys:{stream}"),
                    &row.key,
                    record.to_string(),
                )
                .unwrap();
            let _: () = connection
                .set(
                    format!("{prefix}:projection:{stream}"),
                    projection.to_string(),
                )
                .unwrap();
            let _: () = connection
                .hset(format!("{prefix}:heads"), row.aggregate, head.to_string())
                .unwrap();
        }
        if version == 2 {
            let _: () = connection.set("runx:store:schema", "2").unwrap();
        }
    }

    #[cfg(feature = "store")]
    pub(super) fn clear(url: &str) {
        if url.starts_with("redis") {
            let mut connection = redis::Client::open(url).unwrap().get_connection().unwrap();
            let _: () = redis::cmd("FLUSHDB").query(&mut connection).unwrap();
        } else {
            let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
            client
                .batch_execute(
                    "DROP TABLE IF EXISTS store_events, store_keys, runx_store_events, runx_store_heads, runx_store_schema",
                )
                .unwrap();
        }
    }
}
