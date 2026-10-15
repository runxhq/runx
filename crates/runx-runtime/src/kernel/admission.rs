//! The admission evaluator: the one place that decides whether an operation
//! call may run. Every surface (the graph engine, `runx skill inspect`, the
//! MCP server, the assistant) reads its answer instead of re-deciding.
//!
//! An exact call gets an [`Admission`]: its checks run in order, each
//! refusing before the next, and an admitted call carries the execution term
//! derived from its capability and delegations, with one subset proof per
//! parent. Anything less exact gets a [`Readiness`] projection from the same
//! checks.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use runx_contracts::connector::{
    AmountRule, ApprovalFloor, ConnectorOperation, Digest, OperationAccess, OperationName,
    Placement, TemplateInputs, credential_declaration_digest, stable_digest,
};
use runx_contracts::schema::{IsoDateTime, NonEmptyString};
use runx_contracts::{
    AdmissionCheck, AdmissionReason, AuthorityApproval, AuthorityBounds, AuthorityDerivation,
    AuthorityEvidence, AuthorityEvidenceKind, AuthorityResourceFamily, AuthorityTerm,
    AuthorityUseLimit, CheckOutcome, ConnectorInstance, JsonObject, JsonValue, OperationIntent,
    Readiness, Reference, ReferenceType,
};
use runx_core::policy::{
    AttenuationRequest, ConnectorBoundsComparator, DerivationParent, ExecutionTermRequest,
    derive_execution_term, mint_attenuated,
};
use runx_parser::{ConnectorRequirement, DuplicatePolicy, OperationStepTarget};

use super::capability::{self, Capability, CapabilityRequest};
use super::connectors::{ConnectorScope, LoadedConnector};
use super::custody::{EffectCustody, Pending};
use super::instances::Instances;
use super::operation::call::{ApprovalText, OperationCall};
use super::operation::hosted::HostedClient;
use super::spend::{self, Charge, SpendError};
use super::store::KernelStore;
use super::usage::{UsageError, UsageLedger, UseCover};

/// Where admission reads connectors, instances and evidence from.
#[derive(Clone, Copy)]
pub(crate) struct Context<'a> {
    pub scope: &'a ConnectorScope,
    pub instances: &'a Instances,
    pub hosted: &'a HostedClient,
    pub env: &'a BTreeMap<String, String>,
    pub cwd: &'a Path,
}

/// What admission needs to evaluate one call.
pub(crate) struct Evaluation<'a> {
    pub step: &'a OperationStepTarget,
    pub call: &'a OperationCall,
    pub duplicates: Option<DuplicatePolicy>,
    /// The harness's provider fixture stands in for a declared operation's
    /// missing hosted implementation.
    pub fixture_stand_in: bool,
    pub run_authority: Option<&'a super::authority::RunAuthority>,
    /// Where the act's custody and its evidence uses are kept.
    pub store: &'a Arc<KernelStore>,
    /// The run the call belongs to: a read's custody, and the run a new use
    /// is attributed to.
    pub run_id: &'a str,
    pub context: Context<'a>,
}

/// Whether an admitted call runs now or waits on an approval. A call that
/// cannot be admitted is refused, never admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdmissionDisposition {
    Admit,
    /// A person must answer first: an approval floor or gate applies.
    NeedsResolution,
}

/// One call's admission: the intent it covers, every reason, and, once
/// admitted, the narrowest term that satisfies the evidence, bounded to this
/// intent and a single use, with its derivation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Admission {
    pub disposition: AdmissionDisposition,
    pub intent: OperationIntent,
    pub intent_digest: Digest,
    pub reasons: Vec<AdmissionReason>,
    pub execution_term: Option<AuthorityTerm>,
    pub derivation: Option<AuthorityDerivation>,
}

/// An exact call admitted, or owed an approval.
#[derive(Clone, Debug)]
pub(crate) struct Admitted {
    pub admission: Admission,
    pub connector: Arc<LoadedConnector>,
    pub operation: OperationName,
    pub instance: ConnectorInstance,
    pub capability: Capability,
    /// Who makes the call: the capability's principal for a direct call, the
    /// delegated actor under a run authority.
    pub actor_ref: Reference,
    pub delegations: Vec<AuthorityTerm>,
    pub mandate_ref: Option<Reference>,
    /// The act's custody, kept for a mutation or a bounded call.
    pub custody: Option<EffectCustody>,
    /// The act's unsettled attempt of this same intent, which this call
    /// recovers.
    pub pending: Option<Pending>,
    /// The custody stream's version when this call was admitted; its attempt
    /// appends onto exactly that.
    pub custody_head: u64,
    pub usage_covers: Vec<UseCover>,
    /// What the call spends against its run's money limit.
    pub spend: Option<Spending>,
    pub target: String,
    pub request_key: Option<String>,
    /// What a person is shown, when the call is gated.
    pub approval: Option<ApprovalText>,
    term_request: AttenuationRequest,
}

impl Admitted {
    #[must_use]
    pub(crate) fn operation(&self) -> &ConnectorOperation {
        &self.connector.manifest.operations[&self.operation]
    }

    pub(crate) fn intent(&self) -> &OperationIntent {
        &self.admission.intent
    }

    pub(crate) fn intent_digest(&self) -> &Digest {
        &self.admission.intent_digest
    }

