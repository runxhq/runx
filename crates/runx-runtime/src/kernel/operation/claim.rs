//! A write's claim (the loop contract's E7): one event appended to the
//! calling package's own store at the version the caller decided at, made
//! after the person's approval and just before the act is attempted. A key
//! that moved refuses the act before anything is sent, so a decision made on
//! a record that changed since is never carried out.
//!
//! The claim is an act of its own, the package's `store/events.append`,
//! admitted as every store call is: its stream is the package's own, placed
//! by the run's scope, its uses count against any bound on the run's store
//! authority, it's kept in custody and run through the kernel's one path,
//! and it settles with its write. Its key is the write's act, its operation
//! on one instance's target under the caller's key, at the version decided
//! at: recovering a write whose claim landed takes the claim as made, a
//! decision made again at a new version is a new claim, and a decision
//! already claimed is never carried out by another act, as one rebound to
//! another instance would be.

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::{Implementation, OperationAccess, OperationName};
use runx_contracts::{JsonObject, JsonValue, OperationIntent};

use super::call::OperationCall;
use super::{AttemptPlan, OperationAdmission, OperationEffect, Tools, execute, invalid, state};
use crate::effects::{EffectStepRequest, RuntimeEffect, RuntimeEffectError};
use crate::kernel::admission::AdmissionDecision;

/// Why a claim stopped its write.
pub(crate) enum Refusal {
    /// The key moved since the caller read it: the write's decision no
    /// longer stands.
    Moved(String),
    /// The store couldn't take the claim; nothing was sent.
    Failed(String),
}

/// The step a claim runs as: the package's own `store/events.append` of the
/// claim's event, under the act of `write`, the call's authorized intent, at
/// the version decided at. None when the call claims nothing; only a step
/// that names the store's append claims.
pub(super) fn claim_step(
    request: EffectStepRequest<'_>,
    target: &runx_parser::OperationStepTarget,
    call: &OperationCall,
    write: &OperationIntent,
) -> Result<Option<runx_parser::WorkflowStep>, RuntimeEffectError> {
    let Some(claim) = &call.claim else {
        return Ok(None);
    };
    let declared = target.claims.clone().ok_or_else(|| {
        invalid(format!(
            "a claim is the package's own {}/{}, which the step doesn't name",
            runx_parser::CLAIM_REQUIREMENT,
            runx_parser::CLAIM_OPERATION
        ))
    })?;
    let act = crate::kernel::custody::act(write)
        .ok_or_else(|| invalid("a claim is made by a write, under the write's key"))?;
    Ok(Some(runx_parser::WorkflowStep {
        id: format!("{}.claim", request.step.id),
        kind: runx_parser::StepKind::Operation(runx_parser::OperationStepTarget {
            requirement: runx_parser::CLAIM_REQUIREMENT.to_owned(),
            operation: OperationName::new(runx_parser::CLAIM_OPERATION.to_owned()),
            from: Vec::new(),
            declared: Some(declared),
            loaded: None,
            claims: None,
        }),
        artifacts: None,
        outputs: None,
        inputs: JsonObject::from([
            ("inputs".to_owned(), JsonValue::Object(claim.inputs())),
            (
                "idempotency_key".to_owned(),
                JsonValue::String(format!("claim:{act}@{}", claim.expected_version)),
            ),
        ]),
        bindings: Vec::new(),
        act: None,
        context_skills: Vec::new(),
        when: None,
        guard: Vec::new(),
        retry_unknown_outcomes: false,
        effects: None,
        reach: None,
    }))
}

impl OperationEffect {
    /// The claim `call` makes, admitted as its package's own store append
    /// under the run's chain and authority and ready to run, or none when it
    /// makes none.
    pub(super) fn claim(
        &self,
        request: EffectStepRequest<'_>,
        target: &runx_parser::OperationStepTarget,
        call: &OperationCall,
        write: &OperationIntent,
    ) -> Result<Option<Box<OperationAdmission>>, RuntimeEffectError> {
        let Some(step) = claim_step(request, target, call, write)? else {
            return Ok(None);
        };
        // The package's own state is never held for a person, so the claim
        // never waits at a gate.
        let admitted = self
            .admit(EffectStepRequest {
                step: &step,
                inputs: &step.inputs,
                held_by_effects: false,
                ..request
            })?
            .ok_or_else(|| state("the claim's append is an operation"))?;
        let mut context = admitted
            .context::<OperationAdmission>()
            .cloned()
            .ok_or_else(|| state("the claim's admission is missing"))?;
        let operation = context.admitted.operation();
        if operation.access() != OperationAccess::Mutate
            || !matches!(operation.implementation, Some(Implementation::Store(_)))
        {
            return Err(invalid("a claim is an append to the package's own store"));
        }
        if context.admitted.admission.decision != AdmissionDecision::Admit {
            return Err(invalid(
                "a claim is the package's own append, which never waits on a person",
            ));
        }
        context.attempt = Some(AttemptPlan {
            number: context
                .admitted
                .pending
                .as_ref()
                .map_or(1, |pending| pending.attempt + 1),
            approval: None,
            evidence_refs: Vec::new(),
        });
        Ok(Some(Box::new(context)))
    }
}

/// Make the claim, through the kernel's one path for every call: committed,
/// or replayed because this decision's claim landed before, lets the write go
/// on; a key that moved refuses it.
pub(super) fn make(
    claim: &OperationAdmission,
    step_id: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    tools: &dyn Tools,
) -> Result<(), Refusal> {
    let outcome = execute(
        claim,
        &format!("{step_id}.claim"),
        env,
        cwd,
        tools,
        &mut None,
    )
    .map_err(|error| Refusal::Failed(format!("the claim couldn't be made: {error}")))?;
    let result = outcome
        .value
        .as_object()
        .and_then(|value| value.get("provider_operation"))
        .and_then(JsonValue::as_object)
        .and_then(|packet| packet.get("result"))
        .and_then(JsonValue::as_object);
    let field = |name: &str| result.and_then(|result| result.get(name));
    let status = field("status").and_then(JsonValue::as_str);
    if matches!(status, Some("committed" | "replayed")) {
        return Ok(());
    }
    let inputs = &claim.call.inputs;
    let text = |name: &str| inputs.get(name).and_then(JsonValue::as_str).unwrap_or("");
    let expected = inputs
        .get("expected_version")
        .and_then(JsonValue::as_u64)
        .unwrap_or_default();
    Err(Refusal::Moved(format!(
        "the claim found {} in {} {}, not at version {expected}, which the write was decided at, so nothing was sent",
        text("key"),
        claim.admitted.target,
        match (status, field("version").and_then(JsonValue::as_u64)) {
            (Some("version_conflict"), Some(version)) => format!("at version {version}"),
            (Some(status), _) => format!("answering {status}"),
            (None, _) => "answering nothing".to_owned(),
        },
    )))
}
