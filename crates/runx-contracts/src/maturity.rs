//! Skill maturity tier and the signals it is computed from.
//!
//! Maturity describes how ready a *skill* is (tests, graph integration). It is
//! orthogonal to trust, which describes the *author*. The tier is computed from
//! harness signals at event points (publish, harness seal) and stored on the
//! registry record; readers never recompute it.

use serde::{Deserialize, Serialize};

/// How ready a skill is. `Alpha` is the floor for any published skill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaturityTier {
    /// Published; harness incomplete or not all declared cases pass.
    #[default]
    Alpha,
    /// Every declared harness case passes.
    Beta,
    /// Every declared case passes and at least one passing case proves the
    /// skill runs inside a graph.
    Stable,
}

/// The harness-derived inputs to the maturity decision. Callers extract these
/// at an event point; the decision itself stays pure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaturitySignals {
    /// Number of harness cases the skill declares.
    pub declared_case_count: usize,
    /// Every declared case ran and passed.
    pub all_declared_cases_passed: bool,
    /// At least one passing case targets a graph that includes this skill.
    pub has_passing_graph_case: bool,
}

impl MaturityTier {
    /// The tier a skill's harness evidence earns: nothing passing is alpha,
    /// every declared case passing is beta, and stable adds a passing case
    /// that runs the skill inside a graph.
    #[must_use]
    pub fn from_signals(signals: MaturitySignals) -> Self {
        if signals.declared_case_count == 0 || !signals.all_declared_cases_passed {
            Self::Alpha
        } else if signals.has_passing_graph_case {
            Self::Stable
        } else {
            Self::Beta
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_evidence_decides_the_tier() {
        let signals = |declared_case_count, all_declared_cases_passed, has_passing_graph_case| {
            MaturitySignals {
                declared_case_count,
                all_declared_cases_passed,
                has_passing_graph_case,
            }
        };
        assert_eq!(
            MaturityTier::from_signals(signals(0, true, true)),
            MaturityTier::Alpha
        );
        assert_eq!(
            MaturityTier::from_signals(signals(3, false, true)),
            MaturityTier::Alpha
        );
        assert_eq!(
            MaturityTier::from_signals(signals(3, true, false)),
            MaturityTier::Beta
        );
        assert_eq!(
            MaturityTier::from_signals(signals(3, true, true)),
            MaturityTier::Stable
        );
    }
}
