//! A store call's arguments, decoded and checked once for every backend.

use std::collections::BTreeSet;

use runx_contracts::{JsonObject, JsonValue, MAX_PORTABLE_INTEGER};
use serde::Deserialize;

use super::StoreError;

/// The stream an event belongs to.
#[derive(Clone, Copy, Debug)]
pub(super) struct Source<'a> {
    pub data_source_ref: &'a str,
    pub resource: &'a str,
    pub aggregate_id: &'a str,
}

/// One store operation, by the operation name the connector declares.
pub(super) enum Request<'a> {
    Append(Append<'a>),
    ReadEvents(ReadEvents<'a>),
    ReadProjection(Source<'a>),
    ListHeads(ListHeads<'a>),
}

pub(super) struct Append<'a> {
    pub source: Source<'a>,
    pub expected_version: u64,
    pub idempotency_key: &'a str,
    pub event: &'a JsonObject,
    pub observed_at: Option<&'a str>,
}

pub(super) struct ReadEvents<'a> {
    pub source: Source<'a>,
    pub limit: usize,
    pub after_version: Option<u64>,
}

pub(super) struct ListHeads<'a> {
    pub data_source_ref: &'a str,
    pub resource: &'a str,
    pub event_types: Vec<&'a str>,
    pub limit: usize,
    pub cursor: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppendInput {
    data_source_ref: String,
    resource: String,
    aggregate_id: String,
    expected_version: u64,
    event: JsonObject,
    #[serde(default)]
    observed_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadEventsInput {
    data_source_ref: String,
    resource: String,
    aggregate_id: String,
    #[serde(default = "default_limit")]
    limit: u64,
    #[serde(default)]
    after_version: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadProjectionInput {
    data_source_ref: String,
    resource: String,
    aggregate_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListHeadsInput {
    data_source_ref: String,
    resource: String,
    #[serde(default)]
    event_types: Vec<String>,
    #[serde(default = "default_limit")]
    limit: u64,
    #[serde(default)]
    cursor: Option<String>,
}

fn default_limit() -> u64 {
    50
}

/// The decoded arguments of one call, owned so a request can borrow them.
pub(super) enum Input {
    Append(AppendInput),
    ReadEvents(ReadEventsInput),
    ReadProjection(ReadProjectionInput),
    ListHeads(ListHeadsInput),
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
            "events.read" => Self::ReadEvents(decode(arguments)?),
            "projection.read" => Self::ReadProjection(decode(arguments)?),
            "heads.list" => Self::ListHeads(decode(arguments)?),
            other => {
                return Err(StoreError::invalid(format!(
                    "the store has no operation {other}"
                )));
            }
        })
    }

    /// The request, with the key the kernel derived for a write's intent.
    pub(super) fn request<'a>(&'a self, key: Option<&'a str>) -> Result<Request<'a>, StoreError> {
        Ok(match self {
            Self::Append(input) => {
                let source = source(&input.data_source_ref, &input.resource, &input.aggregate_id)?;
                let idempotency_key =
                    key.ok_or_else(|| StoreError::invalid("an append needs an idempotency key"))?;
                token("idempotency_key", idempotency_key, 256, true)?;
                if input.expected_version >= MAX_PORTABLE_INTEGER {
                    return Err(StoreError::invalid(format!(
                        "expected_version must be less than {MAX_PORTABLE_INTEGER}"
                    )));
                }
                if let Some(observed_at) = input.observed_at.as_deref()
                    && runx_core::policy::parse_rfc3339_moment(observed_at).is_none()
                {
                    return Err(StoreError::invalid("observed_at must be RFC 3339"));
                }
                Request::Append(Append {
                    source,
                    expected_version: input.expected_version,
                    idempotency_key,
                    event: &input.event,
                    observed_at: input.observed_at.as_deref(),
                })
            }
            Self::ReadEvents(input) => {
                let source = source(&input.data_source_ref, &input.resource, &input.aggregate_id)?;
                if input
                    .after_version
                    .is_some_and(|version| version > MAX_PORTABLE_INTEGER)
                {
                    return Err(StoreError::invalid(format!(
                        "after_version must not exceed {MAX_PORTABLE_INTEGER}"
                    )));
                }
                Request::ReadEvents(ReadEvents {
                    source,
                    limit: bounded_limit(input.limit, 500)?,
                    after_version: input.after_version,
                })
            }
            Self::ReadProjection(input) => Request::ReadProjection(source(
                &input.data_source_ref,
                &input.resource,
                &input.aggregate_id,
            )?),
            Self::ListHeads(input) => {
                token("data_source_ref", &input.data_source_ref, 512, true)?;
                identifier("resource", &input.resource, false)?;
                if input.event_types.len() > 20 {
                    return Err(StoreError::invalid(
                        "event_types may contain at most 20 values",
                    ));
                }
                let mut seen = BTreeSet::new();
                let mut event_types = Vec::with_capacity(input.event_types.len());
                for event_type in &input.event_types {
                    validate_event_type(event_type)?;
                    if seen.insert(event_type.as_str()) {
                        event_types.push(event_type.as_str());
                    }
                }
                Request::ListHeads(ListHeads {
                    data_source_ref: &input.data_source_ref,
                    resource: &input.resource,
                    event_types,
                    limit: bounded_limit(input.limit, 100)?,
                    cursor: input.cursor.as_deref().filter(|cursor| !cursor.is_empty()),
                })
            }
        })
    }
}

fn source<'a>(
    data_source_ref: &'a str,
    resource: &'a str,
    aggregate_id: &'a str,
) -> Result<Source<'a>, StoreError> {
    token("data_source_ref", data_source_ref, 512, true)?;
    identifier("resource", resource, false)?;
    identifier("aggregate_id", aggregate_id, true)?;
    Ok(Source {
        data_source_ref,
        resource,
        aggregate_id,
    })
}

fn bounded_limit(value: u64, maximum: usize) -> Result<usize, StoreError> {
    usize::try_from(value)
        .ok()
        .filter(|value| (1..=maximum).contains(value))
        .ok_or_else(|| StoreError::invalid(format!("limit must be from 1 to {maximum}")))
}

pub(super) fn validate_event_type(value: &str) -> Result<(), StoreError> {
    token("event_type", value, 128, false)
}

pub(super) fn validate_aggregate_id(value: &str) -> Result<(), StoreError> {
    identifier("aggregate_id", value, true)
}

fn identifier(field: &str, value: &str, aggregate: bool) -> Result<(), StoreError> {
    let valid = !value.is_empty()
        && value.len() <= if aggregate { 192 } else { 128 }
        && value.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphanumeric()
                || (index > 0
                    && matches!(character, '.' | '_' | ':' | '-' | '@' | '/')
                    && (aggregate || character != '@' && character != '/'))
        });
    if valid {
        Ok(())
    } else {
        Err(StoreError::invalid(format!(
            "{field} must be a safe identifier"
        )))
    }
}

fn token(field: &str, value: &str, maximum: usize, allow_slash: bool) -> Result<(), StoreError> {
    let valid = !value.trim().is_empty()
        && value.trim() == value
        && value.len() <= maximum
        && !value.chars().any(char::is_control)
        && (allow_slash || !value.contains('/'));
    if valid {
        Ok(())
    } else {
        Err(StoreError::invalid(format!("{field} is invalid")))
    }
}
