use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{
    InspectedInput, InspectedOutput, JsonObject, JsonValue, OperatorJourney, RunnerInspection,
    SkillCapabilities,
};
use runx_parser::{
    CatalogMetadata, SkillInput, SkillRunnerDefinition, SkillRunnerManifest, ValidatedSkillPackage,
};

use super::SkillInspectionError;

pub(super) fn inspect_runner(
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    examples: &[JsonObject],
) -> Result<RunnerInspection, SkillInspectionError> {
    Ok(RunnerInspection {
        name: runner.name.clone(),
        input_schema: runx_contracts::input_contract_schema_with_examples(&runner.inputs, examples),
        inputs: runner
            .inputs
            .iter()
            .map(|(name, input)| inspect_input(name, input))
            .collect(),
        outputs: result_steps(runner)
            .filter_map(|step| step.outputs.as_ref())
            .flat_map(|outputs| outputs.iter())
            .map(|(name, declaration)| inspect_output(name, declaration))
            .collect(),
        declared_result_packets: declared_result_packets(runner),
        requirements: manifest.execution_requirements(runner),
        scopes: runner.declared_scopes(),
        continuations: runner.continuations.clone(),
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

fn declared_result_packets(
    runner: &SkillRunnerDefinition,
) -> BTreeMap<String, runx_contracts::PacketId> {
    let mut packets = BTreeMap::new();
    let mut conflicting = BTreeSet::new();
    let mut add = |artifacts: &runx_parser::SkillArtifactContract| {
        if let (Some(named), Some(schemas)) = (&artifacts.named_emits, &artifacts.packets) {
            for (name, schema) in schemas {
                if !named.contains_key(name) {
                    continue;
                }
                if packets.get(name).is_some_and(|prior| prior != schema) {
                    conflicting.insert(name.clone());
                } else {
                    packets.insert(name.clone(), schema.clone());
                }
            }
        }
    };
    for artifacts in runner.result_artifacts() {
        add(artifacts);
    }
    for name in conflicting {
        packets.remove(&name);
    }
    packets
}

/// The steps whose outputs form the runner's public result.
fn result_steps(runner: &SkillRunnerDefinition) -> impl Iterator<Item = &runx_parser::GraphStep> {
    runner
        .graph
        .steps
        .iter()
        .filter(|step| runner.graph.result_from.contains(&step.id))
}

pub(super) fn catalog_capabilities(catalog: &CatalogMetadata) -> Option<SkillCapabilities> {
    Some(SkillCapabilities {
        execution: catalog.execution?.as_str().to_owned(),
        completion: catalog.completion?.as_str().to_owned(),
        approval: catalog.approval?.as_str().to_owned(),
    })
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
            details.get("required").and_then(JsonValue::as_bool),
        ),
        _ => (None, None),
    };
    InspectedOutput {
        name: name.to_owned(),
        output_type,
        required,
    }
}

pub(super) fn fixture_examples(
    package: &ValidatedSkillPackage,
    manifest: Option<&runx_parser::SkillRunnerManifest>,
    runner: &str,
) -> Vec<String> {
    let mut examples = manifest
        .and_then(|manifest| manifest.harness.as_ref())
        .into_iter()
        .flat_map(|harness| harness.cases.iter())
        .filter(|case| case.runner.as_deref().is_none_or(|name| name == runner))
        .map(|case| case.name.clone())
        .chain(
            package
                .source
                .files
                .keys()
                .filter(|path| path.starts_with("fixtures/") && path.ends_with(".yaml"))
                .cloned(),
        )
        .collect::<Vec<_>>();
    examples.sort();
    examples.dedup();
    examples
}

pub(super) fn fixture_operator_journeys(
    package: &ValidatedSkillPackage,
    manifest: &SkillRunnerManifest,
) -> Vec<OperatorJourney> {
    let mut journeys = manifest
        .harness
        .iter()
        .flat_map(|harness| harness.cases.iter())
        .flat_map(|case| {
            case.operator_journeys.iter().map(|journey| {
                project_operator_journey(&case.name, case.runner.as_deref(), journey)
            })
        })
        .chain(package.harness_fixtures.values().flat_map(|fixture| {
            fixture.operator_journeys.iter().map(|journey| {
                project_operator_journey(&fixture.name, fixture.runner.as_deref(), journey)
            })
        }))
        .collect::<Vec<_>>();
    journeys.sort_by(|left, right| left.case.cmp(&right.case));
    journeys
}

fn project_operator_journey(
    case: &str,
    runner: Option<&str>,
    journey: &runx_parser::OperatorJourneyClaim,
) -> OperatorJourney {
    OperatorJourney {
        case: case.to_owned(),
        mode: match journey.mode {
            runx_parser::OperatorJourneyMode::Standalone => "standalone",
            runx_parser::OperatorJourneyMode::Composed => "composed",
            runx_parser::OperatorJourneyMode::Refusal => "refusal",
        }
        .to_owned(),
        request: journey.request.clone(),
        expected_outcome: journey.expected_outcome.clone(),
        confusors: journey.confusors.clone(),
        prior_evidence: journey.prior_evidence.clone(),
        must_not_repeat: journey.must_not_repeat.clone(),
        runner: runner.map(str::to_owned),
        exercises_runner: journey.exercises_runner.clone(),
    }
}
