//! Act assignment envelope: host kind, actor, intent key, and idempotency hashing.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{JsonObject, JsonValue};

mod hash;

use hash::{sha256_prefixed, stable_hash_json};

pub const ACT_ASSIGNMENT_SCHEMA: &str = "runx.act_assignment.v1";
pub const SHA256_ALGORITHM: &str = "sha256";

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
    /// A comment on a provider thread; `trigger_ref` locates it.
    Comment,
    System,
}

// `ActAssignmentActor` / `ActAssignmentHost` fields stay `String`: they feed the
// fixture-backed idempotency hash pipeline (`normalize_*`, `derive_*`), whose
// digests must not change. Builders normalize empty strings
// away before hashing; the contract keeps the raw inbound envelope permissive.
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
    pub trigger_ref: Option<String>,
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
    pub content_hash: NonEmptyString,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.act_assignment.v1")]
pub struct ActAssignment {
    pub schema: ActAssignmentSchema,
    pub skill_ref: NonEmptyString,
    pub runner: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<NonEmptyString>,
    pub requested_at: IsoDateTime,
    pub host: ActAssignmentHost,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_overrides: Option<JsonObject>,
    pub idempotency: ActAssignmentIdempotency,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildActAssignment {
    pub skill_ref: String,
    pub runner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub requested_at: String,
    pub host: ActAssignmentHost,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_overrides: Option<JsonObject>,
}

impl BuildActAssignment {
    #[must_use]
    pub fn build(self) -> ActAssignment {
        let input_overrides = non_empty_object(self.input_overrides);
        let source_ref = non_empty_string(self.source_ref);
        let host = normalize_host(self.host);
        let trigger_key = derive_trigger_key(host.kind.clone(), host.trigger_ref.clone());
        let content_hash = derive_content_hash(input_overrides.clone());

        ActAssignment {
            schema: ActAssignmentSchema::V1,
            skill_ref: self.skill_ref.clone().into(),
            runner: self.runner.clone().into(),
            source_ref: source_ref.clone().map(Into::into),
            requested_at: self.requested_at.into(),
            host,
            input_overrides: input_overrides.clone(),
            idempotency: ActAssignmentIdempotency {
                algorithm: SHA256_ALGORITHM.to_owned(),
                intent_key: derive_intent_key(IntentKeyInput {
                    skill_ref: self.skill_ref,
                    runner: self.runner,
                    source_ref,
                    input_overrides,
                })
                .into(),
                trigger_key: trigger_key.map(Into::into),
                content_hash: content_hash.into(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentKeyInput {
    pub skill_ref: String,
    pub runner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_overrides: Option<JsonObject>,
}

/// Derives the stable act assignment intent idempotency key.
///
/// The digest is fixture-backed for the contract envelope: ASCII object keys in
/// a fixed order, JSON-string values with `JSON.stringify` escape semantics, and
/// integer numeric values.
#[must_use]
pub fn derive_intent_key(input: IntentKeyInput) -> String {
    sha256_prefixed(&stable_hash_json(&intent_hash_payload(input)))
}

/// Derives the stable trigger idempotency key when a non-empty trigger exists.
///
/// The same fixture-backed hash envelope as [`derive_intent_key`] applies.
#[must_use]
pub fn derive_trigger_key(
    host_kind: ActAssignmentHostKind,
    trigger_ref: Option<String>,
) -> Option<String> {
    let trigger_ref = non_empty_string(trigger_ref)?;
    let mut payload = BTreeMap::new();
    payload.insert(
        "host_kind".to_owned(),
        JsonValue::String(host_kind_to_string(&host_kind)),
    );
    payload.insert("trigger_ref".to_owned(), JsonValue::String(trigger_ref));
    Some(sha256_prefixed(&stable_hash_json(&JsonValue::Object(
        payload,
    ))))
}

/// Derives the stable content hash for act assignment input overrides.
///
/// The same fixture-backed hash envelope as [`derive_intent_key`] applies.
#[must_use]
pub fn derive_content_hash(input_overrides: Option<JsonObject>) -> String {
    sha256_prefixed(&stable_hash_json(&JsonValue::Object(
        non_empty_object(input_overrides).unwrap_or_default(),
    )))
}

fn intent_hash_payload(input: IntentKeyInput) -> JsonValue {
    let mut payload = BTreeMap::new();
    payload.insert("skill_ref".to_owned(), JsonValue::String(input.skill_ref));
    payload.insert("runner".to_owned(), JsonValue::String(input.runner));
    if let Some(source_ref) = non_empty_string(input.source_ref) {
        payload.insert("source_ref".to_owned(), JsonValue::String(source_ref));
    }
    if let Some(input_overrides) = non_empty_object(input.input_overrides) {
        payload.insert(
            "input_overrides".to_owned(),
            JsonValue::Object(input_overrides),
        );
    }
    JsonValue::Object(payload)
}

fn non_empty_string(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn non_empty_object(value: Option<JsonObject>) -> Option<JsonObject> {
    // The TS oracle recursively prunes only `undefined`, which cannot appear in
    // this JSON value model. Nested nulls and empty objects are preserved as
    // observable JSON; only a top-level empty override object is omitted.
    value.filter(|value| !value.is_empty())
}

fn normalize_host(host: ActAssignmentHost) -> ActAssignmentHost {
    ActAssignmentHost {
        kind: host.kind,
        trigger_ref: non_empty_string(host.trigger_ref),
        scope_set: normalize_scope_set(host.scope_set),
        actor: normalize_actor(host.actor),
    }
}

fn normalize_scope_set(value: Option<Vec<String>>) -> Option<Vec<String>> {
    let scope_set: Vec<String> = value
        .unwrap_or_default()
        .into_iter()
        .filter(|scope| !scope.is_empty())
        .collect();
    (!scope_set.is_empty()).then_some(scope_set)
}

fn normalize_actor(actor: Option<ActAssignmentActor>) -> Option<ActAssignmentActor> {
    let actor = actor?;
    let normalized = ActAssignmentActor {
        actor_id: non_empty_string(actor.actor_id),
        display_name: non_empty_string(actor.display_name),
        role: non_empty_string(actor.role),
        provider_identity: non_empty_string(actor.provider_identity),
    };
    [
        &normalized.actor_id,
        &normalized.display_name,
        &normalized.role,
        &normalized.provider_identity,
    ]
    .iter()
    .any(|value| value.is_some())
    .then_some(normalized)
}

fn host_kind_to_string(kind: &ActAssignmentHostKind) -> String {
    match kind {
        ActAssignmentHostKind::Cli => "cli",
        ActAssignmentHostKind::Api => "api",
        ActAssignmentHostKind::Comment => "comment",
        ActAssignmentHostKind::System => "system",
    }
    .to_owned()
}
