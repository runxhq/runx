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

impl RunAuthority {
    /// How far the run's delegations let it spread: the most child runs one
    /// turn may start, and how deep child runs may nest. The tightest bound
    /// any delegation sets holds; `None` where none sets one.
    #[must_use]
    pub fn child_limits(&self) -> ChildLimits {
        let tightest = |bound: fn(&AttenuationRequest) -> Option<u32>| {
            self.delegations.iter().filter_map(bound).min()
        };
        ChildLimits {
            fanout: tightest(|request| request.bounds.max_fanout),
            depth: tightest(|request| request.bounds.max_child_depth),
        }
    }
}

/// The bounds a run's delegations set on the child runs it starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChildLimits {
    /// The most child runs one turn may start.
    pub fanout: Option<u32>,
    /// How many child runs deep a run may nest below its root.
    pub depth: Option<u32>,
}

#[cfg(test)]
mod tests {
    use runx_contracts::{AuthorityBounds, AuthorityResourceFamily, Reference, ReferenceType};
    use runx_core::policy::AttenuationRequest;

    use super::{ChildLimits, RunAuthority};

    fn delegation(max_fanout: Option<u32>, max_child_depth: Option<u32>) -> AttenuationRequest {
        AttenuationRequest {
            principal_ref: Reference::runx(ReferenceType::Principal, "actor"),
            resource_ref: Reference::runx(ReferenceType::ConnectorInstance, "any"),
            resource_family: AuthorityResourceFamily::Connector,
            verbs: Vec::new(),
            capabilities: Vec::new(),
            bounds: AuthorityBounds {
                max_fanout,
                max_child_depth,
                ..AuthorityBounds::default()
            },
            expires_at: None,
        }
    }

    #[test]
    fn the_tightest_delegation_bounds_the_child_runs() {
        let authority = RunAuthority {
            actor_ref: Reference::runx(ReferenceType::Principal, "actor"),
            mandate_ref: None,
            delegations: vec![delegation(Some(3), None), delegation(Some(1), Some(2))],
        };
        assert_eq!(
            authority.child_limits(),
            ChildLimits {
                fanout: Some(1),
                depth: Some(2),
            }
        );
    }
}
