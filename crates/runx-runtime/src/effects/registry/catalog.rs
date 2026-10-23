use std::collections::BTreeSet;

use crate::CapabilityContract;

use super::super::EffectToolRequest;
use super::{RuntimeEffectError, RuntimeEffectRegistry};

impl RuntimeEffectRegistry {
    pub(crate) fn capability(&self, tool: &str) -> Option<&'static dyn CapabilityContract> {
        self.families
            .values()
            .flat_map(|effect| effect.capabilities())
            .copied()
            .find(|capability| capability.definition().id == tool)
    }

    pub(crate) fn capabilities(&self) -> Vec<&'static dyn CapabilityContract> {
        self.families
            .values()
            .flat_map(|effect| effect.capabilities())
            .copied()
            .collect()
    }

    pub(crate) fn invoke_tool(
        &self,
        request: EffectToolRequest<'_>,
    ) -> Option<Result<super::super::EffectToolOutput, crate::RuntimeError>> {
        let effect = self.families.values().find(|effect| {
            effect
                .capabilities()
                .iter()
                .any(|capability| capability.definition().id == request.tool)
        })?;
        let capability = effect
            .capabilities()
            .iter()
            .copied()
            .find(|capability| capability.definition().id == request.tool)?;
        let inputs = match capability.normalize_inputs(request.inputs) {
            Ok(inputs) => inputs,
            Err(error) => return Some(Err(error)),
        };
        let normalized = EffectToolRequest {
            tool: request.tool,
            observed_at: request.observed_at,
            inputs: &inputs,
            env: request.env,
            skill_directory: request.skill_directory,
            credential_delivery: request.credential_delivery,
            admission: request.admission,
        };
        Some(invoke_effect_tool(effect.as_ref(), capability, normalized))
    }

    pub(super) fn capability_owner(&self, tool: &str) -> Option<&'static str> {
        self.families.values().find_map(|effect| {
            effect
                .capabilities()
                .iter()
                .any(|capability| capability.definition().id == tool)
                .then(|| effect.family())
        })
    }
}

fn invoke_effect_tool(
    effect: &dyn super::super::RuntimeEffect,
    capability: &dyn CapabilityContract,
    request: EffectToolRequest<'_>,
) -> Result<super::super::EffectToolOutput, crate::RuntimeError> {
    let tool = request.tool;
    let output = effect.invoke_tool(request).unwrap_or_else(|| {
        Err(crate::RuntimeError::SkillFailed {
            skill_name: tool.to_owned(),
            message: format!(
                "effect family {} declares tool {tool} but does not implement it",
                effect.family()
            ),
        })
    })?;
    capability.validate_output(&output)?;
    effect.partition_tool_output(request, output)
}

pub(super) fn validate_capabilities(
    family: &'static str,
    capabilities: &[&dyn CapabilityContract],
) -> Result<(), RuntimeEffectError> {
    let mut names = BTreeSet::new();
    for capability in capabilities {
        let definition = capability.definition();
        crate::capability::validate_capability_contract(*capability).map_err(|error| {
            RuntimeEffectError::InvalidMetadata {
                family: family.to_owned(),
                message: error.to_string(),
            }
        })?;
        if !names.insert(definition.id) {
            return Err(RuntimeEffectError::InvalidMetadata {
                family: family.to_owned(),
                message: format!("tool {} is declared more than once", definition.id),
            });
        }
    }
    Ok(())
}
