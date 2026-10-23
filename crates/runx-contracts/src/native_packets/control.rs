//! What the native control tools return.

use serde::{Deserialize, Serialize};

use crate::JsonObject;
use crate::schema::RunxSchema;

/// What `control.capture_harness_context` returns: the harness, signal and
/// decision packets a run was given, held as explicit context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.control.harness_context.v1")]
pub struct HarnessContext {
    pub captured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<JsonObject>,
}
