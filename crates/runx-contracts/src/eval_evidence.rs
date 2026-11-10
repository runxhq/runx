//! `runx.eval_evidence.v1`: what a package's evals measured, and on what.
//! `skill-review#triggers` measures selection from `eval_queries.json`, and
//! `skill-review#evals` runs `evals/evals.json` and decides each attempt's
//! verdict. Either returns this, built from its own run's record, and the
//! registry reads it from the sealed receipt of the run that measured it.
//!
//! Its numbers are per host, as each turn's answer records its host, and
//! never blended: one run's turns can each be answered by a different host.
//! A turn's model is scripted or real by its provider: the registry counts no
//! evidence a `scripted` provider gave. The run that measured is live by its
//! origin, so the evidence states no providers.

use serde::{Deserialize, Serialize};

use crate::run_read::{RanOn, RunImplementation, TurnHost};
use crate::schema::{NonEmptyString, RunxSchema};
use crate::verdict::{Evaluator, Pinned, VerdictDecision};

pub const EVAL_EVIDENCE_SCHEMA: &str = "runx.eval_evidence.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum EvalEvidenceSchema {
    #[serde(rename = "runx.eval_evidence.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.eval_evidence.v1")]
pub struct EvalEvidence {
    pub schema: EvalEvidenceSchema,
    /// The package measured, at the digest it was measured at.
    pub package: Pinned,
    /// Where the run that measured ran, as it was started.
    pub ran_on: RanOn,
    /// What started the run that measured.
    pub implementation: RunImplementation,
    /// How many times each query or eval was worked.
    pub repeats: u32,
    /// What was measured, one entry per host.
    pub hosts: Vec<HostEvidence>,
}

/// What one host's turns measured.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct HostEvidence {
    /// The host that answered these turns, as their answers record it; none
    /// when they record none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<TurnHost>,
    /// The models these turns stated.
    pub models: Vec<NonEmptyString>,
    pub results: EvalResults,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvalResults {
    /// How often the host selected the package for each query: a query
    /// passes when it was selected in at least half its runs and should
    /// have been, or in fewer than half and shouldn't.
    Selection {
        queries: Vec<QuerySelection>,
        passed: u32,
        should_trigger: Tally,
        should_not_trigger: Tally,
    },
    /// Each eval's attempts and how each verdict was decided.
    Evals {
        evals: Vec<EvalAttempts>,
        met: u32,
        not_met: u32,
        inconclusive: u32,
        refused: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct QuerySelection {
    pub query: NonEmptyString,
    pub should_trigger: bool,
    /// The runs whose turn loaded the package, by its digest.
    pub selected: u32,
    pub runs: u32,
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Tally {
    pub queries: u32,
    pub passed: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct EvalAttempts {
    pub id: u32,
    /// The criteria set the eval's verdicts answered.
    pub criteria_set: Pinned,
    pub attempts: Vec<EvalAttempt>,
}

/// One attempt: how its verdict was decided, who judged it, and the receipt
/// its run sealed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct EvalAttempt {
    pub decision: VerdictDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator: Option<Evaluator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<NonEmptyString>,
}
