//! Authority supplied by a trusted run host, separate from skill inputs and
//! context. A direct run has none, unless its operator limits what it spends;
//! an acting host names its actor, the hosted grants its calls run under, the
//! attenuation requests to mint against live capability evidence, what the
//! run may change and where its tools may go, how far the run's child runs
//! may spread and what the run may spend. The run record binds it: a resumed
//! run keeps the authority it started with.

use std::collections::BTreeMap;

use runx_contracts::connector::ConnectorName;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{Effects, Reach, Reference};
use runx_core::policy::AttenuationRequest;
use runx_parser::{Workflow, WorkflowStep};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// What the run may change, and whether a change waits: `none` refuses
    /// every change, `ask` holds every write with no approval of its own for
    /// a person, and `allow`, the default, holds nothing. Its methods and
    /// steps only narrow it.
    #[serde(default, skip_serializing_if = "runx_contracts::schema::is_default")]
    pub effects: Effects,
    /// Where the run's native file, git, command and authoring tools may go:
    /// `workspace` keeps them inside the workspace and off every secret file
    /// and runx's own state, and `inputs`, the default, reaches the roots the
    /// run's inputs name. Its methods and steps only narrow it.
    #[serde(default, skip_serializing_if = "runx_contracts::schema::is_default")]
    pub reach: Reach,
    /// The person's standing approvals of the writes the run's floor holds
    /// under `effects: ask`; a host sets the first set when it starts the
    /// run, and the person replaces it through `runx resume`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub standing_approvals: Vec<runx_contracts::StandingApproval>,
    /// The most the run, child runs included, may spend, in integer micro-USD
    /// (1 = $0.000001): every priced call's quote and every USD or USDC
    /// amount a call moves. Absent, the run is unbounded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spend_micro_usd: Option<u64>,
}

impl RunAuthority {
    /// What a run of `method` holds under `authority`: its effects and reach,
    /// narrowed by the method's own.
    #[must_use]
    pub(crate) fn of_method(authority: Option<Self>, method: &Workflow) -> Option<Self> {
        Self::narrowed(authority, method.effects, method.reach)
    }

    /// What the child run `step` starts holds under `authority`: its effects
    /// and reach, narrowed by the step's own.
    #[must_use]
    pub(crate) fn of_step(authority: Option<Self>, step: &WorkflowStep) -> Option<Self> {
        Self::narrowed(authority, step.effects, step.reach)
    }

    /// `authority` with its effects and reach narrowed where `effects` and
    /// `reach` are narrower, never wider. A run no host gave an authority
    /// gains one only when something narrows it.
    fn narrowed(
        authority: Option<Self>,
        effects: Option<Effects>,
        reach: Option<Reach>,
    ) -> Option<Self> {
        let stated = authority.is_some();
        let mut authority = authority.unwrap_or_default();
        authority.effects = authority.effects.narrowed(effects);
        authority.reach = authority.reach.narrowed(reach);
        (stated || authority != Self::default()).then_some(authority)
    }
}

/// What `step` may change under the run's `authority`: the run's effects,
/// narrowed by the step's own. A run's authority already holds its method's.
#[must_use]
pub(crate) fn step_effects(authority: Option<&RunAuthority>, step: &WorkflowStep) -> Effects {
    authority
        .map_or_else(Effects::default, |authority| authority.effects)
        .narrowed(step.effects)
}

/// Where `step`'s tools may go under the run's `authority`: the run's reach,
/// narrowed by the step's own. A run's authority already holds its method's.
#[must_use]
pub(crate) fn step_reach(authority: Option<&RunAuthority>, step: &WorkflowStep) -> Reach {
    authority
        .map_or_else(Reach::default, |authority| authority.reach)
        .narrowed(step.reach)
}

#[cfg(test)]
mod tests {
    use runx_contracts::{Effects, Reach};

    use super::{RunAuthority, step_effects, step_reach};

    fn method(effects: Option<Effects>, reach: Option<Reach>) -> runx_parser::Workflow {
        runx_parser::Workflow {
            name: "narrow".to_owned(),
            result_from: Vec::new(),
            outputs: None,
            effects,
            reach,
            steps: Vec::new(),
        }
    }

