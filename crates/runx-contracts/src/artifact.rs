//! Artifact contract: emitted artifacts and their producer attribution.
use serde::{Deserialize, Serialize};

use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{ActAddress, DigestAlgorithm, JsonObject, Reference};

pub const ARTIFACT_SCHEMA: &str = "runx.artifact.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ArtifactSchema {
    #[serde(rename = "runx.artifact.v1")]
    V1,
}

/// An artifact's digest and how its bytes were canonicalized for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct DigestCommitment {
    pub algorithm: DigestAlgorithm,
    pub value: NonEmptyString,
    pub canonicalization: NonEmptyString,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactProducedBy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<ActAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_ref: Option<Reference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.artifact.v1")]
pub struct Artifact {
    pub schema: ArtifactSchema,
    pub artifact_id: NonEmptyString,
    pub artifact_ref: Reference,
    pub produced_by: ArtifactProducedBy,
    pub media_type: NonEmptyString,
    pub created_at: IsoDateTime,
    pub size_bytes: u64,
    pub digest: DigestCommitment,
    #[serde(default)]
    pub redaction_refs: Vec<Reference>,
    #[serde(default)]
    pub source_refs: Vec<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
}
