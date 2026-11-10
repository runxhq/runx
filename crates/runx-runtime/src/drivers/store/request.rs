//! A store call's arguments, decoded and checked once for every backend.
//!
//! The author writes a stream's own name, `accounts`; the stream a call
//! reaches is the target admission rendered for it, `runx/reddit/accounts`,
//! under the package the call is made from or, for a read, the package it
//! names (§4.8). The driver reads only that target, so a call never chooses
//! another package's stream itself.

use std::collections::BTreeSet;

use runx_contracts::{JsonObject, JsonValue, MAX_PORTABLE_INTEGER};
use serde::Deserialize;

use super::StoreError;
use super::model::Writer;

/// One key of one stream, the stream full: `<owner>/<stream>`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Key<'a> {
    pub stream: &'a str,
    pub key: &'a str,
}

/// One store operation, by the operation name the connector declares.
pub(super) enum Request<'a> {
    Append(Append<'a>),
    Read(ReadEvents<'a>),
    Inspect(Key<'a>),
    List(ListKeys<'a>),
    Search(SearchKeys<'a>),
}

pub(super) struct Append<'a> {
    pub at: Key<'a>,
    pub expected_version: u64,
    pub idempotency_key: &'a str,
    pub event_type: &'a str,
    pub data: &'a JsonObject,
    pub writer: Writer,
    /// On `private`, whether the event makes its key standing.
    pub standing: bool,
    /// On `private`, the last date the event is valid, `YYYY-MM-DD`.
    pub valid_until: Option<&'a str>,
    /// On `private`, the fields of `data` a search matches the key by while
    /// the event is its newest.
    pub searchable: Option<Vec<&'a str>>,
}

pub(super) struct ReadEvents<'a> {
    pub at: Key<'a>,
    pub limit: usize,
    pub after_version: Option<u64>,
}

/// Which keys of a stream a listing or a search reads, by their newest
/// event: the standing ones or the others, or every key, and only those
/// still valid at a date, or every one.
#[derive(Clone, Copy, Default)]
pub(super) struct Selection<'a> {
    pub standing: Option<bool>,
    /// `YYYY-MM-DD`: a key whose newest event's `valid_until` is before it is
    /// passed over.
    pub valid_at: Option<&'a str>,
}

impl Selection<'_> {
    /// Whether a key whose newest event ends on `valid_until` had ended
    /// before the date this selection is valid at. Dates in their one form
    /// order as text.
    pub(super) fn ended(&self, valid_until: Option<&str>) -> bool {
        matches!((self.valid_at, valid_until), (Some(at), Some(until)) if until < at)
    }
}

pub(super) struct ListKeys<'a> {
    pub stream: &'a str,
    pub types: Vec<&'a str>,
    pub selection: Selection<'a>,
    pub limit: usize,
    pub cursor: Option<&'a str>,
}

pub(super) struct SearchKeys<'a> {
    pub stream: &'a str,
    pub query: &'a str,
    pub types: Vec<&'a str>,
    pub selection: Selection<'a>,
    pub limit: usize,
    pub cursor: Option<&'a str>,
}

