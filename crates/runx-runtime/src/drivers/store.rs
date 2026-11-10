//! The native implementation of the runtime's own `store` and `private`
//! connectors (`store.md`): streams of keys, each key's events appended at
//! the version its caller expects under an idempotency key, read in version
//! order, inspected, and listed newest first; on `private`, a stream's
//! current entries searched.
//!
//! On `private`, an event can also be standing, and can state the last date
//! it's valid. A listing or a search reads a stream's standing keys apart
//! from the others, by an index of their own, and one valid at a date passes
//! over a key whose newest event ended before it. They're `private`'s, so
//! `store` refuses them.
//!
//! A stream belongs to a package. The author writes its own name, and the
//! runtime places it under the package the call is made from, rendering the
//! call's target as `<package>/<stream>` (§4.8). So a package reads and
//! writes its own streams with no naming, a read of another's names it
//! under a term, and a write can't name one. Every event records its writer:
//! the package and the digest of the bytes it ran from, set by the runtime.
//!
//! The instance's `url` parameter chooses where the streams live: SQLite
//! (`sqlite:<path>`), Postgres (`postgres://`) or Redis (`redis://`,
//! `rediss://`). Without one, `store` is the workspace store, a SQLite file
//! beside the kernel store, and `private` its own file in the runx home. A
//! password is the instance's credential, never part of the URL. Every
//! backend keeps the same streams with the same digests, so a store moves
//! between them without changing what a skill sees, wherever an instance is
//! accepted: a `private` instance is accepted only where it can search.
//!
//! An event is keyed by the idempotency key the kernel derives from the
//! call's intent, so the same call replays in any run. An append's answer is
//! authoritative in its own transaction, and the kernel still confirms it:
//! the readback reads what the store holds under the key, which must be the
//! event the append reported, or nothing after a conflict. A conflict is the
//! append's answer, not a refusal, so a caller can read again and decide.

use std::path::{Path, PathBuf};

use runx_contracts::connector::{ReadbackExpected, ReadbackMatch};
use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};

use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver, Observed, Unconfirmed, Unperformed};

mod model;
#[cfg(feature = "store")]
mod postgres;
#[cfg(feature = "store")]
mod redis;
mod request;
mod sqlite;

use model::{Appended, EventRecord, KeyState, Listed, Matched, Writer};
use request::{Append, Input, Key, ListKeys, ReadEvents, Request, SearchKeys};

/// What confirms a store append: the store holds the reported event under
/// the call's key, or nothing after a conflict.
pub(crate) fn readback_matches() -> Vec<ReadbackMatch> {
    vec![ReadbackMatch {
        expected: ReadbackExpected::Acknowledgement {
            path: "landed".to_owned().into(),
        },
        observed: "landed".to_owned().into(),
    }]
}

/// Nothing is held under the key.
const NOTHING_LANDED: &str = "none";

/// The runtime's connector for the person's own data: the store's driver
/// and tables in its own file in the runx home, every call under a term.
pub(crate) const PRIVATE: &str = "private";

