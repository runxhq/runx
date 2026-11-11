#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use runx_contracts::connector::OperationAccess;
use runx_contracts::{JsonObject, JsonValue};

use super::Store;
use crate::identity::ChainLink;
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver, Unconfirmed};

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
                    .map_err(Unconfirmed::reason)
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
/// data from other bytes is another event.
#[test]
fn an_events_digest_covers_its_writer() {
    use super::model::{EventRecord, Writer, event_digest};
    let data = JsonObject::from([("label".to_owned(), JsonValue::String("one".to_owned()))]);
    let event = |writer: Writer, event_type: &str| EventRecord {
        id: String::new(),
        version: 1,
        event_type: event_type.to_owned(),
        data: data.clone(),
        digest: String::new(),
        idempotency_key: "k".to_owned(),
        committed_at: "2026-10-07T00:00:00.000Z".to_owned(),
        writer,
        standing: false,
        valid_until: None,
        searchable: None,
    };
    let writer = |digest: &str| Writer {
        package: OWNER.to_owned(),
        digest: digest.to_owned(),
    };
    let one = event_digest(&event(
        writer(&format!("sha256:{}", "1".repeat(64))),
        "item.labelled",
    ))
    .unwrap();
    assert_ne!(
        one,
        event_digest(&event(
            writer(&format!("sha256:{}", "2".repeat(64))),
            "item.labelled"
        ))
        .unwrap()
    );
    assert_ne!(
        one,
        event_digest(&event(
            writer(&format!("sha256:{}", "1".repeat(64))),
            "item.renamed"
        ))
        .unwrap()
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

/// A key listing walks an index in the order it returns its keys: no scan,
/// and no sort, so the rows it visits are the rows it steps. A listing of
/// every key walks the recency index, a listing of the standing keys or of
/// the others walks the standing index, so neither visits the other's, and a
/// listing by a prefix walks a range of the keys table's primary key.
#[test]
fn a_key_listing_walks_its_index_with_no_scan_and_no_sort() {
    let root = tempfile::tempdir().unwrap();
    let private = Test::private(root.path());
    let kept = private
        .call("events.append", append("k1", "tea", 0, "tea black"))
        .unwrap();
    assert_eq!(kept.text("status"), "committed");
    let file = crate::paths::RunxPaths::resolve(&private.env, root.path()).private_store();
    let connection = rusqlite::Connection::open(file).unwrap();
    let walks = |sql: String, values: usize, index: &str, case: String| {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let values = vec![rusqlite::types::Value::Text("x".to_owned()); values];
        let plan = statement
            .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|step| step.contains(index))
                && plan
                    .iter()
                    .all(|step| !step.starts_with("SCAN") && !step.contains("TEMP B-TREE")),
            "{case}: {plan:?}"
        );
    };
    for (standing, index) in [
        (false, "store_keys_recent"),
        (true, "store_keys_standing_recent"),
    ] {
        for after_cursor in [false, true] {
            walks(
                super::sqlite::keys_query(standing, after_cursor),
                1 + usize::from(standing) + if after_cursor { 3 } else { 0 },
                index,
                format!("standing {standing}, after a cursor {after_cursor}"),
            );
        }
    }
    for after_cursor in [false, true] {
        walks(
            super::sqlite::prefix_query(after_cursor),
            3 + usize::from(after_cursor),
            "sqlite_autoindex_store_keys_1",
            format!("a prefix, after a cursor {after_cursor}"),
        );
    }
}

/// An append on `items` in `private` whose event is `standing` and valid
/// until `valid_until`, when each is given.
fn kept(
    idempotency: &str,
    key: &str,
    expected: u64,
    text: &str,
    standing: bool,
    valid_until: Option<&str>,
) -> serde_json::Value {
    let mut call = append(idempotency, key, expected, text);
    call["data"] = serde_json::json!({"text": text});
    call["searchable"] = serde_json::json!(["text"]);
    if standing {
        call["standing"] = true.into();
    }
    if let Some(until) = valid_until {
        call["valid_until"] = until.into();
    }
    call
}

