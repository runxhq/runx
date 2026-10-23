//! Provider-neutral observations from an operator-selected human decision
//! adapter. The native pending request remains the authority; this packet is
//! evidence that the bound adapter saw a human response to that request.

use serde::{Deserialize, Serialize};

use crate::schema::RunxSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[runx_schema(id = "runx.decision_observation.v1")]
pub enum DecisionObservation {
    Pending {
        request_digest: String,
        nonce: String,
        coverage_complete: bool,
        next_cursor: Option<String>,
    },
    Decided {
        request_digest: String,
        nonce: String,
        presentation_locator: String,
        actor_id: String,
        origin: DecisionOrigin,
        evidence_locator: String,
        occurred_at: String,
        approved: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOrigin {
    HumanAccount,
    AppOrBot,
    Unknown,
}
