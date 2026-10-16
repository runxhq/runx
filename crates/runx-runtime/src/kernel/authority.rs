//! Authority supplied by a trusted run host, separate from skill inputs and
//! context. A direct run has none, unless its operator bounds what it spends;
//! an acting host names its actor, the hosted grants its calls run under, the
//! attenuation requests to mint against live capability evidence, how far the
//! run's child runs may spread and what the run may spend. The run record
//! binds it: a resumed run keeps the authority it started with.

use std::collections::BTreeMap;

use runx_contracts::Reference;
use runx_contracts::connector::ConnectorName;
use runx_contracts::schema::NonEmptyString;
use runx_core::policy::AttenuationRequest;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunAuthority {
    /// The hosted grant each connector's calls run under, by connector: the
    /// host's own statement of the run's instances. A stated grant stands for
    /// its connector's hosted placement and wins over the operator's
    /// selection; admission reads the grant itself through the host, so a
    /// host names a grant and never asserts what it covers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub grants: BTreeMap<ConnectorName, NonEmptyString>,
    /// Who acts. Absent, each call is made by its instance's principal, and
    /// the authority only bounds the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_ref: Option<Reference>,
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
    /// The most the run, child runs included, may spend, in integer micro-USD
    /// (1 = $0.000001): every priced call's quote and every USD or USDC
    /// amount a call moves. Absent, the run is unbounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_micro: Option<u64>,
}
