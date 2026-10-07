//! Portable conversation intake envelopes. Provider-specific message fields
//! remain opaque here; the operator and inbox validate the common identity
//! fields before recording an action.

use serde::{Deserialize, Serialize};

use crate::JsonObject;
use crate::schema::{NonEmptyString, RunxSchema};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ConversationPageSchema {
    #[serde(rename = "runx.conversation_page.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.conversation_page.v1")]
pub struct ConversationPage {
    pub schema: ConversationPageSchema,
    pub items: Vec<JsonObject>,
    pub next_cursor: Option<NonEmptyString>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ConversationObservationSchema {
    #[serde(rename = "runx.conversation_observation.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.conversation_observation.v1")]
pub struct ConversationObservation {
    pub schema: ConversationObservationSchema,
    pub source_kind: NonEmptyString,
    pub summary: NonEmptyString,
    pub message: JsonObject,
}
