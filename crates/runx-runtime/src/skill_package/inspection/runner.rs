use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{JsonObject, JsonValue};
use runx_parser::{
    CatalogMetadata, SkillInput, SkillRunnerDefinition, SkillRunnerManifest, ValidatedSkillPackage,
};

use super::SkillInspectionError;

pub(super) fn inspect_runner(
    manifest: &SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    examples: &[JsonObject],
) -> Result<JsonValue, SkillInspectionError> {
    let mut output = JsonObject::from([
        ("name".to_owned(), JsonValue::String(runner.name.clone())),
        (
            "input_schema".to_owned(),
            JsonValue::Object(runx_contracts::input_contract_schema_with_examples(
                &runner.inputs,
                examples,
            )),
        ),
        (
            "inputs".to_owned(),
            JsonValue::Array(
                runner
                    .inputs
                    .iter()
                    .map(|(name, input)| inspect_input(name, input))
                    .collect(),
            ),
        ),
        (
            "outputs".to_owned(),
            JsonValue::Array(
                result_steps(runner)
                    .filter_map(|step| step.outputs.as_ref())
                    .flat_map(|outputs| outputs.iter())
                    .map(|(name, declaration)| inspect_output(name, declaration))
                    .collect(),
            ),
        ),
    ]);
    output.insert(
        "declared_result_packets".to_owned(),
        JsonValue::Object(declared_result_packets(runner)),
    );
    output.insert(
        "requirements".to_owned(),
        serde_json::to_value(manifest.execution_requirements(runner))
            .and_then(serde_json::from_value)
            .map_err(|source| SkillInspectionError::Json {
                context: "serializing runner requirements",
                source,
            })?,
    );
    insert_runner_contract_metadata(&mut output, runner);
    if !manifest.connectors.is_empty() {
        output.insert(
            "connectors".to_owned(),
            JsonValue::from_serialize(&manifest.connectors).map_err(|error| {
                SkillInspectionError::Json {
                    context: "serializing connector requirements",
                    source: serde::ser::Error::custom(error),
                }
            })?,
        );
    }
    Ok(JsonValue::Object(output))
}

fn declared_result_packets(runner: &SkillRunnerDefinition) -> JsonObject {
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
        .into_iter()
        .map(|(name, schema)| (name, JsonValue::String(schema)))
        .collect()
}

/// The steps whose outputs form the runner's public result.
fn result_steps(runner: &SkillRunnerDefinition) -> impl Iterator<Item = &runx_parser::GraphStep> {
    runner
        .graph
        .steps
        .iter()
        .filter(|step| runner.graph.result_from.contains(&step.id))
}

fn insert_runner_contract_metadata(output: &mut JsonObject, runner: &SkillRunnerDefinition) {
    let scopes = runner.declared_scopes();
    if !scopes.is_empty() {
        output.insert(
            "scopes".to_owned(),
            JsonValue::Array(scopes.into_iter().map(JsonValue::String).collect()),
        );
    }
}

pub(super) fn catalog_capabilities(catalog: &CatalogMetadata) -> Option<JsonValue> {
    Some(JsonValue::Object(JsonObject::from([
        (
            "execution".to_owned(),
            JsonValue::String(catalog.execution?.as_str().to_owned()),
        ),
        (
            "completion".to_owned(),
            JsonValue::String(catalog.completion?.as_str().to_owned()),
        ),
        (
            "approval".to_owned(),
            JsonValue::String(catalog.approval?.as_str().to_owned()),
        ),
    ])))
}

fn inspect_input(name: &str, input: &SkillInput) -> JsonValue {
    let mut output = JsonObject::from([
        ("name".to_owned(), JsonValue::String(name.to_owned())),
        (
            "type".to_owned(),
            JsonValue::String(input.input_type.clone()),
        ),
        ("required".to_owned(), JsonValue::Bool(input.required)),
    ]);
    if let Some(description) = &input.description {
        output.insert(
            "description".to_owned(),
            JsonValue::String(description.clone()),
        );
    }
    output.insert(
        "schema".to_owned(),
        JsonValue::Object(input.effective_schema()),
    );
    JsonValue::Object(output)
}