/// Keep `text` under `key` on `items` in `private`'s file under `root`,
/// committed at second `second` of 2026, through the backend's own append,
/// so a test orders commits exactly rather than by the clock.
fn keep_at(
    root: &std::path::Path,
    key: &str,
    expected_version: u64,
    text: &str,
    standing: bool,
    valid_until: Option<&str>,
    second: u32,
) {
    use super::Backend as _;
    let env = Test::private(root).env;
    let path = crate::paths::RunxPaths::resolve(&env, root).private_store();
    let mut store = super::sqlite::Sqlite::open(&path, super::Kind::Private).unwrap();
    let data = JsonObject::from([("text".to_owned(), JsonValue::String(text.to_owned()))]);
    let idempotency_key = format!("{key}:{expected_version}");
    let append = super::request::Append {
        at: super::request::Key {
            stream: "project:board/items",
            key,
        },
        expected_version,
        idempotency_key: &idempotency_key,
        event_type: "item.labelled",
        data: &data,
        writer: super::model::Writer {
            package: OWNER.to_owned(),
            digest: format!("sha256:{}", "b".repeat(64)),
        },
        standing,
        valid_until,
        searchable: Some(vec!["text"]),
    };
    let appended = store
        .append(&append, &format!("2026-01-01T00:00:{second:02}.000Z"))
        .unwrap();
    assert!(matches!(appended, super::model::Appended::Committed { .. }));
}

/// A `private` event can be standing and can say the date it stops being
/// valid. Both are the event's own, beside its type: an event that names
/// either holds it, its digest covers it, and an event that names neither
/// reads as it always has.
#[test]
fn a_private_event_carries_its_standing_and_its_end_date() {
    let root = tempfile::tempdir().unwrap();
    let private = Test::private(root.path());
    let rule = private
        .call(
            "events.append",
            kept(
                "k1",
                "rule",
                0,
                "Sign off as Kam.",
                true,
                Some("2026-12-31"),
            ),
        )
        .unwrap();
    assert_eq!(rule.text("status"), "committed");
    assert!(rule.confirmed());
    let plain = private
        .call(
            "events.append",
            kept("k2", "plain", 0, "Sign off as Kam.", false, None),
        )
        .unwrap();
    let inspect = |key: &str| {
        private
            .call(
                "key.inspect",
                serde_json::json!({"stream": "items", "key": key}),
            )
            .unwrap()
            .field("last_event")
            .as_object()
            .unwrap()
            .clone()
    };
    let rule_event = inspect("rule");
    assert_eq!(rule_event["standing"], JsonValue::Bool(true));
    assert_eq!(
        rule_event["valid_until"],
        JsonValue::String("2026-12-31".to_owned())
    );
    let plain_event = inspect("plain");
    assert!(!plain_event.contains_key("standing"), "{plain_event:?}");
    assert!(!plain_event.contains_key("valid_until"), "{plain_event:?}");
    // The same data, type and writer: only what the event itself says
    // differs, and the digest covers it.
    assert_ne!(rule_event["digest"], plain_event["digest"]);
    assert_ne!(
        rule.field("event").as_object().unwrap()["digest"],
        plain.field("event").as_object().unwrap()["digest"]
    );
    // A history read returns them too.
    let read = private
        .call(
            "events.read",
            serde_json::json!({"stream": "items", "key": "rule"}),
        )
        .unwrap();
    let events = read.field("events").as_array().unwrap();
    assert_eq!(
        events[0].as_object().unwrap()["standing"],
        JsonValue::Bool(true)
    );
}

