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
    Admission, AdmissionCheck, AdmissionDisposition, AdmissionReason, AuthorityApproval,
    AuthorityBounds, AuthorityDerivation, AuthorityEvidence, AuthorityEvidenceKind,
    AuthorityResourceFamily, AuthorityTerm, AuthorityUseLimit, CheckOutcome, InstanceAuthority,
    JsonObject, JsonValue, OperationIntent, Readiness, Reference, ReferenceType,
};
use runx_core::policy::{
    AttenuationRequest, ConnectorBoundsComparator, DerivationParent, ExecutionTermRequest,
    derive_execution_term, mint_attenuated,
};
use runx_parser::{ConnectorRequirement, DuplicatePolicy, OperationStepTarget, UncertainOutcome};

use super::capability::{self, Capability, CapabilityRequest};
use super::connectors::{ConnectorScope, LoadedConnector};
use super::instances::{Instances, ResolvedInstance};
use super::operation::call::{ApprovalText, OperationCall};
use super::operation::{egress, hosted::HostedClient};
use super::usage::{UsageLedger, UseCover};

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
    /// The step-level request key, used when the call carries none.
    pub step_idempotency_key: Option<&'a str>,
    pub on_uncertain: Option<UncertainOutcome>,
    pub duplicates: Option<DuplicatePolicy>,
    /// The harness's provider fixture stands in for a declared operation's
    /// missing hosted implementation.
    pub fixture_stand_in: bool,
    pub run_authority: Option<&'a super::authority::RunAuthority>,
    pub usage: &'a UsageLedger,
    pub call_key: &'a str,
    pub run_id: &'a str,
    pub context: Context<'a>,
}

/// An exact call admitted, or owed an approval.
#[derive(Clone, Debug)]
pub(crate) struct Admitted {
    /// `Admit` or `NeedsResolution`; never `Refuse`.
    pub admission: Admission,
    pub connector: Arc<LoadedConnector>,
    pub operation: OperationName,
    pub instance: ResolvedInstance,
    pub capability: Capability,
    pub delegations: Vec<AuthorityTerm>,
    pub mandate_ref: Option<Reference>,
    pub usage_covers: Vec<UseCover>,
    pub target: String,
    pub request_key: Option<String>,
    /// What a person is shown, when the call is gated.
    pub approval: Option<ApprovalText>,
    /// The host's egress guard applies: approval shows the exact arguments.
    pub guarded: bool,
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

    fn refuse(mut self, check: AdmissionCheck, detail: impl Into<String>) -> Refusal {
        self.push(check, CheckOutcome::Fail, detail);
        Refusal {
            check,
            reasons: self.0,
            verb: self.1,
        }
    }
}

