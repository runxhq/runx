//! Skill maturity: how ready a *skill* is, and what that rests on.
//!
//! Maturity is orthogonal to trust, which describes the *author*. The registry
//! computes the tier from the tests it runs when a version is published and
//! stores it with its basis on the record; readers never recompute it. Eval
//! evidence (`runx.eval_evidence.v1`) shows beside it and decides nothing in
//! 0.12.

use serde::{Deserialize, Serialize};

use crate::eval_evidence::EvalEvidenceKind;
use crate::run_read::TurnHost;
use crate::schema::NonEmptyString;

/// How ready a skill is, what its tier rests on, and the eval evidence
/// shown beside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Maturity {
    pub tier: MaturityTier,
    pub basis: MaturityBasis,
    /// The eval evidence of the version, by the receipt of each run that
    /// measured it, one entry per host. Never required.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evals: Vec<MaturityEval>,
}

/// How ready a skill is. `Alpha` is the floor for any published skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaturityTier {
    /// Published; test incomplete or not all declared cases pass.
    Alpha,
    /// Every declared test case passes.
    Beta,
    /// Every declared case passes and at least one passing case proves the
    /// skill runs inside a workflow.
    Stable,
}

/// What a tier rests on: the version's tests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaturityBasis {
    pub tests: TestBasis,
}

/// The tests the tier was computed from, as they were run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestBasis {
    /// The declared cases.
    pub cases: u32,
    /// The cases that run the skill inside a workflow.
    pub workflow_cases: u32,
    /// Whether every declared case passed.
    pub passed: bool,
    /// Who ran them.
    pub ran_by: TestedBy,
    /// The runx that ran them.
    pub runx_version: NonEmptyString,
}

/// Who ran a version's tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestedBy {
    /// The registry, when the version was published.
    Registry,
}

/// One host's numbers from one run's eval evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaturityEval {
    /// The receipt of the run that measured, which binds its evidence.
    pub receipt_id: NonEmptyString,
    pub kind: EvalEvidenceKind,
    /// The host that answered its turns; none when they record none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<TurnHost>,
    /// The models its turns stated.
    pub models: Vec<NonEmptyString>,
    /// The queries that passed, or the attempts whose verdict was met.
    pub passed: u32,
    /// The queries measured, or the attempts made.
    pub total: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_states_maturity_as_a_tier_with_its_basis_and_never_a_bare_tier()
    -> Result<(), serde_json::Error> {
        let stated = serde_json::json!({
            "tier": "stable",
            "basis": {
                "tests": {
                    "cases": 4,
                    "workflow_cases": 1,
                    "passed": true,
                    "ran_by": "registry",
                    "runx_version": "0.12.0"
                }
            },
            "evals": [{
                "receipt_id": "rx_1",
                "kind": "selection",
                "host": {"kind": "mcp_client", "name": "claude-code", "version": "2.1.293"},
                "models": ["claude-sonnet-4-6"],
                "passed": 9,
                "total": 10
            }]
        });
        let maturity: Maturity = serde_json::from_value(stated.clone())?;
        assert_eq!(maturity.tier, MaturityTier::Stable);
        assert_eq!(serde_json::to_value(&maturity)?, stated);

        assert!(serde_json::from_value::<Maturity>(serde_json::json!("alpha")).is_err());
        Ok(())
    }
}