/// An event that names neither digests exactly as it did before either was
/// named, so every chain already kept still verifies.
#[test]
fn an_event_naming_neither_digests_as_it_always_has() {
    use super::model::{EventRecord, Writer, event_digest};
    let data = JsonObject::from([("label".to_owned(), JsonValue::String("one".to_owned()))]);
    let writer = Writer {
        package: OWNER.to_owned(),
        digest: format!("sha256:{}", "1".repeat(64)),
    };
    let event = EventRecord {
        id: String::new(),
        version: 1,
        event_type: "item.labelled".to_owned(),
        data: data.clone(),
        digest: String::new(),
        idempotency_key: "k".to_owned(),
        committed_at: "2026-10-07T00:00:00.000Z".to_owned(),
        writer: writer.clone(),
        standing: false,
        valid_until: None,
        searchable: None,
    };
    let before = super::model::digest(&JsonValue::Object(JsonObject::from([
        ("data".to_owned(), JsonValue::Object(data)),
        (
            "type".to_owned(),
            JsonValue::String("item.labelled".to_owned()),
        ),
        (
            "writer".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("package".to_owned(), JsonValue::String(writer.package)),
                ("digest".to_owned(), JsonValue::String(writer.digest)),
            ])),
        ),
    ])))
    .unwrap();
    assert_eq!(event_digest(&event).unwrap(), before);
    let standing = EventRecord {
        standing: true,
        ..event.clone()
    };
    let ending = EventRecord {
        valid_until: Some("2026-12-31".to_owned()),
        ..event.clone()
    };
    let searched = EventRecord {
        searchable: Some(vec!["label".to_owned()]),
        ..event
    };
    assert_ne!(event_digest(&standing).unwrap(), before);
    assert_ne!(event_digest(&ending).unwrap(), before);
    assert_ne!(event_digest(&searched).unwrap(), before);
    assert_ne!(
        event_digest(&standing).unwrap(),
        event_digest(&ending).unwrap()
    );
}

/// A listing that names `standing` reads only the standing keys, or only
/// the others, each newest commit first; one that doesn't reads every key.
/// A key is standing while its newest event is, so a later event that isn't
/// moves it to the others.
#[test]
fn a_listing_reads_the_standing_keys_apart_from_the_others() {
    let root = tempfile::tempdir().unwrap();
    keep_at(
        root.path(),
        "rule",
        0,
        "Always sign off as Kam.",
        true,
        None,
        1,
    );
    keep_at(root.path(), "tea", 0, "Tea, black.", false, None, 2);
    keep_at(
        root.path(),
        "habit",
        0,
        "Reply within a day.",
        true,
        None,
        3,
    );
    keep_at(
        root.path(),
        "coffee",
        0,
        "Coffee before noon.",
        false,
        None,
        4,
    );
    let private = Test::private(root.path());
    let list = |standing: Option<bool>| {
        let mut call = serde_json::json!({"stream": "items"});
        if let Some(standing) = standing {
            call["standing"] = standing.into();
        }
        private.call("keys.list", call).unwrap().keys("keys")
    };
    assert_eq!(list(Some(true)), vec!["habit", "rule"]);
    assert_eq!(list(Some(false)), vec!["coffee", "tea"]);
    assert_eq!(list(None), vec!["coffee", "habit", "tea", "rule"]);
    // The rule's next event isn't standing, so it's one of the others now.
    keep_at(
        root.path(),
        "rule",
        1,
        "Sign off as Kam, usually.",
        false,
        None,
        5,
    );
    assert_eq!(list(Some(true)), vec!["habit"]);
    assert_eq!(list(Some(false)), vec!["rule", "coffee", "tea"]);
    // Pages of one partition follow its own order.
    let first = private
        .call(
            "keys.list",
            serde_json::json!({"stream": "items", "standing": false, "limit": 2}),
        )
        .unwrap();
    assert_eq!(first.keys("keys"), vec!["rule", "coffee"]);
    let cursor = first.text("next_cursor").to_owned();
    let second = private
        .call(
            "keys.list",
            serde_json::json!({"stream": "items", "standing": false, "limit": 2, "cursor": cursor}),
        )
        .unwrap();
    assert_eq!(second.keys("keys"), vec!["tea"]);
    assert_eq!(second.field("next_cursor"), &JsonValue::Null);
}