/// Admit one exact call.
pub(crate) fn admit(evaluation: &Evaluation<'_>) -> Result<Admitted, Refusal> {
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
    let instance = match evaluation
        .context
        .instances
        .resolve(&connector, evaluation.context.env)
    {
        Ok(instance) => instance,
        Err(error) => return Err(reasons.refuse(AdmissionCheck::Instance, error.to_string())),
    };
    if let Err(detail) = pins_current(&connector, &operation_name, &operation_digest, &instance) {
        return Err(reasons.refuse(AdmissionCheck::Instance, detail));
    }
    reasons.pass(
        AdmissionCheck::Instance,
        format!(
            "instance {} ({} placement, {} authority)",
            instance.instance.instance_id,
            instance.instance.placement.as_str(),
            instance.instance.authority.kind()
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
    let arguments = JsonValue::Object(call.input.clone());
    let scopes = operation_spec
        .scopes
        .iter()
        .map(|scope| scope.as_str())
        .collect::<Vec<_>>();
    let guarded = match egress::admit(evaluation.context.env, &arguments, &scopes) {
        Ok(guarded) => guarded && access == OperationAccess::Mutate,
        Err(detail) => return Err(reasons.refuse(AdmissionCheck::Delegation, detail)),
    };
    let approval = approval_text(call, operation_spec, guarded);

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
    let actor_ref = evaluation.run_authority.map_or_else(
        || capability.term.principal_ref.clone(),
        |authority| authority.actor_ref.clone(),
    );
    let instance_ref = capability::instance_reference(&instance.instance);
    let mut delegations = Vec::new();
    if let Some(authority) = evaluation.run_authority {
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
        let intent = OperationIntent {
            connector: manifest.id.clone(),
            operation: operation_name.clone(),
            operation_digest,
            instance_id: instance.instance.instance_id.clone(),
            target: NonEmptyString::from(target.as_str()),
            payload_digest,
            request_key_digest: request_key.as_deref().map(|key| Digest::of(key.as_bytes())),
            amount: call.amount.clone(),
            approval_digest,
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
        actor_ref,
        manifest.id.as_str(),
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
    let use_key = runx_contracts::sha256_prefixed(
        format!("{}\0{intent_digest}", evaluation.call_key).as_bytes(),
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let mut usage_covers = Vec::new();
    for parent in std::iter::once(&capability.term).chain(delegations.iter()) {
        if let Some(limit) = &parent.bounds.uses {
            match evaluation.usage.cover(
                &format!("term:{}", parent.term_id),
                limit,
                &use_key,
                intent_digest.as_str(),
                evaluation.run_id,
                now,
            ) {
                Ok(cover) => usage_covers.push(cover),
                Err(error) => {
                    return Err(reasons.refuse(
                        AdmissionCheck::Delegation,
                        format!("bounded authority use was refused: {error}"),
                    ));
                }
            }
        }
    }
    reasons.pass(
        AdmissionCheck::Capability,
        format!(
            "grant {} covers {} on {target}",
            capability.grant_id,
            scopes.join(", ")
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

    // Approval: the operation's floor, the step's gate, or the host's guard.
    let disposition = match &approval {
        Some(_) => {
            reasons.push(
                AdmissionCheck::Approval,
                CheckOutcome::Required,
                if operation_spec.approval == ApprovalFloor::Required {
                    "the operation requires approval of this exact intent"
                } else if guarded {
                    "the host requires approval of every outward mutation"
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
        && instance.instance.placement == Placement::Hosted
        && operation_spec.hosted.is_none();
    if !stood_in && let Err(detail) = executable(operation_spec, instance.instance.placement) {
        return Err(reasons.refuse(
            AdmissionCheck::Implementation,
            format!("{}/{operation_name}: {detail}", manifest.id),
        ));
    }
    reasons.pass(
        AdmissionCheck::Implementation,
        format!("{} placement", instance.instance.placement.as_str()),
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
        delegations,
        mandate_ref,
        usage_covers,
        target,
        request_key,
        approval,
        guarded,
        term_request,
    })
}

/// The readiness of every operation a skill's connectors block lists, from
/// the instances and evidence present here. A projection, never an admission.
pub fn skill_readiness(
    skill_dir: &Path,
    requirements: &BTreeMap<String, ConnectorRequirement>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<Readiness>, String> {
    let scope = ConnectorScope::for_skill(skill_dir).map_err(|error| error.to_string())?;
    project(&scope, requirements, env, cwd)
}

/// The readiness of every operation of one connector in the workspace at
/// `cwd`, as if a skill required all of them.
pub fn connector_readiness(
    connector: &LoadedConnector,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<Readiness>, String> {
    let scope = ConnectorScope::for_workspace(cwd).map_err(|error| error.to_string())?;
    let requirement = ConnectorRequirement {
        operations: connector.manifest.operations.keys().cloned().collect(),
        connector_from: None,
        allowed_connectors: Vec::new(),
    };
    project(
        &scope,
        &BTreeMap::from([(connector.manifest.id.as_str().to_owned(), requirement)]),
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
    let store =
        super::store::KernelStore::for_workspace(env, cwd).map_err(|error| error.to_string())?;
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
    let connector = match scope.connector(requirement_key) {
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
    readiness.connector = Some(connector.manifest.id.clone());
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
    let instance = match instances.resolve(&connector, env) {
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
    readiness.instance_id = Some(instance.instance.instance_id.clone());
    readiness.placement = Some(instance.instance.placement);
    if let Err(detail) = pins_current(&connector, operation_name, operation_digest, &instance) {
        reasons.push(AdmissionCheck::Instance, CheckOutcome::Fail, detail);
        return finish(readiness, reasons);
    }
    reasons.pass(
        AdmissionCheck::Instance,
        format!("instance {}", instance.instance.instance_id),
    );
    match &instance.instance.authority {
        InstanceAuthority::Session { .. } => reasons.push(
            AdmissionCheck::Capability,
            CheckOutcome::Unknown,
            "the session's permission depends on the call's target",
        ),
        _ => match capability::evidence(&CapabilityRequest {
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
                        format!(
                            "grant {} covers the operation's scopes",
                            capability.grant_id
                        ),
                    );
                } else {
                    reasons.push(
                        AdmissionCheck::Capability,
                        CheckOutcome::Fail,
                        format!("grant {} lacks {}", capability.grant_id, missing.join(", ")),
                    );
                }
            }
            Err(detail) => reasons.push(AdmissionCheck::Capability, CheckOutcome::Fail, detail),
        },
    }
    reasons.pass(AdmissionCheck::Delegation, "the principal acts directly");
    if spec.approval == ApprovalFloor::Required {
        reasons.push(
            AdmissionCheck::Approval,
            CheckOutcome::Required,
            "each call needs approval of its exact intent or a standing approval",
        );
    } else {
        reasons.pass(AdmissionCheck::Approval, "no approval floor applies");
    }
    match executable(spec, instance.instance.placement) {
        Ok(()) => reasons.pass(
            AdmissionCheck::Implementation,
            format!("{} placement", instance.instance.placement.as_str()),
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
    let operation = match (&step.operation, &call.operation) {
        (Some(operation), None) => operation.clone(),
        (None, Some(operation)) => operation.clone(),
        (Some(_), Some(_)) => {
            return Err("the step names its operation and also binds one".to_owned());
        }
        (None, None) => return Err("the step binds no operation".to_owned()),
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
        .connector(connector_id)
        .map_err(|error| error.to_string())?;
    Ok((connector, operation))
}

/// The instance still means what it meant when it was confirmed, and the
/// credential it holds may authorize the operation.
fn pins_current(
    connector: &LoadedConnector,
    operation: &OperationName,
    operation_digest: &Digest,
    instance: &ResolvedInstance,
) -> Result<(), String> {
    let instance = &instance.instance;
    let origin = super::instances::plugin_root(connector);
    if instance.plugin_root.as_ref().map(|root| root.as_str()) != origin.as_deref() {
        return Err(format!(
            "instance {} was confirmed for the {} connector of {}, not of {}; confirm one here",
            instance.instance_id,
            connector.manifest.id,
            instance
                .plugin_root
                .as_ref()
                .map_or("runx", |root| root.as_str()),
            origin.as_deref().unwrap_or("runx"),
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
fn call_shape(
    evaluation: &Evaluation<'_>,
    operation: &ConnectorOperation,
    instance: &ResolvedInstance,
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
            parameters: Some(&instance.instance.parameters),
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
    let request_key = call
        .idempotency_key
        .as_deref()
        .or(evaluation.step_idempotency_key);
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

fn approval_text(
    call: &OperationCall,
    operation: &ConnectorOperation,
    guarded: bool,
) -> Option<ApprovalText> {
    if let Some(text) = &call.approval {
        return Some(text.clone());
    }
    if operation.approval == ApprovalFloor::Required {
        return Some(ApprovalText {
            reason: operation.description.as_str().to_owned(),
            gate_type: Some("operation".to_owned()),
        });
    }
    guarded.then(|| ApprovalText {
        reason: "Approve this exact assistant provider mutation.".to_owned(),
        gate_type: Some("assistant_outward".to_owned()),
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
        if evaluation.on_uncertain.is_some() || evaluation.duplicates.is_some() {
            return Err("a read has no uncertain outcome to tolerate".to_owned());
        }
        return Ok(());
    }
    if evaluation.on_uncertain == Some(UncertainOutcome::Reconcile)
        && operation.guarantees.reconcile.is_none()
    {
        return Err(
            "the operation names no reconcile read, so this use cannot reconcile".to_owned(),
        );
    }
    Ok(())
}

/// The intent as the narrowest term: one verb, one operation, its scopes and
/// the exact target. Use caps are admitted only after custody can reserve them.
fn term_request(
    instance: &ResolvedInstance,
    actor_ref: Reference,
    connector: &str,
    operation: &OperationName,
    spec: &ConnectorOperation,
    target: &str,
    uses: Option<AuthorityUseLimit>,
) -> AttenuationRequest {
    AttenuationRequest {
        principal_ref: actor_ref,
        resource_ref: capability::instance_reference(&instance.instance),
        resource_family: AuthorityResourceFamily::Connector,
        verbs: vec![spec.verb.authority_verb()],
        capabilities: Vec::new(),
        bounds: AuthorityBounds {
            operations: vec![NonEmptyString::from(format!("{connector}/{operation}"))],
            scopes: spec.scopes.iter().cloned().collect(),
            targets: vec![NonEmptyString::from(target)],
            uses,
            ..AuthorityBounds::default()
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
            DerivationParent::Capability => format!(
                "grant {} does not cover this call: {error}",
                capability.grant_id
            ),
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
    use crate::kernel::instances::{InstanceSource, ResolvedInstance, pinned};

    /// Two plugins ship a connector of the same id and the same operations.
    /// Its process tool is each plugin's own code, so an account confirmed
    /// for one plugin's connector never serves the other's.
    #[test]
    fn an_instance_serves_only_the_plugin_it_was_confirmed_for() {
        let plugin = || {
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
            let loaded = ConnectorScope::for_skill(&skill)
                .unwrap()
                .connector("acme")
                .unwrap();
            (root, loaded)
        };
        let (_first_root, first) = plugin();
        let (_second_root, second) = plugin();
        assert_eq!(first.operation_digests, second.operation_digests);

        let instance = ResolvedInstance {
            instance: pinned(
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
            ),
            source: InstanceSource::Selected,
        };
        let (name, _, digest) = first.operation("records.read").unwrap();
        pins_current(&first, name, digest, &instance).unwrap();
        let refused = pins_current(&second, name, digest, &instance).unwrap_err();
        assert!(refused.contains("confirm one here"), "{refused}");
    }
}
