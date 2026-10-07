//! Authority supplied by a trusted run host, separate from skill inputs and
//! context. A direct run has no delegation; an acting host names its actor and
//! the attenuation requests to mint against live capability evidence.

use runx_contracts::Reference;
use runx_core::policy::AttenuationRequest;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunAuthority {
    pub actor_ref: Reference,
    /// The trusted host's operator-authored mandate binding. Delegated calls
    /// require it, and their receipts cite it rather than the connector
    /// instance as if the instance had delegated authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandate_ref: Option<Reference>,
    /// Host-supplied narrowing requests in parent-to-child order. The kernel
    /// mints each term from fresh capability evidence at admission; model-
    /// or skill-supplied terms are never accepted as authority.
    pub delegations: Vec<AttenuationRequest>,
}