impl Request<'_> {
    /// Whether the call names what only `private` keeps: a standing event,
    /// an end date, what a search matches, or a selection by either of the
    /// first two.
    pub(super) fn is_private(&self) -> bool {
        let selects = |selection: &Selection<'_>| {
            selection.standing.is_some() || selection.valid_at.is_some()
        };
        match self {
            Self::Append(append) => {
                append.standing || append.valid_until.is_some() || append.searchable.is_some()
            }
            Self::List(list) => selects(&list.selection),
            Self::Search(search) => selects(&search.selection),
            Self::Read(_) | Self::Inspect(_) => false,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppendInput {
    stream: String,
    key: String,
    expected_version: u64,
    #[serde(rename = "type")]
    event_type: String,
    data: JsonObject,
    #[serde(default)]
    standing: bool,
    #[serde(default)]
    valid_until: Option<String>,
    #[serde(default)]
    searchable: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadInput {
    stream: String,
    key: String,
    #[serde(default)]
    after_version: Option<u64>,
    #[serde(default = "read_limit")]
    limit: u64,
    #[serde(default)]
    package: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InspectInput {
    stream: String,
    key: String,
    #[serde(default)]
    package: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListInput {
    stream: String,
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    standing: Option<bool>,
    #[serde(default)]
    valid_at: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default = "list_limit")]
    limit: u64,
    #[serde(default)]
    package: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchInput {
    stream: String,
    query: String,
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    standing: Option<bool>,
    #[serde(default)]
    valid_at: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default = "search_limit")]
    limit: u64,
}

fn read_limit() -> u64 {
    100
}

fn list_limit() -> u64 {
    50
}

fn search_limit() -> u64 {
    20
}

/// The decoded arguments of one call, owned so a request can borrow them.
pub(super) enum Input {
    Append(AppendInput),
    Read(ReadInput),
    Inspect(InspectInput),
    List(ListInput),
    Search(SearchInput),
}

impl Input {
    pub(super) fn decode(operation: &str, arguments: &JsonValue) -> Result<Self, StoreError> {
        fn decode<T: serde::de::DeserializeOwned>(arguments: &JsonValue) -> Result<T, StoreError> {
            arguments
                .clone()
                .deserialize_into()
                .map_err(|error| StoreError::invalid(error.to_string()))
        }
        Ok(match operation {
            "events.append" => Self::Append(decode(arguments)?),
            "events.read" => Self::Read(decode(arguments)?),
            "key.inspect" => Self::Inspect(decode(arguments)?),
            "keys.list" => Self::List(decode(arguments)?),
            "keys.search" => Self::Search(decode(arguments)?),
            other => {
                return Err(StoreError::invalid(format!(
                    "the store has no operation {other}"
                )));
            }
        })
    }

    /// The request on `stream`, the full stream admission rendered, with the
    /// key the kernel derived for a write's intent and the package the call
    /// is made from as an append's writer.
    pub(super) fn request<'a>(
        &'a self,
        stream: &'a str,
        idempotency_key: Option<&'a str>,
        writer: Option<&Writer>,
    ) -> Result<Request<'a>, StoreError> {
        Ok(match self {
            Self::Append(input) => {
                let at = key(stream, &input.stream, None, &input.key)?;
                let idempotency_key = idempotency_key
                    .ok_or_else(|| StoreError::invalid("an append needs an idempotency key"))?;
                token("idempotency_key", idempotency_key, 256)?;
                if input.expected_version >= MAX_PORTABLE_INTEGER {
                    return Err(StoreError::invalid(format!(
                        "expected_version must be less than {MAX_PORTABLE_INTEGER}"
                    )));
                }
                validate_type(&input.event_type)?;
                let writer = writer.cloned().ok_or_else(|| {
                    StoreError::invalid(
                        "an append is made from a package's step, and this one is made from none",
                    )
                })?;
                Request::Append(Append {
                    at,
                    expected_version: input.expected_version,
                    idempotency_key,
                    event_type: &input.event_type,
                    data: &input.data,
                    writer,
                    standing: input.standing,
                    valid_until: date("valid_until", input.valid_until.as_deref())?,
                    searchable: searchable(input.searchable.as_deref(), &input.data)?,
                })
            }
            Self::Read(input) => {
                if input
                    .after_version
                    .is_some_and(|version| version > MAX_PORTABLE_INTEGER)
                {
                    return Err(StoreError::invalid(format!(
                        "after_version must not exceed {MAX_PORTABLE_INTEGER}"
                    )));
                }
                Request::Read(ReadEvents {
                    at: key(stream, &input.stream, input.package.as_deref(), &input.key)?,
                    limit: bounded_limit(input.limit, 500)?,
                    after_version: input.after_version,
                })
            }
            Self::Inspect(input) => Request::Inspect(key(
                stream,
                &input.stream,
                input.package.as_deref(),
                &input.key,
            )?),
            Self::List(input) => Request::List(ListKeys {
                stream: full_stream(stream, &input.stream, input.package.as_deref())?,
                types: types(&input.types)?,
                selection: Selection {
                    standing: input.standing,
                    valid_at: date("valid_at", input.valid_at.as_deref())?,
                },
                limit: bounded_limit(input.limit, 100)?,
                cursor: input.cursor.as_deref(),
            }),
            Self::Search(input) => {
                if input.query.trim().is_empty() || input.query.len() > 512 {
                    return Err(StoreError::invalid("a query is 1 to 512 bytes of words"));
                }
                Request::Search(SearchKeys {
                    stream: full_stream(stream, &input.stream, None)?,
                    query: &input.query,
                    types: types(&input.types)?,
                    selection: Selection {
                        standing: input.standing,
                        valid_at: date("valid_at", input.valid_at.as_deref())?,
                    },
                    limit: bounded_limit(input.limit, 100)?,
                    cursor: input.cursor.as_deref(),
                })
            }
        })
    }
}

