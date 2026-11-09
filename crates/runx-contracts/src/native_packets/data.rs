//! What the native data tools return.

use serde::{Deserialize, Serialize};

use crate::digest::DigestAlgorithm;
use crate::schema::RunxSchema;
use crate::Digest;

/// What `data.digest` returns: the SHA-256 digest of canonical JSON or of
/// exact UTF-8 text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.data.digest.v1")]
pub struct DataDigest {
    pub algorithm: DigestAlgorithm,
    pub digest: Digest,
}

/// What `data.compare` returns: whether two resolved values are equal, and
/// the digest of each, without copying either into the output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.data.comparison.v1")]
pub struct DataComparison {
    pub equal: bool,
    pub actual_digest: Digest,
    pub expected_digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.data.validation.v1")]
pub struct DataValidation {
    pub valid: bool,
    pub schema_digest: String,
    pub values_digest: String,
    pub results: Vec<DataValueValidation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct DataValueValidation {
    pub index: usize,
    pub valid: bool,
    pub errors: Vec<DataValidationIssue>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct DataValidationIssue {
    pub instance_path: String,
    pub schema_path: String,
}
