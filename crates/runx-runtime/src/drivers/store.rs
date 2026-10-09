//! The store connector's native implementation: an event store with
//! append-only streams, an expected version and an idempotency key per
//! append, a rolling projection per stream, and stream heads by their latest
//! event.
//!
//! The instance's `url` parameter chooses where the streams live: SQLite
//! (`sqlite:<path>`), Postgres (`postgres://`) or Redis (`redis://`,
//! `rediss://`). Without one, the instance is the workspace store, a SQLite
//! file beside the kernel store. A password is the instance's credential,
//! never part of the URL. Every backend keeps the same streams with the same
//! digests, so a store moves between them without changing what a skill sees.
//!
//! An event is keyed by the idempotency key the kernel derives from the
//! call's intent, so the same call replays in any run. An append's answer is
//! authoritative in its own transaction, and the kernel still confirms it:
//! the readback reads what the store holds under the key, which must be the
//! event the append reported, or nothing after a conflict. A version conflict is the append's result, not a
//! refusal, so a caller can re-read and decide.

use std::path::{Path, PathBuf};

use runx_contracts::connector::{ReadbackExpected, ReadbackMatch};
use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};

use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver, Observed, Unperformed};

mod model;
#[cfg(feature = "store")]
mod postgres;
#[cfg(feature = "store")]
mod redis;
mod request;
mod sqlite;

use model::{Appended, EventRecord, Projection, StreamHead};
use request::{Append, Input, ListHeads, ReadEvents, Request, Source};

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

/// Where an instance's streams live, from its `url` parameter.
enum Location {
    Sqlite(PathBuf),
    #[cfg(feature = "store")]
    Postgres(String),
    #[cfg(feature = "store")]
    Redis(String),
}

