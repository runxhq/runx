//! Stable vocabulary for execution ceilings that shape a governed run.
//!
//! Resource owners keep their numeric constants beside enforcement. This
//! contract gives every adapter the same receipt-visible representation instead
//! of centralizing unrelated policy in a god table.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The key a receipt's unsigned metadata holds the invocation's limits under.
pub const EXECUTION_LIMITS_METADATA: &str = "execution_limits";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionLimitUnit {
    Bytes,
    Count,
    Jobs,
    Milliseconds,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLimit {
    pub configured: u64,
    pub maximum: u64,
    pub unit: ExecutionLimitUnit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_field: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLimitHit {
    pub id: String,
    #[serde(flatten)]
    pub limit: ExecutionLimit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLimits {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub configured: BTreeMap<String, ExecutionLimit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit: Option<ExecutionLimitHit>,
}
