//! What the native run tools return.

use serde::{Deserialize, Serialize};

use crate::receipt::RunOutcome;
use crate::run_read::RunState;
use crate::schema::{BoundedVec, NonEmptyString, RunxSchema};

/// The most runs one `run.list` returns.
pub const RUN_LIST_MAX_RUNS: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum RunListSchema {
    #[serde(rename = "runx.run.list.v1")]
    V1,
}

/// What `run.list` returns: the workspace's top-level runs, most recently
/// advanced first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.run.list.v1")]
pub struct RunList {
    pub schema: RunListSchema,
    pub runs: BoundedVec<RunListEntry, 0, RUN_LIST_MAX_RUNS>,
}

/// One run of a listing, read from its first and latest events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct RunListEntry {
    pub run_id: NonEmptyString,
    pub skill: NonEmptyString,
    pub runner: NonEmptyString,
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// The request a paused run's step is held on: its turn's act, or what
    /// it asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_request_id: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<NonEmptyString>,
    pub created_at: NonEmptyString,
    pub updated_at: NonEmptyString,
}