fn inspect_output(name: &str, declaration: &JsonValue) -> JsonValue {
    let mut output = JsonObject::from([("name".to_owned(), JsonValue::String(name.to_owned()))]);
    match declaration {
        JsonValue::String(kind) => {
            output.insert("type".to_owned(), JsonValue::String(kind.clone()));
        }
        JsonValue::Object(details) => {
            if let Some(kind) = details.get("type").and_then(JsonValue::as_str) {
                output.insert("type".to_owned(), JsonValue::String(kind.to_owned()));
            }
            if let Some(required) = details.get("required").and_then(JsonValue::as_bool) {
                output.insert("required".to_owned(), JsonValue::Bool(required));
            }
        }
        _ => {}
    }
    JsonValue::Object(output)
}

pub(super) fn fixture_examples(
    package: &ValidatedSkillPackage,
    manifest: Option<&runx_parser::SkillRunnerManifest>,
    runner: &str,
) -> Vec<JsonValue> {
    let mut examples = manifest
        .and_then(|manifest| manifest.harness.as_ref())
        .into_iter()
        .flat_map(|harness| harness.cases.iter())
        .filter(|case| case.runner.as_deref().is_none_or(|name| name == runner))
        .map(|case| JsonValue::String(case.name.clone()))
        .chain(
            package
                .source
                .files
                .keys()
                .filter(|path| path.starts_with("fixtures/") && path.ends_with(".yaml"))
                .cloned()
                .map(JsonValue::String),
        )
        .collect::<Vec<_>>();
    examples.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    examples.dedup();
    examples
}

pub(super) fn fixture_operator_journeys(
    package: &ValidatedSkillPackage,
    manifest: &SkillRunnerManifest,
) -> Vec<JsonValue> {
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
    journeys.sort_by(|left, right| {
        left.as_object()
            .and_then(|value| value.get("case"))
            .and_then(JsonValue::as_str)
            .cmp(
                &right
                    .as_object()
                    .and_then(|value| value.get("case"))
                    .and_then(JsonValue::as_str),
            )
    });
    journeys
}

fn project_operator_journey(
    case: &str,
    runner: Option<&str>,
    journey: &runx_parser::OperatorJourneyClaim,
) -> JsonValue {
    let mode = match journey.mode {
        runx_parser::OperatorJourneyMode::Standalone => "standalone",
        runx_parser::OperatorJourneyMode::Composed => "composed",
        runx_parser::OperatorJourneyMode::Refusal => "refusal",
    };
    let mut object = JsonObject::from([
        ("case".to_owned(), JsonValue::String(case.to_owned())),
        ("mode".to_owned(), JsonValue::String(mode.to_owned())),
        (
            "request".to_owned(),
            JsonValue::String(journey.request.clone()),
        ),
        (
            "expected_outcome".to_owned(),
            JsonValue::String(journey.expected_outcome.clone()),
        ),
        (
            "confusors".to_owned(),
            JsonValue::Array(
                journey
                    .confusors
                    .iter()
                    .cloned()
                    .map(JsonValue::String)
                    .collect(),
            ),
        ),
        (
            "prior_evidence".to_owned(),
            JsonValue::Array(
                journey
                    .prior_evidence
                    .iter()
                    .cloned()
                    .map(JsonValue::String)
                    .collect(),
            ),
        ),
        (
            "must_not_repeat".to_owned(),
            JsonValue::Array(
                journey
                    .must_not_repeat
                    .iter()
                    .cloned()
                    .map(JsonValue::String)
                    .collect(),
            ),
        ),
    ]);
    if let Some(runner) = runner {
        object.insert("runner".to_owned(), JsonValue::String(runner.to_owned()));
    }
    if let Some(exercises_runner) = &journey.exercises_runner {
        object.insert(
            "exercises_runner".to_owned(),
            JsonValue::String(exercises_runner.clone()),
        );
    }
    JsonValue::Object(object)
}