    /// Derive the execution term with the approval evidence the call now
    /// holds, and admit it.
    pub(crate) fn admit_with(&mut self, approvals: Vec<AuthorityApproval>) -> Result<(), String> {
        if self.admission.disposition != AdmissionDisposition::NeedsResolution
            || approvals.is_empty()
        {
            return Err("the call has no pending resolution with approval evidence".to_owned());
        }
        let evidence = approvals
            .iter()
            .map(|approval| AuthorityEvidence {
                kind: AuthorityEvidenceKind::Approval,
                reference: approval.approval_ref.clone(),
                term: None,
            })
            .collect::<Vec<_>>();
        let (term, derivation) = derive(
            &self.capability,
            &self.delegations,
            self.mandate_ref.as_ref(),
            &self.term_request,
            self.admission.intent_digest.as_str(),
            approvals,
            evidence,
        )
        .map_err(|(_, message)| message)?;
        for reason in &mut self.admission.reasons {
            if reason.check == AdmissionCheck::Approval && reason.outcome == CheckOutcome::Required
            {
                reason.outcome = CheckOutcome::Pass;
                reason.detail = NonEmptyString::from("approval evidence is bound to this intent");
            }
        }
        self.admission.disposition = AdmissionDisposition::Admit;
        self.admission.execution_term = Some(term);
        self.admission.derivation = Some(derivation);
        Ok(())
    }
}

/// What an admitted call spends against its run's money limit. The limit
/// is checked again, and the spend reserved, in the commit that records the
/// call's attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Spending {
    pub limit: u64,
    pub charge: Charge,
    /// The run the call belongs to; it spends from its root's ledger.
    pub run_id: String,
    /// The attempt the spend is reserved for.
    pub spend_key: String,
}

/// Why an exact call cannot run.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AdmitError {
    /// A check refused it.
    Refused(Refusal),
    /// The kernel's state holds it: the act has an unsettled attempt the
    /// call cannot recover, or the state cannot be read.
    State(String),
}

/// A refusal: the reasons up to and including the check that failed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Refusal {
    pub check: AdmissionCheck,
    pub reasons: Vec<AdmissionReason>,
    /// The verb of the operation refused, once the requirement named one.
    pub verb: Option<runx_contracts::AuthorityVerb>,
}

impl Refusal {
    pub(crate) fn message(&self) -> String {
        self.reasons
            .iter()
            .find(|reason| reason.outcome == CheckOutcome::Fail)
            .map_or_else(
                || "the call was refused".to_owned(),
                |reason| reason.detail.to_string(),
            )
    }
}

struct Reasons(Vec<AdmissionReason>, Option<runx_contracts::AuthorityVerb>);

impl Reasons {
    fn pass(&mut self, check: AdmissionCheck, detail: impl Into<String>) {
        self.push(check, CheckOutcome::Pass, detail);
    }

    fn push(&mut self, check: AdmissionCheck, outcome: CheckOutcome, detail: impl Into<String>) {
        let detail = detail.into();
        self.0.push(AdmissionReason {
            check,
            outcome,
            detail: NonEmptyString::from(if detail.is_empty() {
                "-".to_owned()
            } else {
                detail
            }),
            refs: Vec::new(),
        });
    }

    fn refuse(mut self, check: AdmissionCheck, detail: impl Into<String>) -> AdmitError {
        self.push(check, CheckOutcome::Fail, detail);
        AdmitError::Refused(Refusal {
            check,
            reasons: self.0,
            verb: self.1,
        })
    }
}