#[derive(Debug)]
pub(crate) struct StoreError {
    kind: StoreErrorKind,
    message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoreErrorKind {
    /// The call is malformed; nothing was sent.
    Invalid,
    /// The store could not be reached or read; nothing was applied.
    Unavailable,
    /// The store holds something it could not have written.
    Corrupt,
    /// A write may have been applied: a networked store did not confirm the
    /// commit.
    #[cfg(feature = "store")]
    Uncertain,
}

impl StoreError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Invalid, message)
    }

    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Unavailable, message)
    }

    pub(crate) fn corrupt(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Corrupt, message)
    }

    #[cfg(feature = "store")]
    pub(crate) fn uncertain(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Uncertain, message)
    }

    fn new(kind: StoreErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn unperformed(self) -> Unperformed {
        let Self { kind, message } = self;
        match kind {
            StoreErrorKind::Invalid => Unperformed::NotSent {
                code: "invalid_request",
                reason: message,
            },
            StoreErrorKind::Unavailable => Unperformed::NotSent {
                code: "store_unavailable",
                reason: message,
            },
            StoreErrorKind::Corrupt => Unperformed::NotSent {
                code: "store_corrupt",
                reason: message,
            },
            #[cfg(feature = "store")]
            StoreErrorKind::Uncertain => Unperformed::MaybeSent {
                code: "store_uncertain",
                reason: message,
            },
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Which of the runtime's two connectors an instance serves: they share the
/// driver and the tables, and each keeps its own file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A package's ordinary state, in the workspace.
    Store,
    /// The person's own data, in the runx home, searchable.
    Private,
}

impl Kind {
    fn of(connector: &str) -> Self {
        if connector == PRIVATE {
            Self::Private
        } else {
            Self::Store
        }
    }
}

/// Where an instance's streams live, from its `url` parameter.
enum Location {
    Sqlite(PathBuf),
    #[cfg(feature = "store")]
    Postgres(String),
    #[cfg(feature = "store")]
    Redis(String),
}

impl Location {
    fn of(
        kind: Kind,
        parameters: &JsonObject,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self, StoreError> {
        // The admitted environment always names the workspace.
        let paths = crate::paths::RunxPaths::resolve(env, Path::new("."));
        let Some(url) = parameters.get("url") else {
            return match kind {
                Kind::Store => Ok(Self::Sqlite(paths.workspace_store())),
                // A home that fell back to the workspace counts for nothing
                // (D30): a repository could ship it.
                Kind::Private if paths.home_fell_back() => Err(StoreError::invalid(
                    "private keeps the person's own data in their runx home, and with HOME and USERPROFILE unset the home fell back to the workspace, which a repository can ship; set HOME, or RUNX_HOME",
                )),
                Kind::Private => Ok(Self::Sqlite(paths.private_store())),
            };
        };
        let url = url
            .as_str()
            .ok_or_else(|| StoreError::invalid("the store url is not text"))?;
        if let Some(path) = url.strip_prefix("sqlite:") {
            if path.is_empty() {
                return Err(StoreError::invalid("a sqlite store url names a file"));
            }
            return Ok(Self::Sqlite(paths.in_workspace(path)));
        }
        let parsed = url::Url::parse(url)
            .map_err(|error| StoreError::invalid(format!("the store url: {error}")))?;
        if parsed.password().is_some() {
            return Err(StoreError::invalid(
                "the store url carries a password; bind it as the instance's credential",
            ));
        }
        let scheme = parsed.scheme();
        if kind == Kind::Private && scheme != "sqlite" {
            return Err(cannot_search(scheme));
        }
        match scheme {
            #[cfg(feature = "store")]
            "postgres" | "postgresql" => Ok(Self::Postgres(url.to_owned())),
            #[cfg(feature = "store")]
            "redis" | "rediss" => Ok(Self::Redis(url.to_owned())),
            scheme => Err(StoreError::invalid(format!(
                "this runx has no {scheme} store backend"
            ))),
        }
    }
}

/// Why a `private` instance on `scheme` is refused.
fn cannot_search(scheme: &str) -> StoreError {
    StoreError::invalid(format!(
        "a private instance serves every operation of private, keys.search among them, and a {scheme} store can't search; name a sqlite: url, or none for its own file in the runx home"
    ))
}

/// Whether an instance of `connector` with `parameters` serves every
/// operation its connector declares, checked when the instance is
/// configured, never on a call: a `private` instance on a backend that
/// can't search is refused.
///
/// # Errors
///
/// Returns why the instance can't be configured.
pub(crate) fn configurable(connector: &str, parameters: &JsonObject) -> Result<(), String> {
    let kind = Kind::of(connector);
    if kind == Kind::Store {
        return Ok(());
    }
    let Some(url) = parameters.get("url").and_then(JsonValue::as_str) else {
        return Ok(());
    };
    if url.starts_with("sqlite:") {
        return Ok(());
    }
    let scheme = url.split_once(':').map_or(url, |(scheme, _)| scheme);
    Err(cannot_search(scheme).to_string())
}

/// What a backend keeps: streams of keys, each with its events and state.
trait Backend {
    /// The backend's name in answers: `sqlite`, `postgres` or `redis`.
    fn name(&self) -> &'static str;
    fn append(&mut self, request: &Append<'_>, committed_at: &str) -> Result<Appended, StoreError>;
    fn state(&mut self, at: Key<'_>) -> Result<KeyState, StoreError>;
    /// A page of a key's events, in version order, and whether more follow.
    fn events(&mut self, request: &ReadEvents<'_>) -> Result<(Vec<EventRecord>, bool), StoreError>;
    /// A page of a stream's keys, newest commit first, and whether more
    /// follow.
    fn keys(&mut self, request: &ListKeys<'_>) -> Result<(Vec<Listed>, bool), StoreError>;
    /// A page of a stream's keys ranked against the query, and whether more
    /// follow. Only a backend `private` accepts implements it.
    fn search(&mut self, _request: &SearchKeys<'_>) -> Result<(Vec<Matched>, bool), StoreError> {
        Err(cannot_search(self.name()))
    }
    /// The digest of the event held under `idempotency_key`.
    fn landed(&mut self, at: Key<'_>, idempotency_key: &str) -> Result<Option<String>, StoreError>;
}

/// One transaction's view of a SQL store's keys, for the shared append.
trait Streams {
    fn state(&mut self, at: Key<'_>) -> Result<KeyState, StoreError>;
    fn existing(
        &mut self,
        at: Key<'_>,
        idempotency_key: &str,
    ) -> Result<Option<EventRecord>, StoreError>;
    /// Insert the event and move its key's state to it.
    fn commit_event(
        &mut self,
        at: Key<'_>,
        record: &EventRecord,
        state: &KeyState,
    ) -> Result<(), StoreError>;
}

/// The append, decided inside one transaction: an idempotency key already
/// used replays or conflicts, which is checked first, a key at another
/// version conflicts, else the event commits.
fn append_in(
    streams: &mut dyn Streams,
    request: &Append<'_>,
    committed_at: &str,
) -> Result<Appended, StoreError> {
    let current = streams.state(request.at)?;
    let next = current
        .version
        .checked_add(1)
        .ok_or_else(|| StoreError::invalid("the key's version exceeds the supported range"))?;
    let proposed = proposed(request, next, committed_at)?;
    if let Some(existing) = streams.existing(request.at, request.idempotency_key)? {
        return Ok(if existing.digest == proposed.digest {
            Appended::Replayed { current, existing }
        } else {
            Appended::IdempotencyConflict {
                current,
                held: existing.digest,
            }
        });
    }
    if current.version != request.expected_version {
        return Ok(Appended::VersionConflict { current });
    }
    let state = model::advance(&current, &proposed)?;
    streams.commit_event(request.at, &proposed, &state)?;
    Ok(Appended::Committed {
        before: current.version,
        record: proposed,
        state,
    })
}

/// The event `request` proposes at `version`.
fn proposed(
    request: &Append<'_>,
    version: u64,
    committed_at: &str,
) -> Result<EventRecord, StoreError> {
    model::record(request, version, committed_at)
}

fn open(call: &AdmittedCall<'_>) -> Result<Box<dyn Backend>, StoreError> {
    let kind = Kind::of(call.connector);
    let password = call
        .credential
        .and_then(|secret| secret.field("password").ok());
    match Location::of(kind, call.parameters, call.env)? {
        Location::Sqlite(path) => {
            if password.is_some() {
                return Err(StoreError::invalid("a sqlite store takes no password"));
            }
            Ok(Box::new(sqlite::Sqlite::open(&path, kind)?))
        }
        #[cfg(feature = "store")]
        Location::Postgres(url) => Ok(Box::new(postgres::Postgres::connect(&url, password)?)),
        #[cfg(feature = "store")]
        Location::Redis(url) => Ok(Box::new(redis::Redis::connect(&url, password)?)),
    }
}

/// One event of a history a probe lays down: on a stream as it's stored,
/// under its package, written by `writer_package`.
#[cfg(feature = "probe-fixtures")]
pub struct HistoryEvent {
    pub stream: String,
    pub key: String,
    pub event_type: String,
    pub data: JsonObject,
    pub standing: bool,
    pub valid_until: Option<String>,
    pub committed_at: String,
    pub writer_package: String,
}

/// Lay down `history` in the `private` store `env` names, in one
/// transaction, each event at the next version of its key, exactly as an
/// append writes it: how a probe gives memory a long record without a call per
/// event.
#[cfg(feature = "probe-fixtures")]
pub(crate) fn lay_down_private_history(
    env: &std::collections::BTreeMap<String, String>,
    history: &[HistoryEvent],
) -> Result<(), StoreError> {
    let location = Location::of(Kind::Private, &JsonObject::new(), env)?;
    // Without the `store` feature, SQLite is the only place a store can be.
    #[cfg(feature = "store")]
    let Location::Sqlite(path) = location else {
        return Err(StoreError::invalid(
            "a history is laid down in a sqlite store",
        ));
    };
    #[cfg(not(feature = "store"))]
    let Location::Sqlite(path) = location;
    let mut store = sqlite::Sqlite::open(&path, Kind::Private)?;
    let mut versions = std::collections::BTreeMap::<(&str, &str), u64>::new();
    let idempotency = history
        .iter()
        .enumerate()
        .map(|(index, event)| format!("history:{index}:{}", event.key))
        .collect::<Vec<_>>();
    let appends = history
        .iter()
        .zip(&idempotency)
        .map(|(event, idempotency_key)| {
            let version = versions
                .entry((event.stream.as_str(), event.key.as_str()))
                .or_insert(0);
            let expected_version = *version;
            *version += 1;
            (
                Append {
                    at: Key {
                        stream: &event.stream,
                        key: &event.key,
                    },
                    expected_version,
                    idempotency_key,
                    event_type: &event.event_type,
                    data: &event.data,
                    writer: Writer {
                        package: event.writer_package.clone(),
                        digest: sha256_prefixed(event.writer_package.as_bytes()),
                    },
                    standing: event.standing,
                    valid_until: event.valid_until.as_deref(),
                },
                event.committed_at.clone(),
            )
        })
        .collect::<Vec<_>>();
    store.append_history(&appends)
}

/// The store, as the kernel drives it.
pub(crate) struct Store<'a> {
    pub call: &'a AdmittedCall<'a>,
}

impl Store<'_> {
    fn writer(&self) -> Option<Writer> {
        self.call.writer.map(|link| Writer {
            package: link.package.as_str().to_owned(),
            digest: link.digest.clone(),
        })
    }

    fn perform_call(&self) -> Result<JsonValue, StoreError> {
        let input = Input::decode(self.call.operation, self.call.arguments)?;
        let writer = self.writer();
        let request =
            input.request(self.call.target, self.call.idempotency_key, writer.as_ref())?;
        if Kind::of(self.call.connector) != Kind::Private && request.is_private() {
            return Err(StoreError::invalid(
                "standing, valid_until and valid_at are private's: a store event is neither standing nor dated",
            ));
        }
        let mut backend = open(self.call)?;
        let name = backend.name();
        match request {
            Request::Append(append) => {
                let committed_at = crate::time::now_iso8601();
                let appended = backend.append(&append, &committed_at)?;
                let mut result = model::appended_result(name, append.at, &appended)?;
                if let JsonValue::Object(object) = &mut result {
                    object.insert(
                        "landed".to_owned(),
                        model::text(appended.landed().unwrap_or(NOTHING_LANDED)),
                    );
                }
                Ok(result)
            }
            Request::Read(read) => {
                let state = backend.state(read.at)?;
                let (events, has_more) = backend.events(&read)?;
                model::events_result(name, read.at, &state, &events, read.after_version, has_more)
            }
            Request::Inspect(at) => {
                let state = backend.state(at)?;
                model::inspected_result(name, at, &state)
            }
            Request::List(list) => {
                let (keys, has_more) = backend.keys(&list)?;
                model::listed_result(name, list.stream, &keys, has_more)
            }
            Request::Search(search) => {
                if Kind::of(self.call.connector) != Kind::Private {
                    return Err(StoreError::invalid(
                        "keys.search searches private's streams only",
                    ));
                }
                let (matches, has_more) = backend.search(&search)?;
                model::matched_result(name, search.stream, &matches, has_more)
            }
        }
    }

    fn landed(&self) -> Result<JsonValue, StoreError> {
        let input = Input::decode(self.call.operation, self.call.arguments)?;
        let writer = self.writer();
        let Request::Append(append) =
            input.request(self.call.target, self.call.idempotency_key, writer.as_ref())?
        else {
            return Err(StoreError::invalid("only an append is read back"));
        };
        let landed = open(self.call)?.landed(append.at, append.idempotency_key)?;
        Ok(JsonValue::Object(JsonObject::from([(
            "landed".to_owned(),
            model::text(landed.as_deref().unwrap_or(NOTHING_LANDED)),
        )])))
    }
}

fn observed(value: JsonValue) -> Observed {
    let evidence = serde_json::to_vec(&value)
        .map(|bytes| sha256_prefixed(&bytes))
        .unwrap_or_default();
    Observed {
        value,
        evidence: vec![evidence],
    }
}

impl Driver for Store<'_> {
    fn perform(&self, _item: Option<usize>) -> Result<Observed, Unperformed> {
        self.perform_call()
            .map(observed)
            .map_err(StoreError::unperformed)
    }

    fn check(&self, check: Check, _write: &JsonValue) -> Result<Observed, Unconfirmed> {
        match check {
            Check::Readback { .. } => self
                .landed()
                .map(observed)
                .map_err(|error| error.to_string().into()),
            Check::Precondition(_) => Err("a store call has no precondition".into()),
        }
    }

    fn result(
        &self,
        write: &JsonValue,
        _confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String> {
        let mut result = write.clone();
        if let JsonValue::Object(object) = &mut result {
            object.remove("landed");
        }
        // The event the call landed on, or for a conflict the key's position
        // it was decided against, in the event id's encoding either way.
        let object = result.as_object();
        let field = |name: &str| object.and_then(|object| object.get(name));
        let operation_id = match field("event")
            .and_then(JsonValue::as_object)
            .and_then(|event| event.get("id"))
            .and_then(JsonValue::as_str)
        {
            Some(id) => Some(id.to_owned()),
            None => match (
                field("stream").and_then(JsonValue::as_str),
                field("key").and_then(JsonValue::as_str),
                field("version").and_then(model::unsigned),
            ) {
                (Some(stream), Some(key), Some(version))
                    if self.call.operation == "events.append" =>
                {
                    Some(model::event_id(stream, key, version))
                }
                _ => None,
            },
        };
        Ok((result, operation_id))
    }
}

#[cfg(test)]
mod tests;
