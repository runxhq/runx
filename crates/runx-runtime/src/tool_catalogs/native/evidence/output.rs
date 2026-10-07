use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::CapabilityOutput;

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceIndexOutput {
    source_index: SourceIndex,
}

impl CapabilityOutput for EvidenceIndexOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct SourceIndex {
    decision: String,
    objective: String,
    sources: Vec<IndexedSource>,
    source_digests: Vec<String>,
    source_evidence: Vec<SourceEvidence>,
    index_digest: String,
    blockers: Vec<String>,
    limits: IndexLimits,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct IndexedSource {
    source_digest: String,
    content_digest: String,
    source_ref: String,
    source_kind: String,
    extracted: String,
    provenance: SourceProvenance,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct SourceEvidence {
    evidence_digest: String,
    content_digest: String,
    source_ref: String,
    source_kind: String,
    provenance: SourceProvenance,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct SourceProvenance {
    observed_at: String,
    bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    redirects: Vec<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct IndexLimits {
    max_sources: u64,
    max_source_characters: u64,
    max_total_characters: u64,
    supplied_sources: u64,
    indexed_sources: u64,
    indexed_characters: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
pub(super) struct EvidenceVerifyOutput {
    verification: Verification,
    #[serde(flatten)]
    artifacts: BTreeMap<String, JsonObject>,
}

impl CapabilityOutput for EvidenceVerifyOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct Verification {
    status: String,
    findings: Vec<Finding>,
    admitted_source_digests: Vec<String>,
    admitted_context_digests: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct Finding {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}