/// A listing or a search valid at a date passes over a key whose newest
/// event stopped being valid before it: one that ends on that date is still
/// valid on it. One that names no date reads every key.
#[test]
fn a_listing_and_a_search_valid_at_a_date_pass_over_what_ended_before_it() {
    let root = tempfile::tempdir().unwrap();
    for (second, key, text, standing, until) in [
        (
            1,
            "ended",
            "The tea shop opens late.",
            false,
            Some("2026-05-31"),
        ),
        (
            2,
            "ends",
            "The tea shop opens early.",
            false,
            Some("2026-06-01"),
        ),
        (
            3,
            "later",
            "The tea shop opens at nine.",
            false,
            Some("2027-01-01"),
        ),
        (4, "always", "The tea shop is on the corner.", false, None),
        (5, "rule", "Buy tea on Mondays.", true, Some("2026-01-01")),
    ] {
        keep_at(root.path(), key, 0, text, standing, until, second);
    }
    let private = Test::private(root.path());
    let list = |extra: serde_json::Value| {
        let mut call = serde_json::json!({"stream": "items"});
        call.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        private.call("keys.list", call).unwrap().keys("keys")
    };
    assert_eq!(
        list(serde_json::json!({"valid_at": "2026-06-01"})),
        vec!["always", "later", "ends"]
    );
    assert_eq!(
        list(serde_json::json!({"valid_at": "2026-06-01", "standing": true})),
        Vec::<String>::new()
    );
    assert_eq!(
        list(serde_json::json!({})),
        vec!["rule", "always", "later", "ends", "ended"]
    );
    let search = |extra: serde_json::Value| {
        let mut call = serde_json::json!({"stream": "items", "query": "tea"});
        call.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut found = private.call("keys.search", call).unwrap().keys("matches");
        found.sort();
        found
    };
    assert_eq!(
        search(serde_json::json!({"valid_at": "2026-06-01", "standing": false})),
        vec!["always", "ends", "later"]
    );
    assert_eq!(search(serde_json::json!({"standing": true})), vec!["rule"]);
    assert_eq!(
        search(serde_json::json!({})),
        vec!["always", "ended", "ends", "later", "rule"],
        "a search that names no date reads what ended too"
    );
    // A date is a calendar date.
    assert!(
        private
            .call(
                "keys.list",
                serde_json::json!({"stream": "items", "valid_at": "2026-02-30"})
            )
            .is_err()
    );
    assert!(
        private
            .call(
                "events.append",
                kept("k6", "bad", 0, "Never kept.", false, Some("next week"))
            )
            .is_err()
    );
}

/// A listing that names a prefix reads only the keys it begins, in key
/// order, page by page, and keeps the standing, types and end date the call
/// names as any listing does. A prefix is 8 to 63 lowercase hex characters,
/// so it names a short handle's key and never reads a stream whole.
#[test]
fn a_prefix_lists_only_the_keys_it_begins_in_key_order() {
    let root = tempfile::tempdir().unwrap();
    for (second, key, standing) in [
        (1, "0aaaaaaa10", false),
        (2, "0aaaaaaa0f", true),
        (3, "0aaaaaab00", false),
        (4, "1aaaaaaa00", false),
    ] {
        keep_at(root.path(), key, 0, "Tea, black.", standing, None, second);
    }
    let private = Test::private(root.path());
    let list = |extra: serde_json::Value| {
        let mut call = serde_json::json!({"stream": "items"});
        call.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        private.call("keys.list", call)
    };
    let keys = |extra: serde_json::Value| list(extra).unwrap().keys("keys");
    assert_eq!(
        keys(serde_json::json!({"prefix": "0aaaaaaa"})),
        vec!["0aaaaaaa0f", "0aaaaaaa10"]
    );
    assert_eq!(
        keys(serde_json::json!({"prefix": "0aaaaaab"})),
        vec!["0aaaaaab00"]
    );
    assert!(keys(serde_json::json!({"prefix": "0aaaaaac"})).is_empty());
    assert_eq!(
        keys(serde_json::json!({"prefix": "0aaaaaaa", "standing": false})),
        vec!["0aaaaaaa10"]
    );
    let first = list(serde_json::json!({"prefix": "0aaaaaaa", "limit": 1})).unwrap();
    assert_eq!(first.keys("keys"), vec!["0aaaaaaa0f"]);
    let cursor = first.text("next_cursor").to_owned();
    let second =
        list(serde_json::json!({"prefix": "0aaaaaaa", "limit": 1, "cursor": cursor})).unwrap();
    assert_eq!(second.keys("keys"), vec!["0aaaaaaa10"]);
    assert_eq!(second.field("next_cursor"), &JsonValue::Null);
    for prefix in [
        "0aaaaaa".to_owned(),
        "0".repeat(64),
        "0AAAAAAA".to_owned(),
        "0aaaaaag".to_owned(),
    ] {
        let refused = list(serde_json::json!({"prefix": prefix})).err().unwrap();
        assert!(refused.contains("prefix"), "{prefix}: {refused}");
    }
}

