use std::collections::BTreeSet;

use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::RuntimeError;
use crate::receipts::archive::ReceiptFilter;

const MAX_LIMIT: usize = 10_000;
const MAX_EXACT_RECEIPTS: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptQueryInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) issuer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) artifact_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) period: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) as_of: Option<String>,
    pub(crate) limit: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) receipt_ids: Option<Vec<String>>,
    pub(crate) verify_chain: bool,
}

pub(super) struct QueryRequest {
    pub(super) exact_ids: Option<Vec<String>>,
    pub(super) verify_chain: bool,
    pub(super) limit: usize,
    pub(super) filter: ReceiptFilter,
}

impl QueryRequest {
    pub(super) fn parse(inputs: &ReceiptQueryInput) -> Result<Self, RuntimeError> {
        let limit = bounded_limit(inputs.limit)?;
        Ok(Self {
            exact_ids: string_array(inputs.receipt_ids.as_deref(), MAX_EXACT_RECEIPTS)?,
            verify_chain: inputs.verify_chain,
            limit,
            filter: history_filter(inputs, limit)?,
        })
    }

    pub(super) fn filter_json(&self) -> JsonValue {
        JsonValue::Object(JsonObject::from([
            ("query".to_owned(), optional_json(&self.filter.query)),
            ("skill".to_owned(), optional_json(&self.filter.skill)),
            ("status".to_owned(), optional_json(&self.filter.status)),
            ("issuer".to_owned(), optional_json(&self.filter.issuer)),
            ("actor".to_owned(), optional_json(&self.filter.actor)),
            (
                "artifact_type".to_owned(),
                optional_json(&self.filter.artifact_type),
            ),
            ("since".to_owned(), optional_json(&self.filter.since)),
            ("until".to_owned(), optional_json(&self.filter.until)),
            (
                "limit".to_owned(),
                JsonValue::Number(JsonNumber::U64(self.limit as u64)),
            ),
            (
                "receipt_ids".to_owned(),
                JsonValue::Array(
                    self.exact_ids
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .cloned()
                        .map(JsonValue::String)
                        .collect(),
                ),
            ),
        ]))
    }
}

fn history_filter(inputs: &ReceiptQueryInput, limit: usize) -> Result<ReceiptFilter, RuntimeError> {
    Ok(ReceiptFilter {
        query: optional_string(inputs.query.as_deref()),
        skill: optional_string(inputs.skill.as_deref()),
        status: optional_string(inputs.status.as_deref()),
        issuer: optional_string(inputs.issuer.as_deref()),
        actor: optional_string(inputs.actor.as_deref()),
        artifact_type: optional_string(inputs.artifact_type.as_deref()),
        since: resolve_since(inputs)?,
        until: optional_string(inputs.until.as_deref()),
        limit: Some(limit),
        include_internal: false,
    })
}

fn resolve_since(inputs: &ReceiptQueryInput) -> Result<Option<String>, RuntimeError> {
    if let Some(value) = optional_string(inputs.since.as_deref()) {
        return Ok(Some(value));
    }
    let Some(period) = optional_string(inputs.period.as_deref()) else {
        return Ok(None);
    };
    crate::time::lookback_since(&period, optional_string(inputs.as_of.as_deref()).as_deref())
        .map(Some)
        .map_err(invalid)
}

fn bounded_limit(value: u64) -> Result<usize, RuntimeError> {
    bounded_integer(value)
}

fn bounded_integer(value: u64) -> Result<usize, RuntimeError> {
    usize::try_from(value)
        .ok()
        .filter(|value| (1..=MAX_LIMIT).contains(value))
        .ok_or_else(|| invalid("limit must be an integer from 1 to 10000"))
}

fn string_array(
    values: Option<&[String]>,
    max: usize,
) -> Result<Option<Vec<String>>, RuntimeError> {
    let Some(values) = values else {
        return Ok(None);
    };
    if values.is_empty() {
        return Ok(None);
    }
    if values.len() > max {
        return Err(invalid(format!(
            "receipt_ids must contain at most {max} entries"
        )));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            return Err(invalid("receipt_ids entries must be non-empty strings"));
        }
        unique.insert(value.to_owned());
    }
    Ok(Some(unique.into_iter().collect()))
}

fn optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn optional_json(value: &Option<String>) -> JsonValue {
    value
        .as_ref()
        .map(|value| JsonValue::String(value.clone()))
        .unwrap_or(JsonValue::Null)
}
