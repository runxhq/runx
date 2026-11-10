//! Authority supplied by a trusted run host, separate from skill inputs and
//! context. A direct run has none, unless its operator limits what it spends;
//! a host names the hosted grants its calls run under, the delegations to
//! mint against live capability evidence, the packages its calls may be made
//! from, its actor when it delegates to one, what the run may change and
//! where its tools may go, how far the run's child runs may spread and what
//! the run may spend. The run record binds it: a resumed run keeps the
//! authority it started with.

use std::collections::BTreeMap;

use runx_contracts::connector::ConnectorName;
use runx_contracts::schema::{IsoDateTime, NonEmptyString, NonEmptyVec};
use runx_contracts::{AuthorityBounds, AuthorityVerb, Effects, Reach, Reference};
use runx_parser::{Workflow, WorkflowStep};
use serde::{Deserialize, Serialize};

/// A run's `max_concurrency`: any positive integer. The ceiling it's
/// narrowed to is the host's resource policy
/// ([`RuntimeOptions::max_concurrency`](crate::RuntimeOptions)), not the
/// type's.
pub type MaxConcurrency = std::num::NonZeroU32;

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
    /// Who acts, when it isn't the instance's principal. Absent, each call is
    /// made by its instance's principal, and the authority only bounds the
    /// run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_ref: Option<Reference>,
    /// The operator's mandate binding a delegation to another actor. A call
    /// made under such a delegation requires it, and its receipt cites it
    /// rather than the connector instance as if the instance had delegated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandate_ref: Option<Reference>,
    /// The host's narrowings, at most one per connector instance. The kernel
    /// mints each term from fresh capability evidence at admission; model-
    /// or skill-supplied terms are never accepted as authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delegations: Vec<Delegation>,
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
    /// The packages every call's chain must be covered by, on whichever
    /// instance the call resolves (D31): a plain narrowing of its
    /// capability. Absent, any package may call; an empty list covers none.
    /// A package is listed once.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "runx_contracts::schema::deserialize_some_unique_items"
    )]
    pub packages: Option<Vec<runx_contracts::PackagePattern>>,
    /// The most calls the run, its steps and child runs included, may have
    /// executing at once: a positive integer, narrowed to the host's ceiling,
    /// which applies when it's absent. A step's `concurrency` only narrows
    /// it, so nesting can't multiply it (§4.11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<MaxConcurrency>,
}

/// A host's narrowing of one connector instance for a run (authority.md): one
/// or more terms, each its own verbs and bounds, so it can say "read the
/// person's words, write observations". Each term narrows a term of the link
/// before it, the first it fits in the host's order, and a call on the
/// instance is admitted only when one of its terms covers it.
///
/// A plain narrowing names nobody. A host delegating to a different actor
/// names it as `principal_ref`, and the run names it as `actor_ref` under the
/// operator's `mandate_ref`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delegation {
    /// The connector whose instance it narrows, by name: nobody writes a
    /// hashed instance id.
    pub connector: ConnectorName,
    /// The instance `connector` resolved to at the root's admission, which
    /// the run records and keeps. A host never states it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_ref: Option<Reference>,
    /// The different actor the host delegates to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_ref: Option<Reference>,
    /// When its terms expire, no later than what each narrows. Without one,
    /// each expires with the term it narrows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<IsoDateTime>,
    pub terms: NonEmptyVec<DelegationTerm>,
}

/// One term of a delegation: its verbs over its bounds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationTerm {
    pub verbs: Vec<AuthorityVerb>,
    pub bounds: AuthorityBounds,
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

/// The most calls a run under `authority` may have executing at once on a
/// host whose ceiling is `host`: the authority's `max_concurrency`, narrowed
/// to the host's ceiling, which applies when it states none.
#[must_use]
pub fn run_concurrency(authority: Option<&RunAuthority>, host: MaxConcurrency) -> u32 {
    authority
        .and_then(|authority| authority.max_concurrency)
        .map_or(host, |limit| limit.min(host))
        .get()
}

