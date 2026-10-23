//! What the native receipt tools return.

use serde::{Deserialize, Serialize};

use crate::JsonObject;
use crate::schema::{BoundedVec, NonEmptyString, RunxSchema};

/// The most receipts one query details, or one proof is asked for.
pub const RECEIPT_TOOL_MAX_RECEIPTS: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ReceiptQuerySchema {
    #[serde(rename = "runx.receipt.query.v1")]
    V1,
}

/// Where a query read its receipts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptQuerySource {
    NativeReceiptStore,
}

/// What `receipt.query` returns: the receipts a filter matched in the native
/// receipt store, with detail and tree verification when asked for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.receipt.query.v1")]
pub struct ReceiptQuery {
    pub schema: ReceiptQuerySchema,
    pub source: ReceiptQuerySource,
    pub store_label: NonEmptyString,
    pub filter: JsonObject,
    pub receipt_ids: Vec<NonEmptyString>,
    pub receipts: Vec<JsonObject>,
    pub receipt_details: BoundedVec<JsonObject, 0, RECEIPT_TOOL_MAX_RECEIPTS>,
    pub verification: ReceiptQueryVerification,
}

/// A query's tree verification; `intact` is null when it was not asked for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptQueryVerification {
    pub checked: bool,
    pub intact: Option<bool>,
    pub trees: Vec<JsonObject>,
    pub findings: Vec<JsonObject>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ReceiptProofSchema {
    #[serde(rename = "runx.receipt.proof.v1")]
    V1,
}

/// Whether every requested receipt resolved and verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptProofDecision {
    Verified,
    NeedsMoreEvidence,
}

/// What `receipt.proof` returns: each requested receipt it found, its
/// redacted detail, and the verification of every tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.receipt.proof.v1")]
pub struct ReceiptProof {
    pub schema: ReceiptProofSchema,
    pub decision: ReceiptProofDecision,
    pub requested_receipt_ids: BoundedVec<NonEmptyString, 1, RECEIPT_TOOL_MAX_RECEIPTS>,
    pub matched_receipts: BoundedVec<MatchedReceipt, 0, RECEIPT_TOOL_MAX_RECEIPTS>,
    pub receipt_details: BoundedVec<JsonObject, 0, RECEIPT_TOOL_MAX_RECEIPTS>,
    pub verification: ReceiptProofVerification,
    pub store: ReceiptProofStore,
}

/// One requested receipt `receipt.proof` found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct MatchedReceipt {
    pub receipt_id: NonEmptyString,
    pub subject_uri: NonEmptyString,
    pub outcome: NonEmptyString,
    pub created_at: NonEmptyString,
    pub verification_status: NonEmptyString,
}

/// A proof's tree verification, which it always runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptProofVerification {
    pub checked: bool,
    pub intact: bool,
    pub trees: Vec<JsonObject>,
    pub findings: Vec<JsonObject>,
}

/// The receipt store a proof read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptProofStore {
    pub label: NonEmptyString,
}
