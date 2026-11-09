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

/// A store an earlier runx wrote is from before 0.12: it refuses to open,
/// saying 0.12 doesn't read it and to move the file aside, and is left as it
/// was, its rows unread.
#[test]
fn a_sqlite_store_from_before_0_12_refuses_to_open() {
    for version in [1, 2] {
        let root = tempfile::tempdir().unwrap();
        let path =
            crate::paths::RunxPaths::resolve(&BTreeMap::new(), root.path()).workspace_store();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        old::write_sqlite(&path, version);
        let before = std::fs::read(&path).unwrap();
        let refused = Test::workspace(root.path())
            .call(
                "key.inspect",
                serde_json::json!({"stream": "accounts", "key": "alice"}),
            )
            .err()
            .unwrap();
        old::says_to_move_it_aside(&refused);
        assert!(refused.contains(&path.display().to_string()), "{refused}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "version {version} is left as it was"
        );
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

/// `private` keeps the person's own data in their runx home, so a home that
/// fell back to the workspace, which a repository can ship, is refused.
#[test]
fn private_refuses_a_home_that_fell_back_to_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let mut private = Test::private(root.path());
    private.env.remove("RUNX_HOME");
    let refused = private
        .call("events.append", append("k1", "a", 0, "one"))
        .err()
        .unwrap();
    assert!(refused.contains("HOME"), "{refused}");
    assert!(!root.path().join(".runx/private.sqlite").exists());
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

/// Postgres and Redis keep the same keys to the same digests as SQLite. They
/// run against `RUNX_TEST_POSTGRES_URL` and `RUNX_TEST_REDIS_URL`, each a
/// database the test may write to, when set.
#[cfg(feature = "store")]
#[test]
fn every_backend_keeps_the_same_streams() {
    let _databases = old::DATABASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        old::clear(&url);
    }
}

/// A Postgres or Redis store from before 0.12 refuses to open and is left as
/// it was, as SQLite's is. Each runs against `RUNX_TEST_POSTGRES_URL` or
/// `RUNX_TEST_REDIS_URL` when set.
#[cfg(feature = "store")]
#[test]
fn a_postgres_store_from_before_0_12_refuses_to_open() {
    let Ok(url) = std::env::var("RUNX_TEST_POSTGRES_URL") else {
        return;
    };
    let _databases = old::DATABASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for version in [1, 2] {
        old::clear(&url);
        old::write(&url, version);
        let isolated = tempfile::tempdir().unwrap();
        let refused = Test::at(isolated.path(), &url)
            .call(
                "key.inspect",
                serde_json::json!({"stream": "accounts", "key": "alice"}),
            )
            .err()
            .unwrap();
        old::says_to_move_it_aside(&refused);
        assert_eq!(
            old::rows_left(&url),
            1,
            "version {version} is left as it was"
        );
    }
    old::clear(&url);
}

#[cfg(feature = "store")]
#[test]
fn a_redis_store_from_before_0_12_refuses_to_open() {
    let Ok(url) = std::env::var("RUNX_TEST_REDIS_URL") else {
        return;
    };
    let _databases = old::DATABASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for version in [1, 2] {
        old::clear(&url);
        old::write(&url, version);
        let isolated = tempfile::tempdir().unwrap();
        let refused = Test::at(isolated.path(), &url)
            .call(
                "key.inspect",
                serde_json::json!({"stream": "accounts", "key": "alice"}),
            )
            .err()
            .unwrap();
        old::says_to_move_it_aside(&refused);
        assert_eq!(
            old::rows_left(&url),
            1,
            "version {version} is left as it was"
        );
    }
    old::clear(&url);
}

/// Stores as runx 0.11 and earlier wrote them, one event in each: streams
/// addressed by data source, resource and aggregate. Version 1 named
/// `data_source_uri` and `event_id` `data_source_ref` and `event_ref`, and
/// its Redis store set no schema version.
mod old {
    /// Held by each test that writes the Postgres and Redis databases, which
    /// they share.
    #[cfg(feature = "store")]
    pub(super) static DATABASES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(super) fn says_to_move_it_aside(refused: &str) {
        assert!(refused.contains("before runx 0.12"), "{refused}");
        assert!(refused.contains("doesn't read"), "{refused}");
        assert!(refused.contains("move"), "{refused}");
        assert!(refused.contains("new store"), "{refused}");
    }

    fn sql(prefix: &str, version: u8, integer: &str) -> String {
        let (source, id) = if version == 1 {
            ("data_source_ref", "event_ref")
        } else {
            ("data_source_uri", "event_id")
        };
        format!(
            "CREATE TABLE {prefix}events ({source} TEXT NOT NULL, resource TEXT NOT NULL, aggregate_id TEXT NOT NULL, version {integer} NOT NULL, event_json TEXT NOT NULL, {id} TEXT NOT NULL);
             CREATE TABLE {prefix}heads ({source} TEXT NOT NULL, resource TEXT NOT NULL, aggregate_id TEXT NOT NULL, projection_digest TEXT NOT NULL);
             INSERT INTO {prefix}events VALUES ('local://reddit', 'reddit_accounts', 'alice', 1, '{{}}', 'reddit_accounts:alice:1');"
        )
    }

    pub(super) fn write_sqlite(path: &std::path::Path, version: u8) {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(&sql("runx_", version, "INTEGER"))
            .unwrap();
        connection
            .pragma_update(None, "application_id", 0x5255_4e53_i64)
            .unwrap();
        connection
            .pragma_update(None, "user_version", i64::from(version))
            .unwrap();
    }

    /// Write an old store to the Postgres or Redis database at `url`.
    #[cfg(feature = "store")]
    pub(super) fn write(url: &str, version: u8) {
        if url.starts_with("redis") {
            use redis::Commands;
            let mut connection = redis::Client::open(url).unwrap().get_connection().unwrap();
            let _: () = connection
                .rpush("runx:store:{reddit}:events:alice", "{}")
                .unwrap();
            if version == 2 {
                let _: () = connection.set("runx:store:schema", "2").unwrap();
            }
        } else {
            let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
            client
                .batch_execute(&format!(
                    "{} CREATE TABLE runx_store_schema (version INTEGER NOT NULL); INSERT INTO runx_store_schema VALUES ({version});",
                    sql("runx_store_", version, "BIGINT")
                ))
                .unwrap();
        }
    }

    /// How many events the old store at `url` still holds.
    #[cfg(feature = "store")]
    pub(super) fn rows_left(url: &str) -> i64 {
        if url.starts_with("redis") {
            use redis::Commands;
            let mut connection = redis::Client::open(url).unwrap().get_connection().unwrap();
            connection.llen("runx:store:{reddit}:events:alice").unwrap()
        } else {
            let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
            client
                .query_one("SELECT count(*) FROM runx_store_events", &[])
                .unwrap()
                .get(0)
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
