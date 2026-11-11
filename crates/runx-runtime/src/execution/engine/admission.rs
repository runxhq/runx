// Admission owns authority, approval, and effect preconditions before a step
// may dispatch.
use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{AuthorityVerb, JsonObject, Receipt};
use runx_parser::WorkflowStep;

use crate::RuntimeError;
use crate::adapter::InvocationOutput;
use crate::adapter::SkillAdapter;
use crate::effects::{
    EffectAdmission, EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest,
    EffectStepRequest, ResolvedEffectTarget, RuntimeEffectError, RuntimeEffectRegistry,
};

/// The refusal a run of `workflow` meets before it starts, if any: each of
/// its own scripts' declared operations admitted from the checks each call
/// of them meets, under the run's authority and the packages its calls are
/// made from, and the first refused refuses the run as its step's admission
/// would. A fault reading what admission reads is an error, not a refusal.
pub(crate) fn declared_operations_refusal<A>(
    runtime: &super::Runtime<A>,
    workflow_dir: &Path,
    workflow: &runx_parser::Workflow,
) -> Result<Option<RuntimeError>, RuntimeError>
where
    A: SkillAdapter,
{
    let declaring = workflow.steps.iter().filter(|step| {
        matches!(&step.kind, runx_parser::StepKind::Script(script) if !script.allowance.operations.is_empty())
    });
    for step in declaring {
        let chain = runtime.options.chain_at(workflow_dir)?;
        let request = EffectStepRequest {
            step,
            target: ResolvedEffectTarget::default(),
            inputs: &step.inputs,
            env: &runtime.options.env,
            workflow_dir,
            run_id: runtime.options.run.run_id(),
            run_authority: runtime.options.run_authority.as_ref(),
            held_by_effects: false,
            chain: &chain,
            clock: &runtime.options.clock,
            reconciliation: None,
        };
        match runtime.options.effects.admit_declared(request) {
            Ok(()) => {}
            Err(
                source @ (RuntimeEffectError::Denied { .. }
                | RuntimeEffectError::Declined { .. }
                | RuntimeEffectError::CostLimitExceeded { .. }),
            ) => return Ok(Some(runtime_effect_error(step, source))),
            Err(source) => return Err(runtime_effect_error(step, source)),
        }
    }
    Ok(None)
}

pub(super) fn enforce_step_authority_admission(
    request: EffectStepRequest<'_>,
    effects: &RuntimeEffectRegistry,
) -> Result<Option<StepAuthorityContext>, RuntimeError> {
    effects
        .admit(request)
        .map(|admission| admission.map(StepAuthorityContext::new))
        .map_err(|source| runtime_effect_error(request.step, source))
}

pub(super) fn prepare_effect_execution(
    request: EffectStepRequest<'_>,
    authority: Option<StepAuthorityContext>,
    host: &mut dyn crate::Host,
    effects: &RuntimeEffectRegistry,
) -> Result<Option<StepAuthorityContext>, RuntimeError> {
    let Some(authority) = authority else {
        return Ok(None);
    };
    let step = request.step;
    match effects.prepare_execution(request, authority.admission, host) {
        Ok(EffectPreparationOutcome::Ready(admission)) => {
            Ok(Some(StepAuthorityContext::new(*admission)))
        }
        Ok(EffectPreparationOutcome::Pending { reason, request }) => {
            Err(RuntimeError::ResolutionPending {
                step_id: step.id.clone(),
                reason,
                request,
                child: None,
            })
        }
        Err(source) => Err(runtime_effect_error(step, source)),
    }
}

pub(super) fn prepare_effect_output_before_gate(
    step: &WorkflowStep,
    authority: Option<&StepAuthorityContext>,
    claim: &JsonObject,
    output: &mut InvocationOutput,
    effects: &RuntimeEffectRegistry,
) -> Result<(), RuntimeError> {
    let Some(authority) = authority else {
        return Ok(());
    };
    effects
        .prepare_output(EffectOutputRequest {
            step,
            admission: &authority.admission,
            claim,
            output,
        })
        .map_err(|source| runtime_effect_error(step, source))
}

