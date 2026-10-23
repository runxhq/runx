use runx_contracts::{JsonObject, JsonValue};

use crate::{
    CapabilityArtifacts, CapabilityContract, CapabilityInput, CapabilityOutput, RuntimeError,
    TypedCapability,
};

use super::NativeInvocation;
use crate::capability::LocalEffect;

pub(super) type TypedNativeHandler<I, O> =
    for<'a> fn(&NativeInvocation<'a, I>) -> Result<O, RuntimeError>;

/// What a call of a tool that changes something would change or run, from
/// the inputs it would run with, without running it; none when this call
/// changes nothing.
pub(super) type TypedEffectDescriber<I> =
    for<'a> fn(&NativeInvocation<'a, I>) -> Result<Option<LocalEffect>, RuntimeError>;

pub(super) trait NativeCapability: CapabilityContract {
    fn invoke(&self, invocation: RawNativeInvocation<'_>) -> Result<JsonValue, RuntimeError>;
    fn execution_boundary(&self) -> runx_contracts::ExecutionBoundaryKind;
    /// What a call with these inputs would change or run; none for a tool
    /// that only reads.
    fn local_effect(
        &self,
        invocation: RawNativeInvocation<'_>,
    ) -> Result<Option<LocalEffect>, RuntimeError>;
    /// Whether the tool describes the effect it has.
    #[cfg(test)]
    fn describes_effect(&self) -> bool;
}

pub(super) fn decode_typed_output<O: CapabilityOutput>(
    tool: &str,
    output: JsonValue,
) -> Result<O, RuntimeError> {
    output
        .deserialize_into::<O>()
        .map_err(|source| RuntimeError::SkillFailed {
            skill_name: tool.to_owned(),
            message: format!("invalid typed output: {source}"),
        })
}

pub(super) struct RawNativeInvocation<'a> {
    pub(super) inputs: JsonObject,
    pub(super) scopes: &'a [String],
    pub(super) observed_at: &'a str,
    pub(super) env: &'a std::collections::BTreeMap<String, String>,
    pub(super) skill_directory: &'a std::path::Path,
    pub(super) credential_delivery: &'a crate::CredentialDelivery,
    pub(super) local_artifacts: &'a crate::services::LocalArtifactService,
    pub(super) effects: &'a crate::RuntimeEffectRegistry,
    pub(super) bounds: crate::services::WorkspaceBounds,
    pub(super) call: Option<&'a crate::tool_catalogs::ToolCall>,
}

pub(super) struct TypedNativeCapability<I, O> {
    contract: TypedCapability<I>,
    handler: TypedNativeHandler<I, O>,
    execution_boundary: runx_contracts::ExecutionBoundaryKind,
    describe: Option<TypedEffectDescriber<I>>,
}

impl<I, O> TypedNativeCapability<I, O> {
    #[must_use]
    pub(super) const fn new(
        definition: crate::CapabilityDefinition,
        handler: TypedNativeHandler<I, O>,
    ) -> Self {
        Self {
            contract: TypedCapability::new(definition),
            handler,
            execution_boundary: runx_contracts::ExecutionBoundaryKind::NativeCapability,
            describe: None,
        }
    }

    #[must_use]
    pub(super) const fn new_with_execution_boundary(
        definition: crate::CapabilityDefinition,
        handler: TypedNativeHandler<I, O>,
        execution_boundary: runx_contracts::ExecutionBoundaryKind,
    ) -> Self {
        Self {
            contract: TypedCapability::new(definition),
            handler,
            execution_boundary,
            describe: None,
        }
    }

    /// The tool changes something, and `describe` says what a call would.
    #[must_use]
    pub(super) const fn describing(self, describe: TypedEffectDescriber<I>) -> Self {
        Self {
            describe: Some(describe),
            ..self
        }
    }
}

