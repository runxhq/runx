//! What the native artifact tools return.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.local_artifact.v1",
    url = "https://schemas.runx.ai/runx/local-artifact/v1.json"
)]
pub struct LocalArtifact {
    pub artifact_uri: String,
    pub media_type: String,
    pub bytes: u64,
    pub whole_digest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.local_artifact.page.v1",
    url = "https://schemas.runx.ai/runx/local-artifact/page/v1.json"
)]
pub struct LocalArtifactPage {
    pub artifact_uri: String,
    pub media_type: String,
    pub offset: u64,
    pub length: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub range_digest: String,
    pub whole_digest: String,
    pub encoding: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<String>,
}
