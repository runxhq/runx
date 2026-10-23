use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{JsonObject, Receipt};
use runx_parser::GraphStep;

use crate::CapabilityContract;
use crate::RuntimeError;
use crate::adapter::InvocationOutput;
use crate::credentials::CredentialDelivery;

use super::{EffectAdmission, RuntimeEffectError};

pub struct EffectToolOutput {
    pub value: runx_contracts::JsonValue,
    pub ephemeral: Option<runx_contracts::JsonValue>,
}

impl EffectToolOutput {
    #[must_use]
    pub fn durable(value: runx_contracts::JsonValue) -> Self {
        Self {
            value,
            ephemeral: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum EffectPreparationOutcome {
    Ready(Box<EffectAdmission>),
    /// The call waits on this request to its host.
    Pending {
        reason: String,
        request: Box<runx_contracts::ResolutionRequest>,
    },
}

pub trait RuntimeEffect: Send + Sync {
    fn family(&self) -> &'static str;

    /// Return true only when this effect family owns the step's execution
    /// target. Graph authors never select a family: the runtime supplies the
    /// step and its catalog tool identity.
    fn matches_target(&self, request: EffectStepRequest<'_>) -> bool {
        let _ = request;
        false
    }

    /// Native catalog contracts implemented by this effect family. Keeping the
    /// contract beside the effect prevents the generic runtime from acquiring
    /// provider- or domain-specific tool knowledge while still giving graphs,
    /// managed agents, and operator discovery one typed catalog surface.
    fn capabilities(&self) -> &'static [&'static dyn CapabilityContract] {
        &[]
    }

    fn admit(
        &self,
        request: EffectStepRequest<'_>,
    ) -> Result<Option<EffectAdmission>, RuntimeEffectError> {
        let _ = request;
        Ok(None)
    }

    /// Prepare an admitted effect immediately before dispatch. The effect owner
    /// creates any attempt state and may return a host-resolution request when
    /// this exact act also requires approval.
    fn prepare_execution(
        &self,
        step: &GraphStep,
        admission: EffectAdmission,
        host: &mut dyn crate::Host,
    ) -> Result<EffectPreparationOutcome, RuntimeEffectError> {
        let _ = (step, host);
        Ok(EffectPreparationOutcome::Ready(Box::new(admission)))
    }

    fn prepare_output(&self, request: EffectOutputRequest<'_>) -> Result<(), RuntimeEffectError> {
        let _ = request;
        Ok(())
    }

    fn finalize_output(&self, request: EffectReceiptRequest<'_>) -> Result<(), RuntimeEffectError> {
        let _ = request;
        Ok(())
    }

    /// Keep what the sealed effect leaves behind, and return what settles it
    /// once the step that consumed it is recorded.
    fn persist(
        &self,
        request: EffectReceiptRequest<'_>,
    ) -> Result<Option<crate::kernel::custody::Settlement>, RuntimeEffectError> {
        let _ = request;
        Ok(None)
    }

    /// The authority the step receipt seals for an admission: the grants
    /// and scopes that admitted it and the execution terms derived for it.
    fn authority(
        &self,
        admission: &EffectAdmission,
    ) -> Result<crate::receipts::SealAuthority, RuntimeEffectError> {
        let _ = admission;
        Ok(crate::receipts::SealAuthority::default())
    }

    /// Invoke one catalog tool owned by this effect family.
    fn invoke_tool(
        &self,
        request: EffectToolRequest<'_>,
    ) -> Option<Result<runx_contracts::JsonValue, RuntimeError>> {
        let _ = request;
        None
    }

    /// Split a verified tool result into its durable claim and an immediate
    /// caller-only overlay. The default keeps the complete result durable.
    fn partition_tool_output(
        &self,
        request: EffectToolRequest<'_>,
        output: runx_contracts::JsonValue,
    ) -> Result<EffectToolOutput, RuntimeError> {
        let _ = request;
        Ok(EffectToolOutput::durable(output))
    }
}

#[derive(Clone, Copy)]
pub struct EffectToolRequest<'a> {
    pub tool: &'a str,
    pub observed_at: &'a str,
    pub inputs: &'a JsonObject,
    pub env: &'a BTreeMap<String, String>,
    pub skill_directory: &'a Path,
    pub credential_delivery: &'a CredentialDelivery,
    pub admission: Option<&'a EffectAdmission>,
}

#[derive(Clone, Copy, Debug)]
pub struct EffectStepRequest<'a> {
    pub step: &'a GraphStep,
    pub target: ResolvedEffectTarget<'a>,
    pub inputs: &'a JsonObject,
    pub env: &'a BTreeMap<String, String>,
    pub graph_dir: &'a Path,
    /// The run the step belongs to.
    pub run_id: &'a str,
    pub run_authority: Option<&'a crate::RunAuthority>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResolvedEffectTarget<'a> {
    pub tool: Option<&'a str>,
}

pub struct EffectOutputRequest<'a> {
    pub step: &'a GraphStep,
    pub admission: &'a EffectAdmission,
    pub claim: &'a JsonObject,
    pub output: &'a mut InvocationOutput,
}

pub struct EffectReceiptRequest<'a> {
    pub step: &'a GraphStep,
    pub graph_dir: &'a Path,
    pub admission: &'a EffectAdmission,
    pub claim: &'a JsonObject,
    pub output: &'a mut InvocationOutput,
    pub receipt: &'a Receipt,
    pub env: &'a BTreeMap<String, String>,
    pub signature_policy: crate::receipts::RuntimeReceiptSignaturePolicy<'a>,
}