/// Standing keys, end dates and what a search matches are `private`'s:
/// `store` refuses an append or a listing that names them, and keeps
/// nothing.
#[test]
fn store_refuses_standing_end_dates_and_searchable_fields() {
    let root = tempfile::tempdir().unwrap();
    let store = Test::workspace(root.path());
    let mut searchable = append("k3", "plain", 0, "Tea, black.");
    searchable["searchable"] = serde_json::json!(["label"]);
    for call in [
        kept("k1", "rule", 0, "Sign off as Kam.", true, None),
        kept("k2", "tea", 0, "Tea, black.", false, Some("2026-12-31")),
        searchable,
    ] {
        let refused = store.call("events.append", call).err().unwrap();
        assert!(refused.contains("private"), "{refused}");
    }
    for extra in [
        serde_json::json!({"standing": true}),
        serde_json::json!({"valid_at": "2026-06-01"}),
        serde_json::json!({"prefix": "0aaaaaaa"}),
    ] {
        let mut call = serde_json::json!({"stream": "items"});
        call.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let refused = store.call("keys.list", call).err().unwrap();
        assert!(refused.contains("private"), "{refused}");
    }
    assert!(
        store
            .call("keys.list", serde_json::json!({"stream": "items"}))
            .unwrap()
            .keys("keys")
            .is_empty()
    );
}

/// A search ranks by score, and among equal scores the newest commit first,
/// then by key, page by page.
#[test]
fn a_searchs_equal_scores_go_newest_first() {
    let root = tempfile::tempdir().unwrap();
    // The same words, so the same score, kept one after another, and two
    // kept at the same moment, which go by key.
    for (second, key) in [(1, "b"), (2, "a"), (3, "d"), (3, "c")] {
        keep_at(root.path(), key, 0, "tea black", false, None, second);
    }
    let private = Test::private(root.path());
    let all = private
        .call(
            "keys.search",
            serde_json::json!({"stream": "items", "query": "tea"}),
        )
        .unwrap();
    assert_eq!(all.keys("matches"), vec!["c", "d", "a", "b"]);
    let mut paged = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut call = serde_json::json!({"stream": "items", "query": "tea", "limit": 1});
        if let Some(cursor) = &cursor {
            call["cursor"] = cursor.clone().into();
        }
        let page = private.call("keys.search", call).unwrap();
        paged.extend(page.keys("matches"));
        match page.field("next_cursor") {
            JsonValue::String(next) => cursor = Some(next.clone()),
            _ => break,
        }
    }
    assert_eq!(paged, vec!["c", "d", "a", "b"]);
}

