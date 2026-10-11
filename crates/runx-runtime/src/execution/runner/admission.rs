// Admission owns authority, approval, and effect preconditions before a step
// may dispatch.
use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{AuthorityVerb, JsonObject, Receipt};
use runx_parser::GraphStep;

use crate::RuntimeError;
use crate::adapter::InvocationOutput;
use crate::effects::{
    EffectAdmission, EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest,
    EffectStepRequest, RuntimeEffectError, RuntimeEffectRegistry,
};

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
    step: &GraphStep,
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
    step: &GraphStep,
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
    pub(super) step: &'a GraphStep,
    pub(super) graph_dir: &'a Path,
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
        graph_dir: context.graph_dir,
        admission: &authority.admission,
        claim: context.claim,
        output: context.output,
        receipt: context.receipt,
        env: context.env,
        signature_policy: context.signature_policy,
    }
}

fn runtime_effect_error(step: &GraphStep, source: RuntimeEffectError) -> RuntimeError {
    match source {
        RuntimeEffectError::Denied { verb, message, .. } => authority_denied(step, verb, message),
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
