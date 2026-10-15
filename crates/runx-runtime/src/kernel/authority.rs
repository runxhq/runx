//! Authority supplied by a trusted run host, separate from skill inputs and
//! context. A direct run has none; an acting host names its actor, the
//! attenuation requests to mint against live capability evidence, and how
//! far the run's child runs may spread. The run record binds it: a resumed
//! run keeps the authority it started with.

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delegations: Vec<AttenuationRequest>,
    /// The most child runs one agent turn may start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fanout: Option<u32>,
    /// How many child runs deep the run may nest below its root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_child_depth: Option<u32>,
    /// The run is worked by a host's agent, so runx bounds it: its native
    /// file, git, command and authoring tools stay inside the workspace, and
    /// it never reads or writes a secret file or runx's own state. An
    /// operator's run leaves it unset and reaches the roots its inputs name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_bounds: bool,
    /// The person's standing approvals of the local effects the run's floor
    /// holds when it sets `agent_bounds`; a host sets the first set when it
    /// starts the run, and the person replaces it through `runx resume`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub standing_approvals: Vec<runx_contracts::StandingApproval>,
}