/// `keys.search` ranks one stream's keys by their newest event, quotes its
/// query, pages in score order, filters by type, and is `private`'s alone.
#[test]
fn private_searches_one_streams_current_entries() {
    let root = tempfile::tempdir().unwrap();
    let private = Test::private(root.path());
    let keep = |idempotency: &str, key: &str, expected: u64, text: &str| {
        let mut call = append(idempotency, key, expected, text);
        call["data"] = serde_json::json!({"text": text});
        call["searchable"] = serde_json::json!(["text"]);
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

/// A search matches a key only by the strings under the `data` fields its
/// newest event names `searchable`: never another field's words, and nothing
/// for an event that names none. The writer says what's searchable, and the
/// store doesn't guess.
#[test]
fn a_search_matches_only_the_fields_its_event_names_searchable() {
    let root = tempfile::tempdir().unwrap();
    let private = Test::private(root.path());
    let mut named = append("k1", "tea", 0, "unused");
    named["data"] = serde_json::json!({
        "text": "I take my tea black.",
        "summary": "Tea",
        "confirmation": {"gate_id": "memory.remember", "status": "approved"},
        "sources": ["conversation"],
    });
    named["searchable"] = serde_json::json!(["text", "summary"]);
    private.call("events.append", named).unwrap();
    let mut unnamed = append("k2", "coffee", 0, "unused");
    unnamed["data"] = serde_json::json!({"text": "Coffee before noon, approved."});
    private.call("events.append", unnamed).unwrap();
    let search = |query: &str| {
        private
            .call(
                "keys.search",
                serde_json::json!({"stream": "items", "query": query}),
            )
            .unwrap()
            .keys("matches")
    };
    assert_eq!(search("black"), vec!["tea"]);
    assert_eq!(search("tea"), vec!["tea"]);
    for unnamed_words in ["approved", "remember", "conversation", "coffee"] {
        assert!(search(unnamed_words).is_empty(), "{unnamed_words}");
    }
    // The event holds what it named, and its next event replaces the row.
    let last = private
        .call(
            "key.inspect",
            serde_json::json!({"stream": "items", "key": "tea"}),
        )
        .unwrap()
        .field("last_event")
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(
        last["searchable"],
        serde_json::from_value::<JsonValue>(serde_json::json!(["text", "summary"])).unwrap()
    );
    let mut next = append("k3", "tea", 1, "unused");
    next["data"] = serde_json::json!({"text": "I take my tea with milk.", "note": "black"});
    next["searchable"] = serde_json::json!(["text"]);
    private.call("events.append", next).unwrap();
    assert!(search("black").is_empty());
    assert_eq!(search("milk"), vec!["tea"]);
}

/// `searchable` names 1 to 16 distinct fields, each one the event's data
/// holds, so a typo can't leave a key silently unsearchable.
#[test]
fn searchable_names_distinct_fields_of_the_data() {
    let root = tempfile::tempdir().unwrap();
    let private = Test::private(root.path());
    let with = |fields: serde_json::Value| {
        let mut call = append("k1", "tea", 0, "unused");
        call["data"] = serde_json::json!({"text": "Tea, black.", "summary": "Tea"});
        call["searchable"] = fields;
        private.call("events.append", call)
    };
    for refused in [
        serde_json::json!(["txt"]),
        serde_json::json!(["text", "text"]),
        serde_json::json!([]),
        serde_json::json!((0..17).map(|n| format!("f{n}")).collect::<Vec<_>>()),
    ] {
        assert!(with(refused.clone()).is_err(), "{refused}");
    }
    assert_eq!(
        with(serde_json::json!(["text", "summary"]))
            .unwrap()
            .text("status"),
        "committed"
    );
}

#[test]
fn a_sqlite_store_syncs_its_commits_and_checkpoints_with_the_full_flush() {
    let root = tempfile::tempdir().unwrap();
    for kind in [super::Kind::Store, super::Kind::Private] {
        let store =
            super::sqlite::Sqlite::open(&root.path().join(format!("{kind:?}.sqlite")), kind)
                .unwrap();
        for pragma in ["fullfsync", "checkpoint_fullfsync"] {
            assert_eq!(store.pragma(pragma), 1, "{kind:?} {pragma}");
        }
    }
}
