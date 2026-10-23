use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};
use super::output::{EvidenceIndexOutput, EvidenceVerifyOutput};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceIndexInput {
    pub(super) objective: String,
    pub(super) source_packets: Vec<JsonValue>,
    pub(super) max_sources: u64,
    pub(super) max_source_characters: u64,
    pub(super) max_total_characters: u64,
}

impl CapabilityInput for EvidenceIndexInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            (
                "max_sources".to_owned(),
                JsonValue::Number(JsonNumber::U64(20)),
            ),
            (
                "max_source_characters".to_owned(),
                JsonValue::Number(JsonNumber::U64(100_000)),
            ),
            (
                "max_total_characters".to_owned(),
                JsonValue::Number(JsonNumber::U64(500_000)),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceVerifyInput {
    pub(super) artifact_name: String,
    pub(super) authoritative_bindings: Vec<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) authoritative_fields: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) authoritative_source: Option<JsonValue>,
    pub(super) blocked_decision: String,
    pub(super) candidate: JsonObject,
    pub(super) claim_bindings: Vec<JsonValue>,
    pub(super) context_bindings: Vec<JsonValue>,
    pub(super) context_requirements: Vec<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fallback_artifact: Option<JsonObject>,
    pub(super) fallback_bindings: Vec<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fallback_source: Option<JsonValue>,
    pub(super) forbid_external_effects: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) identity_source: Option<JsonValue>,
    pub(super) ready_decision: String,
    pub(super) reference_bindings: Vec<JsonValue>,
    pub(super) require_all_contexts: bool,
    pub(super) require_claim_bindings: bool,
    pub(super) required_paths: Vec<JsonValue>,
    pub(super) required_value_bindings: Vec<JsonValue>,
    pub(super) required_values: Vec<JsonValue>,
    pub(super) source_digests: Vec<String>,
    pub(super) source_records: Vec<JsonValue>,
}

impl CapabilityInput for EvidenceVerifyInput {
    fn defaults() -> JsonObject {
        let empty = || JsonValue::Array(Vec::new());
        JsonObject::from([
            (
                "artifact_name".to_owned(),
                JsonValue::String("verified_artifact".to_owned()),
            ),
            ("authoritative_bindings".to_owned(), empty()),
            (
                "blocked_decision".to_owned(),
                JsonValue::String("needs_more_evidence".to_owned()),
            ),
            ("claim_bindings".to_owned(), empty()),
            ("context_bindings".to_owned(), empty()),
            ("context_requirements".to_owned(), empty()),
            ("fallback_bindings".to_owned(), empty()),
            ("forbid_external_effects".to_owned(), JsonValue::Bool(true)),
            (
                "ready_decision".to_owned(),
                JsonValue::String("ready".to_owned()),
            ),
            ("reference_bindings".to_owned(), empty()),
            ("require_all_contexts".to_owned(), JsonValue::Bool(true)),
            ("require_claim_bindings".to_owned(), JsonValue::Bool(true)),
            ("required_paths".to_owned(), empty()),
            ("required_value_bindings".to_owned(), empty()),
            ("required_values".to_owned(), empty()),
            ("source_digests".to_owned(), empty()),
            ("source_records".to_owned(), empty()),
        ])
    }
}

const INDEX_FIELDS: &[CapabilityField] = &[
    field(
        "objective",
        "Bounded objective the source set must support.",
    ),
    field("source_packets", "Governed source packets to index."),
    field("max_sources", "Maximum number of source packets admitted."),
    field(
        "max_source_characters",
        "Maximum characters admitted from one source.",
    ),
    field(
        "max_total_characters",
        "Maximum characters admitted across all sources.",
    ),
];

const VERIFY_FIELDS: &[CapabilityField] = &[
    field(
        "artifact_name",
        "Top-level key used for the verified artifact.",
    ),
    field(
        "authoritative_bindings",
        "Paths copied from the authoritative source.",
    ),
    field(
        "authoritative_fields",
        "Top-level fields overriding candidate values.",
    ),
    field(
        "authoritative_source",
        "Admitted source used by authoritative bindings.",
    ),
    field(
        "blocked_decision",
        "Decision emitted when verification fails.",
    ),
    field("candidate", "Agent-authored candidate artifact."),
    field("claim_bindings", "Candidate claims and source digests."),
    field(
        "context_bindings",
        "Context bindings claimed by the candidate.",
    ),
    field(
        "context_requirements",
        "Admitted context digests and rules.",
    ),
    field(
        "fallback_artifact",
        "Sanitized fallback emitted on failure.",
    ),
    field(
        "fallback_bindings",
        "Paths copied onto the fallback artifact.",
    ),
    field(
        "fallback_source",
        "Admitted source used by fallback bindings.",
    ),
    field(
        "forbid_external_effects",
        "Reject unsupported external-effect claims.",
    ),
    field(
        "identity_source",
        "Admitted source used for identity bindings.",
    ),
    field(
        "ready_decision",
        "Candidate decision requesting release as ready.",
    ),
    field(
        "reference_bindings",
        "Additional records whose digests must be admitted.",
    ),
    field(
        "require_all_contexts",
        "Require every admitted context to be bound.",
    ),
    field(
        "require_claim_bindings",
        "Require a bound claim for ready artifacts.",
    ),
    field(
        "required_paths",
        "Candidate paths that must resolve non-empty.",
    ),
    field(
        "required_value_bindings",
        "Candidate and identity paths that must match.",
    ),
    field(
        "required_values",
        "Candidate path values bound to caller input.",
    ),
    field(
        "source_digests",
        "Exact source digests admitted for claims.",
    ),
    field(
        "source_records",
        "Admitted records used to verify source content digests.",
    ),
];

const fn field(name: &'static str, description: &'static str) -> CapabilityField {
    CapabilityField { name, description }
}

static INDEX: TypedNativeCapability<EvidenceIndexInput, EvidenceIndexOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "evidence.index_sources",
            source: "runx-runtime/evidence",
            summary: "Bound, deduplicate, and digest governed remote or local source packets.",
            scopes: &["runx:evidence:read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::None,
            fields: INDEX_FIELDS,
        },
        super::index_sources,
    );

static VERIFY: TypedNativeCapability<EvidenceVerifyInput, EvidenceVerifyOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "evidence.verify_artifact",
            source: "runx-runtime/evidence",
            summary: "Verify evidence and context bindings and reject unsupported effect claims.",
            scopes: &["runx:evidence:read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::None,
            fields: VERIFY_FIELDS,
        },
        super::verify_artifact,
    );

pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&INDEX, &VERIFY];