/// The full stream admission rendered, checked against what the call names:
/// it ends in the call's own stream, under the package it names when it
/// names one.
fn full_stream<'a>(
    rendered: &'a str,
    named: &str,
    package: Option<&str>,
) -> Result<&'a str, StoreError> {
    validate_stream(named)?;
    let owner = rendered
        .strip_suffix(named)
        .and_then(|owner| owner.strip_suffix('/'))
        .filter(|owner| !owner.is_empty() && package.is_none_or(|package| *owner == package));
    if owner.is_none() || rendered.len() > 512 {
        return Err(StoreError::invalid(
            "the call's stream is not the one admission rendered for it",
        ));
    }
    Ok(rendered)
}

fn key<'a>(
    rendered: &'a str,
    named: &str,
    package: Option<&str>,
    key: &'a str,
) -> Result<Key<'a>, StoreError> {
    validate_key(key)?;
    Ok(Key {
        stream: full_stream(rendered, named, package)?,
        key,
    })
}

fn types(values: &[String]) -> Result<Vec<&str>, StoreError> {
    if values.len() > 16 {
        return Err(StoreError::invalid("types may hold at most 16 values"));
    }
    let mut seen = BTreeSet::new();
    let mut types = Vec::with_capacity(values.len());
    for value in values {
        token("types", value, 128)?;
        if seen.insert(value.as_str()) {
            types.push(value.as_str());
        }
    }
    Ok(types)
}

/// The fields a search matches, when an append names them: 1 to 16
/// distinct fields, each one `data` holds, so a typo can't leave a key
/// silently unsearchable.
fn searchable<'a>(
    fields: Option<&'a [String]>,
    data: &JsonObject,
) -> Result<Option<Vec<&'a str>>, StoreError> {
    let Some(fields) = fields else {
        return Ok(None);
    };
    if !(1..=16).contains(&fields.len()) {
        return Err(StoreError::invalid(
            "searchable names 1 to 16 fields of the event's data",
        ));
    }
    let mut seen = BTreeSet::new();
    for field in fields {
        if !seen.insert(field.as_str()) {
            return Err(StoreError::invalid(format!(
                "searchable names {field} twice"
            )));
        }
        if !data.contains_key(field) {
            return Err(StoreError::invalid(format!(
                "searchable names {field}, which the event's data doesn't hold"
            )));
        }
    }
    Ok(Some(fields.iter().map(String::as_str).collect()))
}

/// `value`, when it's a calendar date, `YYYY-MM-DD`.
fn date<'a>(field: &str, value: Option<&'a str>) -> Result<Option<&'a str>, StoreError> {
    match value {
        Some(value) if runx_contracts::schema::IsoDate::new(value).is_none() => Err(
            StoreError::invalid(format!("{field} must be a calendar date, YYYY-MM-DD")),
        ),
        value => Ok(value),
    }
}

fn bounded_limit(value: u64, maximum: usize) -> Result<usize, StoreError> {
    usize::try_from(value)
        .ok()
        .filter(|value| (1..=maximum).contains(value))
        .ok_or_else(|| StoreError::invalid(format!("limit must be from 1 to {maximum}")))
}

/// A stream's own name: one segment, never a path, so it never spells
/// another package's.
pub(super) fn validate_stream(value: &str) -> Result<(), StoreError> {
    identifier("stream", value, 128, &['.', '_', ':', '-'])
}

pub(super) fn validate_key(value: &str) -> Result<(), StoreError> {
    identifier("key", value, 192, &['.', '_', ':', '@', '/', '-'])
}

/// A declared event type: dotted lowercase words, at least two.
pub(super) fn validate_type(value: &str) -> Result<(), StoreError> {
    let words = value.split('.').collect::<Vec<_>>();
    let valid = value.len() <= 128
        && words.len() >= 2
        && words.iter().all(|word| {
            word.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase() || (index > 0 && (byte.is_ascii_digit() || byte == b'_'))
            }) && !word.is_empty()
        });
    if valid {
        Ok(())
    } else {
        Err(StoreError::invalid(
            "type must be dotted lowercase words, such as counter.bumped",
        ))
    }
}

fn identifier(
    field: &str,
    value: &str,
    maximum: usize,
    allowed: &[char],
) -> Result<(), StoreError> {
    let valid = !value.is_empty()
        && value.len() <= maximum
        && value.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphanumeric() || (index > 0 && allowed.contains(&character))
        });
    if valid {
        Ok(())
    } else {
        Err(StoreError::invalid(format!(
            "{field} must be a safe identifier"
        )))
    }
}

fn token(field: &str, value: &str, maximum: usize) -> Result<(), StoreError> {
    let valid = !value.trim().is_empty()
        && value.trim() == value
        && value.len() <= maximum
        && !value.chars().any(char::is_control);
    if valid {
        Ok(())
    } else {
        Err(StoreError::invalid(format!("{field} is invalid")))
    }
}