pub(super) fn finalize_effect_output_before_success(
    context: EffectReceiptContext<'_>,
) -> Result<(), RuntimeError> {
    let Some(authority) = context.authority else {
        return Ok(());
    };
    let effects = context.effects;
    let step = context.step;
    effects
        .finalize_output(effect_receipt_request(context, authority))
        .map_err(|source| runtime_effect_error(step, source))
}

pub(super) fn persist_effect_state_for_step(
    context: EffectReceiptContext<'_>,
) -> Result<Option<crate::kernel::custody::Settlement>, RuntimeError> {
    let Some(authority) = context.authority else {
        return Ok(None);
    };
    let effects = context.effects;
    let step = context.step;
    effects
        .persist(effect_receipt_request(context, authority))
        .map_err(|source| runtime_effect_error(step, source))
}

pub(super) fn authority_denied(
    step: &WorkflowStep,
    verb: AuthorityVerb,
    reason: String,
) -> RuntimeError {
    RuntimeError::AuthorityDenied {
        verb,
        step_id: step.id.clone(),
        reason,
    }
}

pub(super) struct EffectReceiptContext<'a> {
    pub(super) step: &'a WorkflowStep,
    pub(super) workflow_dir: &'a Path,
    pub(super) authority: Option<&'a StepAuthorityContext>,
    pub(super) claim: &'a JsonObject,
    pub(super) output: &'a mut InvocationOutput,
    pub(super) receipt: &'a Receipt,
    pub(super) env: &'a BTreeMap<String, String>,
    pub(super) signature_policy: crate::receipts::RuntimeReceiptSignaturePolicy<'a>,
    pub(super) effects: &'a RuntimeEffectRegistry,
}

fn effect_receipt_request<'a>(
    context: EffectReceiptContext<'a>,
    authority: &'a StepAuthorityContext,
) -> EffectReceiptRequest<'a> {
    EffectReceiptRequest {
        step: context.step,
        workflow_dir: context.workflow_dir,
        admission: &authority.admission,
        claim: context.claim,
        output: context.output,
        receipt: context.receipt,
        env: context.env,
        signature_policy: context.signature_policy,
    }
}

fn runtime_effect_error(step: &WorkflowStep, source: RuntimeEffectError) -> RuntimeError {
    match source {
        RuntimeEffectError::Denied { verb, message, .. } => authority_denied(step, verb, message),
        RuntimeEffectError::Declined { message, .. } => RuntimeError::Declined {
            step_id: step.id.clone(),
            reason: message,
        },
        RuntimeEffectError::CostLimitExceeded { message, .. } => RuntimeError::CostLimitExceeded {
            step_id: step.id.clone(),
            reason: message,
        },
        RuntimeEffectError::Failed {
            operation, message, ..
        } if operation.contains("state") => RuntimeError::effect_state(operation, message),
        other => RuntimeError::ReceiptInvalid {
            message: other.to_string(),
        },
    }
}

#[derive(Clone, Debug)]
pub(super) struct StepAuthorityContext {
    admission: EffectAdmission,
}

impl StepAuthorityContext {
    fn new(admission: EffectAdmission) -> Self {
        Self { admission }
    }

    pub(super) fn admission_witness(&self) -> &runx_core::state_machine::AuthorityAdmissionWitness {
        self.admission.witness()
    }

    pub(super) fn admission(&self) -> &EffectAdmission {
        &self.admission
    }

    pub(super) fn authority(
        &self,
        effects: &RuntimeEffectRegistry,
    ) -> Result<crate::receipts::SealAuthority, RuntimeError> {
        effects
            .authority(&self.admission)
            .map_err(|source| RuntimeError::ReceiptInvalid {
                message: source.to_string(),
            })
    }
}
