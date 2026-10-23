//! One typed contract for runtime-owned and effect-owned capabilities.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use runx_contracts::schema::RunxSchema;
use runx_contracts::tools::ToolInput;
use runx_contracts::{JsonObject, JsonValue};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::RuntimeError;

mod schema;

use schema::{
    catalog_inputs, decode_inputs, effective_schema, normalize_inputs, output_schema,
    schema_properties, validate_output,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityEffect {
    Read,
    Mutate,
}

impl CapabilityEffect {
    #[must_use]
    pub const fn mutating(self) -> bool {
        matches!(self, Self::Mutate)
    }
}

/// What a step that changes the workspace or runs a process would do, as the
/// person who approves it is shown it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LocalEffect {
    /// What executes, carried on its approval's gate.
    pub(crate) payload: JsonValue,
    /// The workspace-relative files it changes.
    pub(crate) paths: Vec<String>,
    /// The argv it runs.
    pub(crate) argv: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityApproval {
    None,
    Effect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityArtifacts {
    None,
    Named {
        output: &'static str,
        packet: &'static str,
    },
    Wrapped {
        output: &'static str,
        packet: &'static str,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityField {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityDefinition {
    pub id: &'static str,
    pub source: &'static str,
    pub summary: &'static str,
    pub scopes: &'static [&'static str],
    pub effect: CapabilityEffect,
    pub approval: CapabilityApproval,
    pub artifacts: CapabilityArtifacts,
    pub fields: &'static [CapabilityField],
}

pub trait CapabilityInput:
    Serialize + DeserializeOwned + RunxSchema + Send + Sync + 'static
{
    fn defaults() -> JsonObject {
        JsonObject::new()
    }
}

pub trait CapabilityOutput:
    Serialize + DeserializeOwned + RunxSchema + Send + Sync + 'static
{
}

pub trait CapabilityContract: Send + Sync {
    fn definition(&self) -> &CapabilityDefinition;
    fn input_schema(&self) -> Result<serde_json::Value, RuntimeError>;
    fn normalize_inputs(&self, inputs: &JsonObject) -> Result<JsonObject, RuntimeError>;
    fn defaults(&self) -> JsonObject;

    fn output_schema(&self) -> serde_json::Value {
        output_schema(self.definition().artifacts)
    }

    fn validate_output(&self, output: &JsonValue) -> Result<(), RuntimeError> {
        validate_output(self.definition(), output)
    }

    fn catalog_inputs(&self) -> Result<BTreeMap<String, ToolInput>, RuntimeError> {
        catalog_inputs(self.definition(), &self.input_schema()?, &self.defaults())
    }
}

pub(crate) fn enforce_required_scopes<'a>(
    operation: &str,
    required: impl IntoIterator<Item = &'a str>,
    declared: &[String],
) -> Result<(), RuntimeError> {
    let declared = declared.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let missing = required
        .into_iter()
        .filter(|scope| !declared.contains(scope))
        .collect::<BTreeSet<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    Err(RuntimeError::SkillFailed {
        skill_name: operation.to_owned(),
        message: format!(
            "missing required scope declaration(s): {}",
            missing.into_iter().collect::<Vec<_>>().join(", ")
        ),
    })
}

pub struct TypedCapability<I> {
    definition: CapabilityDefinition,
    marker: PhantomData<fn() -> I>,
}

impl<I> TypedCapability<I> {
    #[must_use]
    pub const fn new(definition: CapabilityDefinition) -> Self {
        Self {
            definition,
            marker: PhantomData,
        }
    }
}

impl<I> TypedCapability<I>
where
    I: CapabilityInput,
{
    pub(crate) fn decode_inputs(&self, inputs: JsonObject) -> Result<I, RuntimeError> {
        decode_inputs::<I>(&self.definition, inputs)
    }
}

impl<I> CapabilityContract for TypedCapability<I>
where
    I: CapabilityInput,
{
    fn definition(&self) -> &CapabilityDefinition {
        &self.definition
    }

    fn input_schema(&self) -> Result<serde_json::Value, RuntimeError> {
        effective_schema::<I>(&self.definition)
    }

    fn normalize_inputs(&self, inputs: &JsonObject) -> Result<JsonObject, RuntimeError> {
        normalize_inputs::<I>(&self.definition, inputs)
    }

    fn defaults(&self) -> JsonObject {
        I::defaults()
    }
}

pub(crate) fn validate_capability_contract(
    contract: &dyn CapabilityContract,
) -> Result<(), RuntimeError> {
    let definition = contract.definition();
    validate_capability_identity(definition)?;
    validate_capability_approval(definition)?;
    validate_capability_artifacts(definition)?;
    validate_capability_fields(definition, &contract.input_schema()?)?;
    validate_capability_output_schema(definition, &contract.output_schema())?;
    contract.catalog_inputs()?;
    Ok(())
}

fn validate_capability_approval(definition: &CapabilityDefinition) -> Result<(), RuntimeError> {
    if definition.approval == CapabilityApproval::Effect && !definition.effect.mutating() {
        return Err(invalid(
            definition.id,
            "effect-owned approval is valid only for a mutating capability",
        ));
    }
    Ok(())
}

fn validate_capability_output_schema(
    definition: &CapabilityDefinition,
    schema: &serde_json::Value,
) -> Result<(), RuntimeError> {
    let object = schema
        .as_object()
        .ok_or_else(|| invalid(definition.id, "typed output schema must be an object"))?;
    match definition.artifacts {
        CapabilityArtifacts::None => Ok(()),
        CapabilityArtifacts::Named { output, packet } => {
            let property = object
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .and_then(|properties| properties.get(output))
                .ok_or_else(|| {
                    invalid(
                        definition.id,
                        format!("typed output schema is missing named packet {output:?}"),
                    )
                })?;
            if property
                .get("x-runx-packet")
                .and_then(serde_json::Value::as_str)
                != Some(packet)
            {
                return Err(invalid(
                    definition.id,
                    format!("typed output schema does not bind packet {packet:?}"),
                ));
            }
            Ok(())
        }
        CapabilityArtifacts::Wrapped { packet, .. } => {
            if object
                .get("x-runx-packet")
                .and_then(serde_json::Value::as_str)
                != Some(packet)
            {
                return Err(invalid(
                    definition.id,
                    format!("typed output schema does not bind packet {packet:?}"),
                ));
            }
            Ok(())
        }
    }
}

fn validate_capability_identity(definition: &CapabilityDefinition) -> Result<(), RuntimeError> {
    for (field, value) in [
        ("id", definition.id),
        ("source", definition.source),
        ("summary", definition.summary),
    ] {
        if value.trim().is_empty() {
            return Err(invalid(definition.id, format!("{field} must not be empty")));
        }
    }
    Ok(())
}

fn validate_capability_artifacts(definition: &CapabilityDefinition) -> Result<(), RuntimeError> {
    match definition.artifacts {
        CapabilityArtifacts::None => {}
        CapabilityArtifacts::Named { output, .. } | CapabilityArtifacts::Wrapped { output, .. }
            if output.trim().is_empty() =>
        {
            return Err(invalid(definition.id, "output name must not be empty"));
        }
        CapabilityArtifacts::Named { packet, .. } | CapabilityArtifacts::Wrapped { packet, .. } => {
            if !is_core_packet_id(packet) {
                return Err(invalid(
                    definition.id,
                    "a native capability's packet is a core packet id, runx.<name>.v<N>",
                ));
            }
        }
    }
    Ok(())
}

/// A native capability is the kernel's, so the packet it emits is core.
fn is_core_packet_id(packet: &str) -> bool {
    runx_contracts::PacketId::new(packet).is_some_and(|packet| packet.is_core())
}

fn validate_capability_fields(
    definition: &CapabilityDefinition,
    schema: &serde_json::Value,
) -> Result<(), RuntimeError> {
    let properties = schema_properties(definition.id, schema)?;
    let mut described = BTreeSet::new();
    for field in definition.fields {
        if !properties.contains_key(field.name) {
            return Err(invalid(
                definition.id,
                format!("description names unknown input {}", field.name),
            ));
        }
        if field.description.trim().is_empty() || !described.insert(field.name) {
            return Err(invalid(
                definition.id,
                format!("input {} has an empty or duplicate description", field.name),
            ));
        }
    }
    if described.len() != properties.len() {
        let missing = properties
            .keys()
            .filter(|name| !described.contains(name.as_str()))
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(invalid(
            definition.id,
            format!("typed inputs are missing descriptions: {missing}"),
        ));
    }
    Ok(())
}

fn invalid(id: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: if id.is_empty() {
            "native-capability".to_owned()
        } else {
            id.to_owned()
        },
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::is_core_packet_id;

    #[test]
    fn a_native_capability_emits_core_packets() {
        for packet in [
            "runx.attestation.v1",
            "runx.skill.apply_result.v1",
            "runx.process.list.v12",
        ] {
            assert!(is_core_packet_id(packet), "{packet}");
        }
        for packet in [
            "attestation.v1",
            "mock.deployment.v1",
            "runx/twitter.plan.v1",
            "runx.scope-bounds.subset.v1",
            "runx.v1",
            "runx.Attestation.v1",
            "runx.attestation",
            "runx.attestation.v0",
            "runx.attestation.latest",
        ] {
            assert!(!is_core_packet_id(packet), "{packet}");
        }
    }
}
