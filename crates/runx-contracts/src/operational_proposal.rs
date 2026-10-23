//! Operational proposal contract: reviewable handoffs over existing actions.
// Module rationale: the proposal schema, the open reference
// type vocabulary, the human-gate and outcome shapes, and the RunxSchema
// reflection together form one cross-language wire surface.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::schema::{
    Identity, IsoDateTime, NamedSchemas, NonEmptyString, Property, RunxSchema,
    deserialize_false_bool, deserialize_true_bool, object_schema, record_named, titled,
};
use crate::{JsonObject, Reference, ReferenceLink};

pub const OPERATIONAL_PROPOSAL_SCHEMA: &str = "runx.operational_proposal.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum OperationalProposalSchema {
    #[serde(rename = "runx.operational_proposal.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationalProposalRedactionStatus {
    /// Private source evidence which has not crossed an outward sharing boundary.
    Unredacted,
    Redacted,
    SummaryOnly,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalProposalRecommendedAction {
    pub action_intent: NonEmptyString,
    pub summary: NonEmptyString,
    pub mutating: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_refs: Vec<Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalProposalIdempotency {
    pub key: NonEmptyString,
    pub digest: NonEmptyString,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalProposalAuthority {
    #[serde(deserialize_with = "deserialize_true_bool")]
    pub proposal_only: bool,
    #[serde(deserialize_with = "deserialize_false_bool")]
    pub mutation_authority_granted: bool,
    #[serde(deserialize_with = "deserialize_false_bool")]
    pub publication_authority_granted: bool,
    #[serde(deserialize_with = "deserialize_false_bool")]
    pub final_decision_authority_granted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NonEmptyString>,
}

impl RunxSchema for OperationalProposalAuthority {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("proposal_only", const_bool(true), true),
                Property::new("mutation_authority_granted", const_bool(false), true),
                Property::new("publication_authority_granted", const_bool(false), true),
                Property::new("final_decision_authority_granted", const_bool(false), true),
                Property::new("notes", Vec::<NonEmptyString>::json_schema(), false),
            ],
            true,
            None,
        );
        titled(schema, "OperationalProposalAuthority")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        record_named(named, "OperationalProposalAuthority", Self::json_schema);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalProposalHumanGate {
    pub gate_id: NonEmptyString,
    pub gate_kind: NonEmptyString,
    pub required: bool,
    pub decision: NonEmptyString,
    pub reason: NonEmptyString,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalProposalOutcome {
    pub observed: bool,
    pub status: NonEmptyString,
    pub summary: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<IsoDateTime>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<Reference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalProposal {
    pub schema: OperationalProposalSchema,
    pub proposal_id: NonEmptyString,
    pub proposal_kind: NonEmptyString,
    pub source_event_id: NonEmptyString,
    pub idempotency: OperationalProposalIdempotency,
    pub source_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_thread_ref: Option<Reference>,
    pub hydrated_context_ref: Reference,
    pub redaction_status: OperationalProposalRedactionStatus,
    pub decision_summary: NonEmptyString,
    pub rationale: NonEmptyString,
    #[serde(default)]
    pub recommended_actions: Vec<OperationalProposalRecommendedAction>,
    #[serde(default)]
    pub evidence_refs: Vec<Reference>,
    #[serde(default)]
    pub artifact_refs: Vec<Reference>,
    #[serde(default)]
    pub receipt_refs: Vec<Reference>,
    #[serde(default)]
    pub story_refs: Vec<Reference>,
    #[serde(default)]
    pub result_refs: Vec<ReferenceLink>,
    #[serde(default)]
    pub publication_refs: Vec<ReferenceLink>,
    pub owner_route_id: NonEmptyString,
    pub confidence: f64,
    #[serde(default)]
    pub risks: Vec<NonEmptyString>,
    #[serde(default)]
    pub caveats: Vec<NonEmptyString>,
    #[serde(default)]
    pub missing_context: Vec<NonEmptyString>,
    pub authority: OperationalProposalAuthority,
    #[serde(default)]
    pub human_gates: Vec<OperationalProposalHumanGate>,
    #[serde(default)]
    pub allowed_next_actions: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_outcome: Option<OperationalProposalOutcome>,
    pub public_summary: NonEmptyString,
    /// Product-neutral extensions. `runx.escalation` carries escalation
    /// severity and urgency without adding provider-specific core fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
}

impl RunxSchema for OperationalProposal {
    // Function rationale: the public proposal schema is a single
    // closed contract document, and keeping its field list contiguous makes
    // review against the wire contract less error-prone.
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("schema", OperationalProposalSchema::json_schema(), true),
                Property::new("proposal_id", id_schema(), true),
                Property::new("proposal_kind", NonEmptyString::json_schema(), true),
                Property::new("source_event_id", id_schema(), true),
                Property::new(
                    "idempotency",
                    OperationalProposalIdempotency::json_schema(),
                    true,
                ),
                Property::new("source_ref", Reference::json_schema(), true),
                Property::new("source_thread_ref", Reference::json_schema(), false),
                Property::new("hydrated_context_ref", Reference::json_schema(), true),
                Property::new(
                    "redaction_status",
                    OperationalProposalRedactionStatus::json_schema(),
                    true,
                ),
                Property::new("decision_summary", NonEmptyString::json_schema(), true),
                Property::new("rationale", NonEmptyString::json_schema(), true),
                Property::new(
                    "recommended_actions",
                    Vec::<OperationalProposalRecommendedAction>::json_schema(),
                    false,
                ),
                Property::new("evidence_refs", Vec::<Reference>::json_schema(), false),
                Property::new("artifact_refs", Vec::<Reference>::json_schema(), false),
                Property::new("receipt_refs", Vec::<Reference>::json_schema(), false),
                Property::new("story_refs", Vec::<Reference>::json_schema(), false),
                Property::new("result_refs", Vec::<ReferenceLink>::json_schema(), false),
                Property::new(
                    "publication_refs",
                    Vec::<ReferenceLink>::json_schema(),
                    false,
                ),
                Property::new("owner_route_id", id_schema(), true),
                Property::new("confidence", confidence_schema(), true),
                Property::new("risks", Vec::<NonEmptyString>::json_schema(), false),
                Property::new("caveats", Vec::<NonEmptyString>::json_schema(), false),
                Property::new(
                    "missing_context",
                    Vec::<NonEmptyString>::json_schema(),
                    false,
                ),
                Property::new(
                    "authority",
                    OperationalProposalAuthority::json_schema(),
                    true,
                ),
                Property::new(
                    "human_gates",
                    Vec::<OperationalProposalHumanGate>::json_schema(),
                    false,
                ),
                Property::new(
                    "allowed_next_actions",
                    Vec::<NonEmptyString>::json_schema(),
                    false,
                ),
                Property::new(
                    "final_outcome",
                    OperationalProposalOutcome::json_schema(),
                    false,
                ),
                Property::new("public_summary", NonEmptyString::json_schema(), true),
                Property::new("extensions", JsonObject::json_schema(), false),
            ],
            true,
            Some(Identity::Runx {
                logical: OPERATIONAL_PROPOSAL_SCHEMA,
                url: None,
            }),
        );
        titled(schema, "OperationalProposal")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "OperationalProposal", Self::json_schema) {
            OperationalProposalSchema::named_schemas(named);
            OperationalProposalIdempotency::named_schemas(named);
            Reference::named_schemas(named);
            OperationalProposalRedactionStatus::named_schemas(named);
            OperationalProposalRecommendedAction::named_schemas(named);
            ReferenceLink::named_schemas(named);
            OperationalProposalAuthority::named_schemas(named);
            OperationalProposalHumanGate::named_schemas(named);
            OperationalProposalOutcome::named_schemas(named);
        }
    }
}

fn id_schema() -> Value {
    json!({
        "minLength": 1,
        "pattern": "^[A-Za-z0-9_.:-]+$",
        "type": "string"
    })
}

fn confidence_schema() -> Value {
    json!({
        "maximum": 1,
        "minimum": 0,
        "type": "number"
    })
}

fn const_bool(value: bool) -> Value {
    json!({ "const": value, "type": "boolean" })
}
