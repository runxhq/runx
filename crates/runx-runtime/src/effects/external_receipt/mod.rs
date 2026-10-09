use runx_contracts::AuthorityVerb;
use runx_contracts::JsonValue;
use runx_core::state_machine::AuthorityAdmissionWitness;

use super::EffectToolRequest;
use super::{EffectAdmission, EffectStepRequest, RuntimeEffect, RuntimeEffectError};
use crate::CapabilityContract;
use crate::RuntimeError;

mod contract;
mod execution;

pub const EXTERNAL_RECEIPT_EFFECT_FAMILY: &str = "external_receipt";
pub const EXTERNAL_RECEIPT_VERIFY_TOOL: &str = "external_receipt.verify";

#[derive(Clone, Copy, Debug, Default)]
pub struct ExternalReceiptEffect;

impl RuntimeEffect for ExternalReceiptEffect {
    fn family(&self) -> &'static str {
        EXTERNAL_RECEIPT_EFFECT_FAMILY
    }

    fn matches_target(&self, request: EffectStepRequest<'_>) -> bool {
        request.target.tool_ref == Some(EXTERNAL_RECEIPT_VERIFY_TOOL)
    }

    fn capabilities(&self) -> &'static [&'static dyn CapabilityContract] {
        contract::CAPABILITIES
    }

    fn admit(
        &self,
        request: EffectStepRequest<'_>,
    ) -> Result<Option<EffectAdmission>, RuntimeEffectError> {
        if request.target.tool_ref != Some(EXTERNAL_RECEIPT_VERIFY_TOOL) {
            return Ok(None);
        }
        Ok(Some(EffectAdmission::new(
            EXTERNAL_RECEIPT_EFFECT_FAMILY,
            AuthorityVerb::Read,
            AuthorityAdmissionWitness {
                verb: AuthorityVerb::Read,
                parent_term_id: "external-receipt:workspace".to_owned(),
                child_term_id: format!("external-receipt:{}", request.step.id),
                idempotency_key: None,
                capability_ref: None,
            },
            (),
        )))
    }

    fn invoke_tool(
        &self,
        request: EffectToolRequest<'_>,
    ) -> Option<Result<JsonValue, RuntimeError>> {
        (request.tool_ref == EXTERNAL_RECEIPT_VERIFY_TOOL)
            .then(|| execution::verify_external_receipt(request))
    }
}

#[cfg(test)]
mod tests;
