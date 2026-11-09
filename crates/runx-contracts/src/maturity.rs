//! Skill maturity tier.
//!
//! Maturity describes how ready a *skill* is (tests, graph integration). It is
//! orthogonal to trust, which describes the *author*. The registry computes
//! the tier from test signals when a version is published and stores it on
//! the record; readers never recompute it.

use serde::{Deserialize, Serialize};

/// How ready a skill is. `Alpha` is the floor for any published skill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaturityTier {
    /// Published; test incomplete or not all declared cases pass.
    #[default]
    Alpha,
    /// Every declared test case passes.
    Beta,
    /// Every declared case passes and at least one passing case proves the
    /// skill runs inside a graph.
    Stable,
}
