//! Act assignment envelope: host kind, actor, intent key, and idempotency.
use serde::{Deserialize, Serialize};

use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::JsonObject;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ActAssignmentSchema {
    #[serde(rename = "runx.act_assignment.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActAssignmentHostKind {
    Cli,
    Api,
    /// A comment on a provider thread; `trigger_uri` locates it.
    Comment,
    System,
}

// `ActAssignmentActor` / `ActAssignmentHost` fields stay `String`: they feed the
// fixture-backed idempotency digest pipeline (`normalize_*`, `derive_*`), whose
// digests must not change. Builders normalize empty strings
// away before digesting; the contract keeps the raw inbound envelope permissive.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ActAssignmentActor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_identity: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ActAssignmentHost {
    pub kind: ActAssignmentHostKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_set: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<ActAssignmentActor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ActAssignmentIdempotency {
    pub algorithm: String,
    pub intent_key: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_key: Option<NonEmptyString>,
    pub content_digest: NonEmptyString,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.act_assignment.v1")]
pub struct ActAssignment {
    pub schema: ActAssignmentSchema,
    pub skill: NonEmptyString,
    pub runner: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_uri: Option<NonEmptyString>,
    pub requested_at: IsoDateTime,
    pub host: ActAssignmentHost,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_overrides: Option<JsonObject>,
    pub idempotency: ActAssignmentIdempotency,
}