/// Admit one exact call.
pub(crate) fn admit(evaluation: &Evaluation<'_>) -> Result<Admitted, AdmitError> {
    let mut reasons = Reasons(Vec::new(), None);
    let call = evaluation.call;

    // Requirement: the connector the step may use, and the operation on it.
    let (connector, operation) = match requirement(evaluation) {
        Ok(found) => found,
        Err(detail) => return Err(reasons.refuse(AdmissionCheck::Requirement, detail)),
    };
    let manifest = &connector.manifest;
    let (operation_name, operation_spec, operation_digest) = connector
        .operation(operation.as_str())
        .map(|(name, spec, digest)| (name.clone(), spec, digest.clone()))
        .ok_or_else(|| {
            Reasons(Vec::new(), None).refuse(
                AdmissionCheck::Requirement,
                format!(
                    "connector {} declares no operation {operation}",
                    manifest.id
                ),
            )
        })?;
    reasons.pass(
        AdmissionCheck::Requirement,
        format!("{}/{operation_name}", manifest.id),
    );
    reasons.1 = Some(operation_spec.verb.authority_verb());

    // Instance: the one binding the call runs under, with its pins current.
    // An operation on a held resource runs under what a purchase holds for
    // it, found by the target its arguments name, never a selected instance.
    let instance = match held_resource(evaluation, &connector, operation_spec) {
        Some(Ok(instance)) => instance,
        Some(Err(detail)) => return Err(reasons.refuse(AdmissionCheck::Instance, detail)),
        None => match evaluation.context.instances.resolve(&connector) {
            Ok(instance) => instance,
            Err(error) => return Err(reasons.refuse(AdmissionCheck::Instance, error.to_string())),
        },
    };
    if let Err(detail) = pins_current(&connector, &operation_name, &operation_digest, &instance) {
        return Err(reasons.refuse(AdmissionCheck::Instance, detail));
    }
    reasons.pass(
        AdmissionCheck::Instance,
        format!(
            "instance {} ({} placement, {} authority)",
            instance.instance_id,
            instance.placement.as_str(),
            instance.authority.kind()
        ),
    );

    // The call's shape: target, request key, amount, approval text. These
    // are the caller's errors, refused before any evidence is read.
    let access = operation_spec.access();
    let shape = call_shape(evaluation, operation_spec, &instance);
    let (target, request_key) = match shape {
        Ok(shape) => shape,
        Err(detail) => return Err(reasons.refuse(AdmissionCheck::Requirement, detail)),
    };
    let approval = approval_text(call, operation_spec);
    // What the call spends, counted only under a money limit.
    let limit = evaluation
        .run_authority
        .and_then(|authority| authority.max_cost_micro);
    let charge = limit
        .map(|_| spend::charge(operation_spec, call.amount.as_ref()))
        .transpose()
        .map(|charge| charge.filter(|charge| charge.usd_micro > 0));

    // Capability: what the principal can do at the provider.
    let capability = match capability::evidence(&CapabilityRequest {
        connector: &connector,
        instance: &instance,
        hosted: evaluation.context.hosted,
        env: evaluation.context.env,
        cwd: evaluation.context.cwd,
    }) {
        Ok(capability) => capability,
        Err(detail) => return Err(reasons.refuse(AdmissionCheck::Capability, detail)),
    };
    let actor_ref = evaluation
        .run_authority
        .and_then(|authority| authority.actor_ref.clone())
        .unwrap_or_else(|| capability.term.principal_ref.clone());
    let instance_ref = capability::instance_reference(&instance);
    let mut delegations = Vec::new();
    if let Some(authority) = evaluation.run_authority {
        if authority.actor_ref.is_none() && !authority.delegations.is_empty() {
            return Err(reasons.refuse(
                AdmissionCheck::Delegation,
                "the run's delegations name no actor to delegate to",
            ));
        }
        let mut preceding = capability.term.clone();
        for request in authority.delegations.iter().filter(|request| {
            request.resource_ref.reference_type == instance_ref.reference_type
                && request.resource_ref.uri == instance_ref.uri
        }) {
            let minted = mint_attenuated(
                &preceding,
                request,
                &ConnectorBoundsComparator,
                IsoDateTime::from(crate::time::now_iso8601()),
            );
            let (term, _) = match minted {
                Ok(minted) => minted,
                Err(error) => return Err(reasons.refuse(
                    AdmissionCheck::Delegation,
                    format!(
                        "the run's delegation does not narrow this instance's capability: {error}"
                    ),
                )),
            };
            preceding = term.clone();
            delegations.push(term);
        }
    }
    let mandate_ref = evaluation
        .run_authority
        .and_then(|authority| authority.mandate_ref.clone());
    if !delegations.is_empty()
        && !mandate_ref
            .as_ref()
            .is_some_and(|reference| reference.reference_type == ReferenceType::Mandate)
    {
        return Err(reasons.refuse(
            AdmissionCheck::Delegation,
            "the actor's delegation has no bound operator mandate",
        ));
    }
    let digests = digest(&call.input).and_then(|payload_digest| {
        let approval_digest = approval.as_ref().map(digest).transpose()?;
        // The act is bound into the intent, so its approval and custody are
        // the act's: a changed act is another call.
        let act_digest = call.act.as_ref().map(digest).transpose()?;
        let intent = OperationIntent {
            connector: connector.name.clone(),
            manifest_digest: connector.manifest_digest.clone(),
            connector_digest: connector.connector_digest.clone(),
            operation: operation_name.clone(),
            operation_digest,
            instance_id: instance.instance_id.clone(),
            target: NonEmptyString::from(target.as_str()),
            payload_digest,
            request_key_digest: request_key.as_deref().map(|key| Digest::of(key.as_bytes())),
            amount: call.amount.clone(),
            approval_digest,
            act_digest,
        };
        let intent_digest = digest(&intent)?;
        Ok((intent, intent_digest))
    });
    let (intent, intent_digest) = match digests {
        Ok(digests) => digests,
        Err(detail) => return Err(reasons.refuse(AdmissionCheck::Requirement, detail)),
    };
    let parent_limits = std::iter::once(&capability.term)
        .chain(delegations.iter())
        .filter_map(|term| term.bounds.uses.as_ref())
        .collect::<Vec<_>>();
    let intent_uses = (!parent_limits.is_empty()).then(|| AuthorityUseLimit {
        total: Some(1),
        per_day: parent_limits
            .iter()
            .any(|limit| limit.per_day.is_some())
            .then_some(1),
    });
    let term_request = term_request(
        &instance,
        actor_ref.clone(),
        connector.name.as_str(),
        &operation_name,
        operation_spec,
        &target,
        intent_uses,
    );

    // Capability and delegation, proven together by deriving the term.
    let (term, derivation) = match derive(
        &capability,
        &delegations,
        mandate_ref.as_ref(),
        &term_request,
        intent_digest.as_str(),
        Vec::new(),
        Vec::new(),
    ) {
        Ok(derived) => derived,
        Err((parent, detail)) => {
            let check = match parent {
                DerivationParent::Capability => AdmissionCheck::Capability,
                DerivationParent::Delegation(_) => AdmissionCheck::Delegation,
            };
            return Err(reasons.refuse(check, detail));
        }
    };

    // Custody: the act's attempts, from any run. A call recovers an
    // unsettled attempt of its own intent and is held by any other; a
    // bounded call takes the use of the attempt sequence it joins.
    let spends = matches!(charge, Ok(Some(_)));
    let custody =
        (access == OperationAccess::Mutate || !parent_limits.is_empty() || spends).then(|| {
            EffectCustody::for_act(
                Arc::clone(evaluation.store),
                &intent,
                &intent_digest,
                evaluation.run_id,
            )
        });
    let (pending, use_key, custody_head) = match &custody {
        Some(custody) => match custody.custody() {
            Ok(state) => (
                state.pending,
                Some(custody.use_key(state.sequence, intent_digest.as_str())),
                state.head,
            ),
            Err(error) => return Err(AdmitError::State(error.to_string())),
        },
        None => (None, None, 0),
    };
    if let Some(pending) = &pending {
        held(pending, intent_digest.as_str(), mandate_ref.as_ref())?;
    }
    let usage = UsageLedger::new(Arc::clone(evaluation.store));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let mut usage_covers = Vec::new();
    for parent in std::iter::once(&capability.term).chain(delegations.iter()) {
        let (Some(limit), Some(use_key)) = (&parent.bounds.uses, &use_key) else {
            continue;
        };
        match usage.cover(
            &format!("term:{}", parent.term_id),
            limit,
            use_key,
            intent_digest.as_str(),
            evaluation.run_id,
            now,
        ) {
            Ok(cover) => usage_covers.push(cover),
            Err(error @ (UsageError::TotalExhausted | UsageError::DailyExhausted)) => {
                return Err(reasons.refuse(
                    AdmissionCheck::Delegation,
                    format!("bounded authority use was refused: {error}"),
                ));
            }
            Err(error) => return Err(AdmitError::State(error.to_string())),
        }
    }
    reasons.pass(
        AdmissionCheck::Capability,
        format!(
            "{} covers {} on {target}",
            capability.source,
            operation_spec
                .scopes
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    reasons.pass(
        AdmissionCheck::Delegation,
        if delegations.is_empty() {
            "the principal acts directly"
        } else {
            "each delegation link was minted from its predecessor and covers the call"
        },
    );

    // Cost: the call fits what is left of the run's money limit. A run with
    // no limit spends unbounded, as it always has, and records no check.
    let spend = match (limit, charge) {
        (None, _) => None,
        (Some(_), Err(detail)) => return Err(reasons.refuse(AdmissionCheck::Cost, detail)),
        (Some(_), Ok(None)) => {
            reasons.pass(
                AdmissionCheck::Cost,
                "the call spends nothing against the run's limit",
            );
            None
        }
        (Some(limit), Ok(Some(charge))) => {
            let Some(spend_key) = use_key.clone() else {
                return Err(AdmitError::State(
                    "a call that spends is not in custody".to_owned(),
                ));
            };
            match spend::cover(
                evaluation.store,
                evaluation.run_id,
                limit,
                &spend_key,
                intent_digest.as_str(),
                charge.usd_micro,
            ) {
                Ok(_) => {}
                Err(error @ SpendError::Exceeded { .. }) => {
                    return Err(reasons.refuse(AdmissionCheck::Cost, error.to_string()));
                }
                Err(error) => return Err(AdmitError::State(error.to_string())),
            }
            reasons.pass(
                AdmissionCheck::Cost,
                format!(
                    "{} micro-USD within the run's limit of {limit}",
                    charge.usd_micro
                ),
            );
            Some(Spending {
                limit,
                charge,
                run_id: evaluation.run_id.to_owned(),
                spend_key,
            })
        }
    };

    // Approval: the operation's floor or the step's gate.
    let disposition = match &approval {
        Some(_) => {
            reasons.push(
                AdmissionCheck::Approval,
                CheckOutcome::Required,
                if operation_spec.approval == ApprovalFloor::Required {
                    "the operation requires approval of this exact intent"
                } else {
                    "the step requires approval of this exact intent"
                },
            );
            AdmissionDisposition::NeedsResolution
        }
        None => {
            reasons.pass(AdmissionCheck::Approval, "no approval floor applies");
            AdmissionDisposition::Admit
        }
    };

    // Implementation: the instance's placement executes the operation.
    let stood_in = evaluation.fixture_stand_in
        && instance.placement == Placement::Hosted
        && operation_spec.hosted.is_none();
    if !stood_in && let Err(detail) = executable(operation_spec, instance.placement) {
        return Err(reasons.refuse(
            AdmissionCheck::Implementation,
            format!("{}/{operation_name}: {detail}", manifest.id),
        ));
    }
    reasons.pass(
        AdmissionCheck::Implementation,
        format!("{} placement", instance.placement.as_str()),
    );

    // Tolerance: what this use accepts when the outcome is unknown.
    if let Err(detail) = tolerance(evaluation, operation_spec) {
        return Err(reasons.refuse(AdmissionCheck::Tolerance, detail));
    }
    reasons.pass(
        AdmissionCheck::Tolerance,
        "within the operation's guarantees",
    );

    let admitted = disposition == AdmissionDisposition::Admit;
    Ok(Admitted {
        admission: Admission {
            disposition,
            intent,
            intent_digest,
            reasons: reasons.0,
            execution_term: admitted.then_some(term),
            derivation: admitted.then_some(derivation),
        },
        connector,
        operation: operation_name,
        instance,
        capability,
        actor_ref,
        delegations,
        mandate_ref,
        custody,
        pending,
        custody_head,
        usage_covers,
        spend,
        target,
        request_key,
        approval,
        term_request,
    })
}

/// An unsettled attempt of the act holds a call of any other intent or
/// mandate.
fn held(
    pending: &Pending,
    intent_digest: &str,
    mandate_ref: Option<&Reference>,
) -> Result<(), AdmitError> {
    if pending.intent_digest != intent_digest {
        return Err(AdmitError::State(
            "an unsettled attempt of this act carries another intent; reconcile it before running a different call".to_owned(),
        ));
    }
    if pending.mandate_ref.as_ref() != mandate_ref {
        return Err(AdmitError::State(
            "an unsettled attempt of this act was admitted under another mandate; reconcile it before resuming".to_owned(),
        ));
    }
    Ok(())
}

/// The readiness of every operation a skill's connectors block lists, from
/// the instances and evidence present here. A projection, never an admission.
pub fn skill_readiness(
    skill_dir: &Path,
    requirements: &BTreeMap<String, ConnectorRequirement>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<Readiness>, String> {
    let scope = ConnectorScope::at(skill_dir)
        .map_err(|error| error.to_string())?
        .in_project(crate::paths::RunxPaths::resolve(env, cwd).connectors());
    project(&scope, requirements, env, cwd)
}

/// The readiness of every operation of one connector in the workspace at
/// `cwd`, as if a skill required all of them.
pub fn connector_readiness(
    connector: &LoadedConnector,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<Readiness>, String> {
    let scope = ConnectorScope::at(cwd)
        .map_err(|error| error.to_string())?
        .in_project(crate::paths::RunxPaths::resolve(env, cwd).connectors());
    let requirement = ConnectorRequirement {
        operations: connector.manifest.operations.keys().cloned().collect(),
        manifest_digest: Some(connector.manifest_digest.clone()),
        connector_from: None,
        allowed_connectors: Vec::new(),
    };
    project(
        &scope,
        &BTreeMap::from([(connector.name.as_str().to_owned(), requirement)]),
        env,
        cwd,
    )
}

fn project(
    scope: &ConnectorScope,
    requirements: &BTreeMap<String, ConnectorRequirement>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<Readiness>, String> {
    let store = super::store::KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd))
        .map_err(|error| error.to_string())?;
    let instances = Instances::new(store);
    let hosted = HostedClient::default();
    let context = Context {
        scope,
        instances: &instances,
        hosted: &hosted,
        env,
        cwd,
    };
    Ok(requirements
        .iter()
        .flat_map(|(key, requirement)| {
            requirement
                .operations
                .iter()
                .map(move |operation| readiness(key, requirement, operation, context))
        })
        .collect())
}

/// Project whether one requirement's operation could be admitted here,
/// without an exact call: the requirement, the instance and its pins, the
/// capability's scopes where they can be read without a target, the
/// approval floor and the implementation.
fn readiness(
    requirement_key: &str,
    requirement: &ConnectorRequirement,
    operation: &OperationName,
    context: Context<'_>,
) -> Readiness {
    let Context {
        scope,
        instances,
        hosted,
        env,
        cwd,
    } = context;
    let mut reasons = Reasons(Vec::new(), None);
    let mut readiness = Readiness {
        requirement: NonEmptyString::from(requirement_key),
        connector: None,
        operation: operation.clone(),
        instance_id: None,
        placement: None,
        ready: false,
        reasons: Vec::new(),
        cost: None,
    };
    let finish = |mut readiness: Readiness, reasons: Reasons| {
        readiness.ready = !reasons
            .0
            .iter()
            .any(|reason| reason.outcome == CheckOutcome::Fail);
        readiness.reasons = reasons.0;
        readiness
    };
    if requirement.connector_from.is_some() {
        reasons.push(
            AdmissionCheck::Requirement,
            CheckOutcome::Unknown,
            "the connector is chosen at run time",
        );
        return finish(readiness, reasons);
    }
    let connector = match scope.requirement(requirement_key, requirement) {
        Ok(connector) => connector,
        Err(error) => {
            reasons.push(
                AdmissionCheck::Requirement,
                CheckOutcome::Fail,
                error.to_string(),
            );
            return finish(readiness, reasons);
        }
    };
    readiness.connector = Some(connector.name.clone());
    let Some((operation_name, spec, operation_digest)) = connector.operation(operation.as_str())
    else {
        reasons.push(
            AdmissionCheck::Requirement,
            CheckOutcome::Fail,
            format!(
                "connector {} declares no operation {operation}",
                connector.manifest.id
            ),
        );
        return finish(readiness, reasons);
    };
    reasons.pass(
        AdmissionCheck::Requirement,
        format!("{}/{operation_name}", connector.manifest.id),
    );
    readiness.cost.clone_from(&spec.cost);
    let instance = match instances.resolve(&connector) {
        Ok(instance) => instance,
        Err(error) => {
            reasons.push(
                AdmissionCheck::Instance,
                CheckOutcome::Fail,
                error.to_string(),
            );
            return finish(readiness, reasons);
        }
    };
    readiness.instance_id = Some(instance.instance_id.clone());
    readiness.placement = Some(instance.placement);
    if let Err(detail) = pins_current(&connector, operation_name, operation_digest, &instance) {
        reasons.push(AdmissionCheck::Instance, CheckOutcome::Fail, detail);
        return finish(readiness, reasons);
    }
    reasons.pass(
        AdmissionCheck::Instance,
        format!("instance {}", instance.instance_id),
    );
    match capability::evidence(&CapabilityRequest {
        connector: &connector,
        instance: &instance,
        hosted,
        env,
        cwd,
    }) {
        Ok(capability) => {
            let missing = spec
                .scopes
                .iter()
                .filter(|scope| {
                    !capability
                        .scopes
                        .iter()
                        .any(|granted| granted == scope.as_str())
                })
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>();
            if missing.is_empty() {
                reasons.pass(
                    AdmissionCheck::Capability,
                    format!("{} covers the operation's scopes", capability.source),
                );
            } else {
                reasons.push(
                    AdmissionCheck::Capability,
                    CheckOutcome::Fail,
                    format!("{} lacks {}", capability.source, missing.join(", ")),
                );
            }
        }
        Err(detail) => reasons.push(AdmissionCheck::Capability, CheckOutcome::Fail, detail),
    }
    reasons.pass(AdmissionCheck::Delegation, "the principal acts directly");
    if spec.approval == ApprovalFloor::Required {
        reasons.push(
            AdmissionCheck::Approval,
            CheckOutcome::Required,
            "each call needs approval of its exact intent",
        );
    } else {
        reasons.pass(AdmissionCheck::Approval, "no approval floor applies");
    }
    match executable(spec, instance.placement) {
        Ok(()) => reasons.pass(
            AdmissionCheck::Implementation,
            format!("{} placement", instance.placement.as_str()),
        ),
        Err(detail) => reasons.push(AdmissionCheck::Implementation, CheckOutcome::Fail, detail),
    }
    finish(readiness, reasons)
}

fn requirement(
    evaluation: &Evaluation<'_>,
) -> Result<(Arc<LoadedConnector>, OperationName), String> {
    let step = evaluation.step;
    let declared = step.declared.as_ref().ok_or_else(|| {
        format!(
            "the step's requirement {} is not declared by an enclosing skill's connectors block",
            step.requirement
        )
    })?;
    let call = evaluation.call;
    let connector_id: &str = match &declared.connector_from {
        None => {
            if call.connector.is_some() {
                return Err("a fixed requirement cannot be given a connector".to_owned());
            }
            &step.requirement
        }
        Some(from) => {
            let chosen = call
                .connector
                .as_ref()
                .ok_or_else(|| format!("{from} named no connector"))?;
            if !declared.allowed_connectors.contains(chosen) {
                return Err(format!(
                    "connector {chosen} is not one requirement {} allows",
                    step.requirement
                ));
            }
            chosen.as_str()
        }
    };
    let operation = match (&step.operation, &call.operation_name) {
        (Some(operation), None) => operation.clone(),
        (None, Some(operation)) => operation.clone(),
        (Some(_), Some(_)) => {
            return Err("the step names its operation and also an operation_name".to_owned());
        }
        (None, None) => return Err("the step chooses no operation_name".to_owned()),
    };
    if !declared.lists(operation.as_str()) {
        return Err(format!(
            "requirement {} does not list operation {operation}",
            step.requirement
        ));
    }
    let connector = evaluation
        .context
        .scope
        .requirement(connector_id, declared)
        .map_err(|error| error.to_string())?;
    Ok((connector, operation))
}

/// The instance still means what it meant when it was confirmed, and the
/// credential it holds may authorize the operation.
fn pins_current(
    connector: &LoadedConnector,
    operation: &OperationName,
    operation_digest: &Digest,
    instance: &ConnectorInstance,
) -> Result<(), String> {
    let instance = &instance;
    if instance.connector != connector.name {
        return Err(format!(
            "instance {} was confirmed for connector {}, not {}; confirm one for {}",
            instance.instance_id, instance.connector, connector.name, connector.name
        ));
    }
    if instance.manifest_digest != connector.manifest_digest
        || instance.connector_digest != connector.connector_digest
    {
        return Err(format!(
            "connector {} changed since instance {} was confirmed; confirm its binding again",
            connector.name, instance.instance_id
        ));
    }
    if instance.operation_digests.get(operation) != Some(operation_digest) {
        return Err(format!(
            "operation {}/{operation} changed since instance {} was confirmed; confirm it again",
            connector.manifest.id, instance.instance_id
        ));
    }
    if let Some(credential) =
        super::instances::delivered_credential(&connector.manifest, &instance.authority)
    {
        if !connector
            .operation(operation.as_str())
            .is_some_and(|(_, declared, _)| declared.credentials.contains(credential))
        {
            return Err(format!(
                "operation {}/{operation} does not accept credential {credential}, which instance {} holds",
                connector.manifest.id, instance.instance_id
            ));
        }
        let declaration = connector
            .manifest
            .auth
            .credentials
            .get(credential)
            .ok_or_else(|| {
                format!(
                    "connector {} no longer declares credential {credential}",
                    connector.manifest.id
                )
            })?;
        let current =
            credential_declaration_digest(declaration).map_err(|error| error.to_string())?;
        if instance.credential_declaration_digest.as_ref() != Some(&current) {
            return Err(format!(
                "credential {credential} of connector {} changed where it may be sent since instance {} was confirmed; confirm it again",
                connector.manifest.id, instance.instance_id
            ));
        }
    }
    Ok(())
}

/// The rendered target and the request key, after the call's shape is
/// checked against the operation.
/// What a purchase holds for the call's resource, pinned for this call: the
/// operation's purchase credential on the target its arguments render. None
/// for an operation a connector instance serves.
fn held_resource(
    evaluation: &Evaluation<'_>,
    connector: &LoadedConnector,
    operation: &ConnectorOperation,
) -> Option<Result<ConnectorInstance, String>> {
    let credential = operation.credentials.iter().find(|name| {
        connector
            .manifest
            .auth
            .credentials
            .get(*name)
            .is_some_and(runx_contracts::connector::CredentialDeclaration::is_purchase)
    })?;
    Some((|| {
        let arguments = JsonValue::Object(evaluation.call.input.clone());
        let target = operation
            .target
            .template
            .render_string(&TemplateInputs {
                arguments: Some(&arguments),
                ..TemplateInputs::default()
            })
            .map_err(|error| format!("the held resource cannot be named: {error}"))?;
        let session = evaluation
            .context
            .hosted
            .session(evaluation.context.env, evaluation.context.cwd)
            .map_err(|error| {
                format!("what a purchase holds is reached through a runx account: {error}")
            })?;
        Ok(super::instances::pinned(
            connector,
            super::instances::instance_id(&[
                "held",
                connector.name.as_str(),
                credential.as_str(),
                &target,
            ])?,
            runx_contracts::Placement::Hosted,
            session.principal_ref,
            None,
            runx_contracts::InstanceAuthority::Held {
                credential: credential.clone(),
                target: target.into(),
            },
        ))
    })())
}

fn call_shape(
    evaluation: &Evaluation<'_>,
    operation: &ConnectorOperation,
    instance: &ConnectorInstance,
) -> Result<(String, Option<String>), String> {
    let call = evaluation.call;
    let access = operation.access();
    input_conforms(operation, &call.input)?;
    let arguments = JsonValue::Object(call.input.clone());
    let rendered = operation
        .target
        .template
        .render_string(&TemplateInputs {
            arguments: Some(&arguments),
            parameters: Some(&instance.parameters),
            ..TemplateInputs::default()
        })
        .map_err(|error| format!("the target cannot be rendered: {error}"))?;
    if let Some(pattern) = &operation.target.pattern {
        let pattern = runx_parser::whole_match(pattern.as_str())
            .map_err(|error| format!("the target pattern is invalid: {error}"))?;
        if !pattern.is_match(&rendered) {
            return Err(format!(
                "target {rendered:?} does not have the shape the connector declares"
            ));
        }
    }
    let target = rendered;
    if target.trim().is_empty() || target.len() > 512 || target.chars().any(char::is_control) {
        return Err("the rendered target must be a safe non-empty value".to_owned());
    }
    let request_key = call.idempotency_key.as_deref();
    let request_key = match access {
        OperationAccess::Mutate => Some(
            request_key
                .ok_or("a mutation needs an idempotency_key")?
                .to_owned(),
        ),
        OperationAccess::Read => {
            if call.approval.is_some() {
                return Err("a read never takes an approval".to_owned());
            }
            None
        }
    };
    match (operation.amount, &call.amount) {
        (AmountRule::Required, None) => return Err("the operation requires an amount".to_owned()),
        (AmountRule::None, Some(_)) => return Err("the operation takes no amount".to_owned()),
        _ => {}
    }
    if call.readback_of.is_some() && access != OperationAccess::Read {
        return Err("readback_of applies only to reads".to_owned());
    }
    Ok((target, request_key))
}

/// The call's input against the schema the operation declares. Every bound a
/// connector puts on its input, such as how many items a repeated request
/// covers, holds here or the call is refused. The refusal names where the
/// input failed, never the value.
fn input_conforms(operation: &ConnectorOperation, input: &JsonObject) -> Result<(), String> {
    let Some(schema) = &operation.input_schema else {
        return Ok(());
    };
    let schema = serde_json::to_value(schema).map_err(|error| error.to_string())?;
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| format!("the operation's input schema is invalid: {error}"))?;
    let instance = serde_json::to_value(input).map_err(|error| error.to_string())?;
    match validator.iter_errors(&instance).next() {
        None => Ok(()),
        Some(error) => Err(format!(
            "the input at {:?} does not satisfy the operation's schema at {:?}",
            error.instance_path().as_str(),
            error.schema_path().as_str()
        )),
    }
}

fn approval_text(call: &OperationCall, operation: &ConnectorOperation) -> Option<ApprovalText> {
    if let Some(text) = &call.approval {
        return Some(text.clone());
    }
    (operation.approval == ApprovalFloor::Required).then(|| ApprovalText {
        reason: operation.description.as_str().to_owned(),
        gate_type: Some("operation".to_owned()),
    })
}

/// Whether this runtime can execute the operation on the placement. A
/// `process` implementation is reached by a host's agent, not a step.
fn executable(operation: &ConnectorOperation, placement: Placement) -> Result<(), String> {
    if !operation.implemented_by(placement) {
        return Err(format!(
            "the {} placement does not implement it",
            placement.as_str()
        ));
    }
    Ok(())
}

fn tolerance(evaluation: &Evaluation<'_>, operation: &ConnectorOperation) -> Result<(), String> {
    if operation.access() == OperationAccess::Read {
        if evaluation.duplicates.is_some() {
            return Err("a read has no uncertain outcome to tolerate".to_owned());
        }
        return Ok(());
    }
    Ok(())
}

/// The intent as the narrowest term: one verb, one operation, its scopes and
/// the exact target. Use caps are admitted only after custody can reserve them.
fn term_request(
    instance: &ConnectorInstance,
    actor_ref: Reference,
    connector: &str,
    operation: &OperationName,
    spec: &ConnectorOperation,
    target: &str,
    uses: Option<AuthorityUseLimit>,
) -> AttenuationRequest {
    AttenuationRequest {
        principal_ref: actor_ref,
        resource_ref: capability::instance_reference(instance),
        resource_family: AuthorityResourceFamily::Connector,
        verbs: vec![spec.verb.authority_verb()],
        bounds: AuthorityBounds {
            operations: vec![NonEmptyString::from(format!("{connector}/{operation}"))],
            scopes: spec.scopes.iter().cloned().collect(),
            targets: vec![NonEmptyString::from(target)],
            uses,
        },
        expires_at: None,
    }
}

fn derive(
    capability: &Capability,
    delegations: &[AuthorityTerm],
    mandate_ref: Option<&Reference>,
    request: &AttenuationRequest,
    intent_digest: &str,
    approvals: Vec<AuthorityApproval>,
    approval_evidence: Vec<AuthorityEvidence>,
) -> Result<(AuthorityTerm, AuthorityDerivation), (DerivationParent, String)> {
    let derived_at = IsoDateTime::from(crate::time::now_iso8601());
    let (term, proofs) = derive_execution_term(
        &ExecutionTermRequest {
            capability: &capability.term,
            delegations,
            intent: request,
            intent_digest,
            approvals,
            issued_by_ref: Reference::runx(ReferenceType::Host, "runx-kernel"),
        },
        &ConnectorBoundsComparator,
        derived_at.clone(),
    )
    .map_err(|error| {
        let parent = error.parent();
        let detail = match parent {
            DerivationParent::Capability => {
                format!("{} does not cover this call: {error}", capability.source)
            }
            DerivationParent::Delegation(index) => {
                format!("delegation link {index} does not cover this call: {error}")
            }
        };
        (parent, detail)
    })?;
    let mut evidence = vec![AuthorityEvidence {
        kind: AuthorityEvidenceKind::Capability,
        reference: capability.term.issued_by_ref.clone(),
        term: Some(capability.term.clone()),
    }];
    let source = if delegations.is_empty() {
        None
    } else {
        Some(mandate_ref.ok_or((
            DerivationParent::Delegation(0),
            "delegation evidence has no operator mandate".to_owned(),
        ))?)
    };
    if let Some(source) = source {
        evidence.extend(delegations.iter().map(|term| AuthorityEvidence {
            kind: AuthorityEvidenceKind::Delegation,
            reference: source.clone(),
            term: Some(term.clone()),
        }));
    }
    evidence.extend(approval_evidence);
    Ok((
        term.clone(),
        AuthorityDerivation {
            term_id: term.term_id.clone(),
            intent_digest: NonEmptyString::from(intent_digest),
            evidence,
            proofs,
            derived_at,
        },
    ))
}

fn digest<T: serde::Serialize + ?Sized>(value: &T) -> Result<Digest, String> {
    stable_digest(value).map_err(|error| format!("the call cannot be digested: {error}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use runx_contracts::{InstanceAuthority, Placement};

    use super::pins_current;
    use crate::ConnectorScope;
    use crate::kernel::instances::pinned;

    /// Two plugins ship a connector of the same name and the same operations.
    /// Its process tool is each plugin's own code, so an account confirmed
    /// for one plugin's connector never serves the other's.
    #[test]
    fn an_instance_serves_only_the_connector_revision_it_was_confirmed_for() {
        let plugin = |tool_source: &str| {
            let root = tempfile::tempdir().unwrap();
            let skill = root.path().join("skills/sync");
            std::fs::create_dir_all(&skill).unwrap();
            let connector = root.path().join("connectors/acme");
            std::fs::create_dir_all(&connector).unwrap();
            std::fs::write(
                connector.join("connector.yaml"),
                r#"schema: runx.connector.v1
id: acme
name: Acme
description: Syncs records.
runtime_capability: false
auth:
  credentials:
    key:
      kind: api_key
      fields: [{name: key}]
      delivery: {env: ACME_KEY}
      audience: [api.acme.invalid]
placements: {local: {auth: [credentials]}}
operations:
  records.read:
    description: Read records.
    verb: read
    scopes: [records.read]
    credentials: [key]
    target: {template: "acme://records"}
    implementation:
      process: {tool: acme.records}
"#,
            )
            .unwrap();
            let tool = connector.join("tools/acme/records");
            std::fs::create_dir_all(&tool).unwrap();
            std::fs::write(
                tool.join("manifest.json"),
                r#"{"schema": "runx.tool.manifest.v1", "name": "acme.records", "version": "0.1.0", "description": "Reads records.", "source": {"command": "node", "args": ["./records.mjs"]}}"#,
            )
            .unwrap();
            std::fs::write(tool.join("records.mjs"), tool_source).unwrap();
            let loaded = ConnectorScope::at(&skill)
                .unwrap()
                .connector("acme")
                .unwrap();
            (root, loaded)
        };
        let (_first_root, first) = plugin("process.stdout.write('{}');\n");
        let (_second_root, second) = plugin("process.stdout.write('{\"other\":true}');\n");
        assert_eq!(first.operation_digests, second.operation_digests);
        assert_ne!(first.connector_digest, second.connector_digest);

        let instance = pinned(
            &first,
            runx_contracts::InstanceId::new("acme").unwrap(),
            Placement::Local,
            "runx:principal:operator".to_owned(),
            None,
            InstanceAuthority::Credential {
                profile: "acme".into(),
                credential: runx_contracts::connector::CredentialName::new("key").unwrap(),
                scopes: vec!["records.read".into()],
                material_ref: "material-1".into(),
            },
        );
        let (name, _, digest) = first.operation("records.read").unwrap();
        pins_current(&first, name, digest, &instance).unwrap();
        let refused = pins_current(&second, name, digest, &instance).unwrap_err();
        assert!(refused.contains("changed since instance"), "{refused}");
    }
}