    fn step(effects: Option<Effects>, reach: Option<Reach>) -> runx_parser::WorkflowStep {
        runx_parser::WorkflowStep {
            id: "step".to_owned(),
            kind: runx_parser::StepKind::Ask(runx_parser::AskGate {
                gate: "step".to_owned(),
                reason: "narrow".to_owned(),
                gate_type: None,
            }),
            artifacts: None,
            outputs: None,
            inputs: runx_contracts::JsonObject::new(),
            bindings: Vec::new(),
            act: None,
            context_skills: Vec::new(),
            when: None,
            guard: Vec::new(),
            retry_unknown_outcomes: false,
            effects,
            reach,
        }
    }

    fn host(effects: Effects, reach: Reach) -> RunAuthority {
        RunAuthority {
            effects,
            reach,
            ..RunAuthority::default()
        }
    }

    #[test]
    fn what_applies_is_the_narrowest_of_host_method_and_step() {
        let asked = host(Effects::Ask, Reach::Workspace);
        // A method narrows its host.
        let narrowed =
            RunAuthority::of_method(Some(asked.clone()), &method(Some(Effects::None), None));
        assert_eq!(
            narrowed
                .as_ref()
                .map(|authority| (authority.effects, authority.reach)),
            Some((Effects::None, Reach::Workspace))
        );
        // A value wider than the host's narrows nothing.
        let widened = RunAuthority::of_method(
            Some(asked.clone()),
            &method(Some(Effects::Allow), Some(Reach::Inputs)),
        );
        assert_eq!(widened, Some(asked.clone()));
        // A step narrows its method's run, and never widens it.
        assert_eq!(
            step_effects(narrowed.as_ref(), &step(Some(Effects::Allow), None)),
            Effects::None
        );
        assert_eq!(
            step_effects(Some(&asked), &step(Some(Effects::None), None)),
            Effects::None
        );
        assert_eq!(
            step_reach(Some(&asked), &step(None, Some(Reach::Inputs))),
            Reach::Workspace
        );
        // A child run holds its parent step's narrowing.
        let child = RunAuthority::of_step(Some(asked), &step(Some(Effects::None), None));
        assert_eq!(
            child.map(|authority| authority.effects),
            Some(Effects::None)
        );
    }

    #[test]
    fn with_no_host_a_run_allows_and_reaches_its_inputs_until_something_narrows_it() {
        assert_eq!(step_effects(None, &step(None, None)), Effects::Allow);
        assert_eq!(step_reach(None, &step(None, None)), Reach::Inputs);
        assert_eq!(
            step_effects(None, &step(Some(Effects::Ask), None)),
            Effects::Ask
        );
        assert_eq!(RunAuthority::of_method(None, &method(None, None)), None);
        assert_eq!(
            RunAuthority::of_method(None, &method(Some(Effects::Allow), Some(Reach::Inputs))),
            None,
            "the defaults narrow nothing"
        );
        assert_eq!(
            RunAuthority::of_method(None, &method(Some(Effects::Ask), None))
                .map(|authority| (authority.effects, authority.reach)),
            Some((Effects::Ask, Reach::Inputs))
        );
        // An authority the host stated, even an empty one, stays stated.
        assert_eq!(
            RunAuthority::of_method(Some(RunAuthority::default()), &method(None, None)),
            Some(RunAuthority::default())
        );
    }

    #[test]
    fn a_run_authority_reads_effects_and_reach_and_leaves_their_defaults_off_the_wire()
    -> Result<(), serde_json::Error> {
        let authority: RunAuthority =
            serde_json::from_value(serde_json::json!({"effects": "ask", "reach": "workspace"}))?;
        assert_eq!(
            (authority.effects, authority.reach),
            (Effects::Ask, Reach::Workspace)
        );
        assert_eq!(
            serde_json::to_value(RunAuthority::default())?,
            serde_json::json!({}),
            "the defaults are left off the wire"
        );
        assert!(
            serde_json::from_value::<RunAuthority>(serde_json::json!({"effects": "free"})).is_err()
        );
        Ok(())
    }
}