/// How many of a step's calls may execute at once: the `concurrency` it
/// declares, narrowed by the run's `authority` and the host's ceiling,
/// `host`. A step that runs calls at once, such as a script, reads both from
/// its runtime's options.
#[must_use]
pub fn step_concurrency(
    authority: Option<&RunAuthority>,
    host: MaxConcurrency,
    declared: u32,
) -> u32 {
    declared.min(run_concurrency(authority, host))
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
                reason: runx_parser::AskReason::Text("narrow".to_owned()),
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

    /// A delegation names its connector and holds one or more terms, each its
    /// own verbs and bounds (authority.md), and a run's packages never repeat.
    #[test]
    fn a_delegation_names_its_connector_and_one_or_more_terms() -> Result<(), String> {
        let read = |value: serde_json::Value| serde_json::from_value::<RunAuthority>(value);
        let term = |verbs: &[&str], targets: &[&str]| {
            serde_json::json!({
                "verbs": verbs,
                "bounds": {
                    "operations": ["private/*"],
                    "targets": targets,
                    "packages": ["runx/agent", "runx/memory"],
                },
            })
        };
        let memory = read(serde_json::json!({"delegations": [{
            "connector": "private",
            "expires_at": "2026-12-01T00:00:00Z",
            "terms": [
                term(&["read"], &["runx/memory/said:*"]),
                term(&["read", "write"], &["runx/memory/observed:*"]),
            ],
        }]}))
        .map_err(|error| error.to_string())?;
        let delegation = memory.delegations.first().ok_or("one delegation")?;
        assert_eq!(delegation.connector.as_str(), "private");
        assert_eq!(delegation.terms.len(), 2);
        assert!(
            delegation.principal_ref.is_none(),
            "a plain narrowing names nobody"
        );
        assert!(
            delegation.resource_ref.is_none(),
            "root admission resolves the instance"
        );
        assert!(
            read(serde_json::json!({"delegations": [{"connector": "private", "terms": []}]}))
                .is_err(),
            "a delegation holds at least one term"
        );
        assert!(
            read(serde_json::json!({"delegations": [{
                "connector": "private",
                "verbs": ["read"],
                "bounds": {"operations": ["private/*"]},
            }]}))
            .is_err(),
            "verbs and bounds belong to a term"
        );
        assert!(
            read(serde_json::json!({"delegations": [{
                "connector": "private",
                "terms": [term(&["read"], &["*"])],
                "resource_family": "connector",
            }]}))
            .is_err()
        );
        assert!(
            read(serde_json::json!({"packages": ["acme/counter", "acme/counter"]})).is_err(),
            "a package named twice is refused"
        );
        Ok(())
    }

    /// `max_concurrency` is any positive integer: the ceiling it's narrowed
    /// to is the host's resource policy, not the type's. What runs at once is
    /// the least of a step's `concurrency`, the authority's limit and the
    /// host's ceiling, and a child run keeps its parent's (authority.md, §4.11).
    #[test]
    fn max_concurrency_is_narrowed_to_the_hosts_ceiling_and_a_step_only_narrows_it()
    -> Result<(), serde_json::Error> {
        use super::{run_concurrency, step_concurrency};
        use crate::execution::engine::DEFAULT_MAX_CONCURRENCY;
        let read = |value: serde_json::Value| serde_json::from_value::<RunAuthority>(value);
        let host =
            |ceiling: u32| std::num::NonZeroU32::new(ceiling).unwrap_or(DEFAULT_MAX_CONCURRENCY);
        let sixty_four = read(serde_json::json!({"max_concurrency": 64}))?;
        assert!(
            read(serde_json::json!({"max_concurrency": 0})).is_err(),
            "0 runs nothing"
        );
        assert!(read(serde_json::json!({"max_concurrency": -1})).is_err());
        // 64 under a host policy of 32 runs at most 32 at once.
        assert_eq!(run_concurrency(Some(&sixty_four), host(32)), 32);
        assert_eq!(step_concurrency(Some(&sixty_four), host(32), 100), 32);
        // With no policy, the CLI's conservative default.
        assert_eq!(DEFAULT_MAX_CONCURRENCY.get(), 16);
        assert_eq!(
            run_concurrency(Some(&sixty_four), DEFAULT_MAX_CONCURRENCY),
            16
        );
        assert_eq!(run_concurrency(None, DEFAULT_MAX_CONCURRENCY), 16);
        // A narrower authority, or step, narrows further.
        let two = read(serde_json::json!({"max_concurrency": 2}))?;
        assert_eq!(step_concurrency(Some(&two), host(32), 4), 2);
        assert_eq!(step_concurrency(Some(&two), host(32), 1), 1);
        assert_eq!(step_concurrency(None, host(32), 4), 4);
        let child = RunAuthority::of_step(Some(two.clone()), &step(None, None));
        assert_eq!(run_concurrency(child.as_ref(), host(32)), 2);
        assert_eq!(
            serde_json::to_value(&two)?,
            serde_json::json!({"max_concurrency": 2})
        );
        Ok(())
    }

    /// The CLI host reads its ceiling from `RUNX_MAX_CONCURRENCY`, a positive
    /// integer, and refuses anything else; absent, the conservative default.
    #[test]
    fn the_host_reads_its_ceiling_from_its_environment() {
        use crate::execution::engine::{DEFAULT_MAX_CONCURRENCY, host_max_concurrency};
        let env = |value: Option<&str>| {
            value
                .map(|value| ("RUNX_MAX_CONCURRENCY".to_owned(), value.to_owned()))
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        assert_eq!(
            host_max_concurrency(&env(None)),
            Ok(DEFAULT_MAX_CONCURRENCY)
        );
        assert_eq!(
            host_max_concurrency(&env(Some("32"))).map(std::num::NonZeroU32::get),
            Ok(32)
        );
        for refused in ["0", "x", "-1", "", "1.5", " 4"] {
            let refusal = host_max_concurrency(&env(Some(refused)))
                .err()
                .unwrap_or_default();
            assert!(
                refusal.contains("RUNX_MAX_CONCURRENCY"),
                "{refused:?}: {refusal}"
            );
        }
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