impl<I, O> CapabilityContract for TypedNativeCapability<I, O>
where
    I: CapabilityInput,
    O: CapabilityOutput,
{
    fn definition(&self) -> &crate::CapabilityDefinition {
        self.contract.definition()
    }

    fn input_schema(&self) -> Result<serde_json::Value, RuntimeError> {
        self.contract.input_schema()
    }

    fn normalize_inputs(&self, inputs: &JsonObject) -> Result<JsonObject, RuntimeError> {
        self.contract.normalize_inputs(inputs)
    }

    fn defaults(&self) -> JsonObject {
        self.contract.defaults()
    }

    fn output_schema(&self) -> serde_json::Value {
        typed_output_schema::<O>(self.definition().artifacts)
    }

    fn validate_output(&self, output: &JsonValue) -> Result<(), RuntimeError> {
        output
            .clone()
            .deserialize_into::<O>()
            .map(|_| ())
            .map_err(|source| RuntimeError::SkillFailed {
                skill_name: self.definition().id.to_owned(),
                message: format!("invalid typed output: {source}"),
            })
    }
}

impl<I, O> NativeCapability for TypedNativeCapability<I, O>
where
    I: CapabilityInput,
    O: CapabilityOutput,
{
    fn execution_boundary(&self) -> runx_contracts::ExecutionBoundaryKind {
        self.execution_boundary
    }

    #[cfg(test)]
    fn describes_effect(&self) -> bool {
        self.describe.is_some()
    }

    fn local_effect(
        &self,
        invocation: RawNativeInvocation<'_>,
    ) -> Result<Option<LocalEffect>, RuntimeError> {
        if !self.definition().effect.mutating() {
            return Ok(None);
        }
        let describe = self.describe.ok_or_else(|| RuntimeError::EngineInvariant {
            context: "describing a local effect",
            message: format!(
                "{} changes something but describes no effect",
                self.definition().id
            ),
        })?;
        let inputs = self.contract.decode_inputs(invocation.inputs)?;
        describe(&NativeInvocation {
            inputs: &inputs,
            observed_at: invocation.observed_at,
            env: invocation.env,
            skill_directory: invocation.skill_directory,
            credential_delivery: invocation.credential_delivery,
            local_artifacts: invocation.local_artifacts,
            effects: invocation.effects,
            bounds: invocation.bounds,
            call: invocation.call,
        })
    }

    fn invoke(&self, invocation: RawNativeInvocation<'_>) -> Result<JsonValue, RuntimeError> {
        crate::capability::enforce_required_scopes(
            self.definition().id,
            self.definition().scopes.iter().copied(),
            invocation.scopes,
        )?;
        let inputs = self.contract.decode_inputs(invocation.inputs)?;
        let invocation = NativeInvocation {
            inputs: &inputs,
            observed_at: invocation.observed_at,
            env: invocation.env,
            skill_directory: invocation.skill_directory,
            credential_delivery: invocation.credential_delivery,
            local_artifacts: invocation.local_artifacts,
            effects: invocation.effects,
            bounds: invocation.bounds,
            call: invocation.call,
        };
        let output = (self.handler)(&invocation)?;
        serde_json::to_value(output)
            .and_then(serde_json::from_value)
            .map_err(|source| {
                RuntimeError::json(
                    format!(
                        "serializing native capability {} output",
                        self.definition().id
                    ),
                    source,
                )
            })
    }
}

fn typed_output_schema<O: CapabilityOutput>(artifacts: CapabilityArtifacts) -> serde_json::Value {
    let mut schema = O::json_schema();
    let Some(object) = schema.as_object_mut() else {
        return schema;
    };
    match artifacts {
        CapabilityArtifacts::None => {}
        CapabilityArtifacts::Named { output, packet } => {
            if let Some(property) = object
                .get_mut("properties")
                .and_then(serde_json::Value::as_object_mut)
                .and_then(|properties| properties.get_mut(output))
                .and_then(serde_json::Value::as_object_mut)
            {
                property.insert(
                    "x-runx-packet".to_owned(),
                    serde_json::Value::String(packet.to_owned()),
                );
            }
        }
        CapabilityArtifacts::Wrapped { packet, .. } => {
            object.insert(
                "x-runx-packet".to_owned(),
                serde_json::Value::String(packet.to_owned()),
            );
        }
    }
    schema
}
