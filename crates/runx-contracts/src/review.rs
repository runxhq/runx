//! Review-receipt output contract: the diagnosis the managed reviewer produces
//! for the diagnose-skill-run skill, consumed by skill-build improvement runs.
//!
//! Identity is the legacy bare `runx.ai/schemas` `$id` (no `x-runx-schema`,
//! no `schema` discriminant). The document is open (`additionalProperties:
//! true`).
use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReceiptVerdict {
    Pass,
    NeedsUpdate,
    Blocked,
}

/// A bounded improvement proposal. Open (`additionalProperties: true`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub struct ReviewReceiptImprovementProposal {
    pub target: String,
    pub change: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[runx_schema(id = "runx.review_receipt_output.v1")]
pub struct ReviewReceiptOutput {
    pub verdict: ReviewReceiptVerdict,
    pub failure_summary: String,
    pub improvement_proposals: Vec<ReviewReceiptImprovementProposal>,
    pub next_harness_checks: Vec<String>,
}