impl Location {
    fn of(call: &AdmittedCall<'_>) -> Result<Self, StoreError> {
        // The admitted environment always names the workspace.
        let paths = crate::paths::RunxPaths::resolve(call.env, Path::new("."));
        let Some(url) = call.parameters.get("url") else {
            return Ok(Self::Sqlite(paths.workspace_store()));
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
        match parsed.scheme() {
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

/// What a backend keeps: streams of events with their projections and heads.
trait Backend {
    /// The backend's name in results: `sqlite`, `postgres` or `redis`.
    fn name(&self) -> &'static str;
    fn append(&mut self, request: &Append<'_>, committed_at: &str) -> Result<Appended, StoreError>;
    fn projection(&mut self, source: Source<'_>) -> Result<Projection, StoreError>;
    /// A page of events, ascending, and whether more follow.
    fn events(&mut self, request: &ReadEvents<'_>) -> Result<(Vec<EventRecord>, bool), StoreError>;
    /// A page of the newest heads, and whether more follow.
    fn heads(&mut self, request: &ListHeads<'_>) -> Result<(Vec<StreamHead>, bool), StoreError>;
    /// The digest of the event held under `idempotency_key`.
    fn landed(
        &mut self,
        source: Source<'_>,
        idempotency_key: &str,
    ) -> Result<Option<String>, StoreError>;
}

/// One transaction's view of a SQL store's streams, for the shared append.
trait Streams {
    fn projection(&mut self, source: Source<'_>) -> Result<Projection, StoreError>;
    fn existing(
        &mut self,
        source: Source<'_>,
        idempotency_key: &str,
    ) -> Result<Option<EventRecord>, StoreError>;
    /// Insert the event and move the stream's head and projection to it.
    fn commit_event(
        &mut self,
        source: Source<'_>,
        record: &EventRecord,
        projection: &Projection,
    ) -> Result<(), StoreError>;
}

/// The append, decided inside one transaction: a key already used replays or
/// conflicts, a stream at another version conflicts, else the event commits.
fn append_in(
    streams: &mut dyn Streams,
    request: &Append<'_>,
    committed_at: &str,
) -> Result<Appended, StoreError> {
    let current = streams.projection(request.source)?;
    let next = current
        .version
        .checked_add(1)
        .ok_or_else(|| StoreError::invalid("the stream version exceeds the supported range"))?;
    let proposed = model::record(
        request.source,
        next,
        request.idempotency_key,
        request.event,
        committed_at,
    )?;
    if let Some(existing) = streams.existing(request.source, request.idempotency_key)? {
        return Ok(if existing.event_digest == proposed.event_digest {
            Appended::Replay { current, existing }
        } else {
            Appended::KeyConflict {
                current,
                proposed,
                held: existing.event_digest,
            }
        });
    }
    if current.version != request.expected_version {
        return Ok(Appended::VersionConflict {
            current,
            proposed,
            expected: request.expected_version,
        });
    }
    let projection = model::advance(&current, &proposed)?;
    streams.commit_event(request.source, &proposed, &projection)?;
    Ok(Appended::Committed {
        before: current.version,
        record: proposed,
        projection,
    })
}

fn open(call: &AdmittedCall<'_>) -> Result<Box<dyn Backend>, StoreError> {
    let password = call
        .credential
        .and_then(|secret| secret.fields.get("password"))
        .map(String::as_str);
    match Location::of(call)? {
        Location::Sqlite(path) => {
            if password.is_some() {
                return Err(StoreError::invalid("a sqlite store takes no password"));
            }
            Ok(Box::new(sqlite::Sqlite::open(&path)?))
        }
        #[cfg(feature = "store")]
        Location::Postgres(url) => Ok(Box::new(postgres::Postgres::connect(&url, password)?)),
        #[cfg(feature = "store")]
        Location::Redis(url) => Ok(Box::new(redis::Redis::connect(&url, password)?)),
    }
}

/// The store, as the kernel drives it.
pub(crate) struct Store<'a> {
    pub call: &'a AdmittedCall<'a>,
}

impl Store<'_> {
    fn perform_call(&self) -> Result<JsonValue, StoreError> {
        let input = Input::decode(self.call.operation, self.call.arguments)?;
        let request = input.request(self.call.idempotency_key)?;
        let mut backend = open(self.call)?;
        let name = backend.name();
        match request {
            Request::Append(append) => {
                let committed_at = append
                    .observed_at
                    .map_or_else(crate::time::now_iso8601, str::to_owned);
                let appended = backend.append(&append, &committed_at)?;
                let mut result = model::appended_result(name, append.source, &appended)?;
                if let JsonValue::Object(object) = &mut result {
                    object.insert(
                        "landed".to_owned(),
                        model::text(appended.landed().unwrap_or(NOTHING_LANDED)),
                    );
                }
                Ok(result)
            }
            Request::ReadEvents(read) => {
                let projection = backend.projection(read.source)?;
                let (events, has_more) = backend.events(&read)?;
                model::events_result(
                    name,
                    read.source,
                    &projection,
                    &events,
                    read.limit,
                    read.after_version,
                    has_more,
                )
            }
            Request::ReadProjection(source) => {
                let projection = backend.projection(source)?;
                model::projection_result(name, source, &projection)
            }
            Request::ListHeads(list) => {
                let (heads, has_more) = backend.heads(&list)?;
                model::heads_result(
                    name,
                    list.data_source_ref,
                    list.resource,
                    list.limit,
                    &heads,
                    has_more,
                )
            }
        }
    }

    fn landed(&self) -> Result<JsonValue, StoreError> {
        let input = Input::decode(self.call.operation, self.call.arguments)?;
        let Request::Append(append) = input.request(self.call.idempotency_key)? else {
            return Err(StoreError::invalid("only an append is read back"));
        };
        let landed = open(self.call)?.landed(append.source, append.idempotency_key)?;
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

    fn check(&self, check: Check, _write: &JsonValue) -> Result<Observed, String> {
        match check {
            Check::Readback { .. } => self
                .landed()
                .map(observed)
                .map_err(|error| error.to_string()),
            Check::Precondition(_) => Err("a store call has no precondition".to_owned()),
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
        // The event the call landed on, or for a conflict the stream position
        // it was decided against: `<resource>:<aggregate>:<version>` either way.
        let object = result.as_object();
        let field = |name: &str| object.and_then(|object| object.get(name));
        let operation_id = match field("event_ref").and_then(JsonValue::as_str) {
            Some(event_ref) => Some(event_ref.to_owned()),
            None => match (
                field("resource").and_then(JsonValue::as_str),
                field("aggregate_id").and_then(JsonValue::as_str),
                field("after_version").and_then(model::unsigned),
            ) {
                (Some(resource), Some(aggregate), Some(version))
                    if self.call.operation == "events.append" =>
                {
                    Some(format!("{resource}:{aggregate}:{version}"))
                }
                _ => None,
            },
        };
        Ok((result, operation_id))
    }
}

#[cfg(test)]
mod tests;
