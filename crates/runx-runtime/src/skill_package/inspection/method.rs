use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{InspectedInput, InspectedOutput, JsonObject, JsonValue, MethodInspection};
use runx_parser::{SkillInput, SkillMethodDefinition, SkillMethodManifest, ValidatedSkillPackage};

use super::SkillInspectionError;

pub(super) fn inspect_method(
    manifest: &SkillMethodManifest,
    method: &SkillMethodDefinition,
    examples: &[JsonObject],
) -> Result<MethodInspection, SkillInspectionError> {
    Ok(MethodInspection {
        name: method.name.clone(),
        input_schema: runx_contracts::input_contract_schema_with_examples(&method.inputs, examples),
        schemas: method
            .schemas()
            .map_err(|error| SkillInspectionError::Json {
                context: "digesting the method's schemas",
                source: serde::ser::Error::custom(error),
            })?,
        inputs: method
            .inputs
            .iter()
            .map(|(name, input)| inspect_input(name, input))
            .collect(),
        outputs: result_steps(method)
            .filter_map(|step| step.outputs.as_ref())
            .flat_map(|outputs| outputs.iter())
            .map(|(name, declaration)| inspect_output(name, declaration))
            .collect(),
        packets: result_packets(method),
        environment: method.environment(),
        connectors: if manifest.connectors.is_empty() {
            None
        } else {
            match JsonValue::from_serialize(&manifest.connectors).map_err(|error| {
                SkillInspectionError::Json {
                    context: "serializing connector requirements",
                    source: serde::ser::Error::custom(error),
                }
            })? {
                JsonValue::Object(connectors) => Some(connectors),
                _ => None,
            }
        },
    })
}

fn result_packets(method: &SkillMethodDefinition) -> BTreeMap<String, runx_contracts::PacketId> {
    let mut packets = BTreeMap::new();
    let mut conflicting = BTreeSet::new();
    // Each result output's packet, by the output's name: a field's, and the
    // whole result's under the name result_as gives it.
    let mut add = |artifacts: &runx_parser::OutputArtifacts| {
        for (name, schema) in &artifacts.packets {
            if packets.get(name).is_some_and(|prior| prior != schema) {
                conflicting.insert(name.clone());
            } else {
                packets.insert(name.clone(), schema.clone());
            }
        }
    };
    for artifacts in method.result_artifacts() {
        add(artifacts);
    }
    for name in conflicting {
        packets.remove(&name);
    }
    packets
}

/// The steps whose outputs form the method's public result.
fn result_steps(
    method: &SkillMethodDefinition,
) -> impl Iterator<Item = &runx_parser::WorkflowStep> {
    method
        .workflow
        .steps
        .iter()
        .filter(|step| method.workflow.result_from.contains(&step.id))
}

fn inspect_input(name: &str, input: &SkillInput) -> InspectedInput {
    InspectedInput {
        name: name.to_owned(),
        input_type: input.input_type.clone(),
        required: input.required,
        description: input.description.clone(),
        schema: input.effective_schema(),
    }
}

fn inspect_output(name: &str, declaration: &JsonValue) -> InspectedOutput {
    let (output_type, required) = match declaration {
        JsonValue::String(kind) => (Some(kind.clone()), None),
        JsonValue::Object(details) => (
            details
                .get("type")
                .and_then(JsonValue::as_str)
                .map(str::to_owned),
            Some(details.get("optional").and_then(JsonValue::as_bool) != Some(true)),
        ),
        _ => (None, None),
    };
    InspectedOutput {
        name: name.to_owned(),
        output_type,
        required,
    }
}

/// The package's tests of `method`, by name, the examples inspection shows.
pub(super) fn fixture_examples(
    package: &ValidatedSkillPackage,
    manifest: Option<&runx_parser::SkillMethodManifest>,
    method: &str,
) -> Vec<String> {
    let default = manifest
        .and_then(SkillMethodManifest::default_method)
        .map(|method| method.name.as_str());
    let mut examples = package
        .test_fixtures
        .values()
        .filter(|test| test.package.is_none())
        .filter(|test| test.method.as_deref().or(default) == Some(method))
        .map(|test| test.name.clone())
        .collect::<Vec<_>>();
    examples.sort();
    examples.dedup();
    examples
}
