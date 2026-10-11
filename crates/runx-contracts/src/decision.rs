//! Decision contracts: choices and closure dispositions.
use serde::{Deserialize, Serialize};

use crate::schema::{NonEmptyString, RunxSchema};

/// The decisions a governed call can carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecisionChoice {
    Continue,
    Escalate,
    Defer,
    Close,
    Decline,
    Monitor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClosureDisposition {
    Closed,
    Deferred,
    Superseded,
    Declined,
    Blocked,
    Failed,
    Killed,
    TimedOut,
}

impl ClosureDisposition {
    /// Stable string label, identical to the serde `snake_case` representation.
    /// Single source of truth for the runtime's disposition strings.
    pub fn label(&self) -> &'static str {
        match self {
            ClosureDisposition::Closed => "closed",
            ClosureDisposition::Deferred => "deferred",
            ClosureDisposition::Superseded => "superseded",
            ClosureDisposition::Declined => "declined",
            ClosureDisposition::Blocked => "blocked",
            ClosureDisposition::Failed => "failed",
            ClosureDisposition::Killed => "killed",
            ClosureDisposition::TimedOut => "timed_out",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.decision.v1")]
pub struct Decision {
    pub decision_id: NonEmptyString,
    pub choice: DecisionChoice,
    /// The act the decision is carried out by. Its intent, target and
    /// artifacts are that act's.
    pub selected_act_id: NonEmptyString,
}
