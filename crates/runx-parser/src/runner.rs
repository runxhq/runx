use std::collections::BTreeMap;

use runx_contracts::{
    AllowedOperation, ExecutionRequirements, JsonObject, JsonValue, PaidSkillFixedOfferTerms,
    PaidSkillOfferTerms, PaidSkillPreparedOfferTerms, TurnAllowance,
};
use serde::{Deserialize, Serialize};

use crate::skill::{
    CatalogMetadata, ConnectorRequirement, RunnerHarnessManifest, SkillRunnerDefinition,
    bind_operation_steps, validate_catalog_metadata, validate_connector_requirements,
    validate_harness_manifest, validate_inputs, validate_runner_definition,
};
use crate::{
    ParseError, ValidationError, assert_execution_profile_yaml_subset, json_fields::JsonFieldReader,
};

const FIELDS: JsonFieldReader = JsonFieldReader::new("runner_manifest");
const MANIFEST_FIELDS: &[&str] = &[
    "skill",
    "version",
    "catalog",
    "connectors",
    "tools",
    "skills",
    "asks",
    "input_definitions",
    "marketplace",
    "runners",
    "harness",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawRunnerManifestIr {
    pub document: JsonObject,
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillRunnerManifest {
    /// The skill's name; it must match SKILL.md and names every runner's
    /// graph.
    pub skill: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogMetadata>,
    /// The connector operations this skill needs, by requirement key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connectors: BTreeMap<String, ConnectorRequirement>,
    /// The catalog tools an agent turn may call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// The child skills an agent turn may run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Whether an agent turn may ask the human.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asks: bool,
    /// Manifest-local reusable input declarations. Runner references are
    /// expanded during parsing, so runtime consumers never resolve them again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_definitions: BTreeMap<String, crate::skill::SkillInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<MarketplaceManifest>,
    pub runners: BTreeMap<String, SkillRunnerDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<RunnerHarnessManifest>,
    pub raw: RawRunnerManifestIr,
}

/// Optional commercial terms for ordinary skill runners. The registry owns
/// seller identity and immutable listing resolution; this parser-owned value
/// contains author declarations only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceManifest {
    pub offers: BTreeMap<String, PaidSkillOfferTerms>,
}

impl SkillRunnerManifest {
    /// Return the exact non-secret requirements selected by one runner.
    ///
    /// This is the parser-owned projection used by inspection and execution;
    /// downstream consumers must not rescan raw YAML or normalize scope values.
    #[must_use]
    pub fn execution_requirements(&self, runner: &SkillRunnerDefinition) -> ExecutionRequirements {
        ExecutionRequirements {
            scopes: runner.declared_scopes(),
            environment: runner.environment(),
        }
    }
}

pub fn parse_runner_manifest_yaml(yaml: &str) -> Result<RawRunnerManifestIr, ParseError> {
    assert_execution_profile_yaml_subset("runner_manifest", yaml)?;
    let parsed: JsonValue =
        serde_norway::from_str(yaml).map_err(|error| ParseError::InvalidYaml {
            field: "runner_manifest".to_owned(),
            message: error.to_string(),
        })?;
    let JsonValue::Object(document) = parsed else {
        return Err(ParseError::InvalidDocument {
            field: "runner_manifest".to_owned(),
            message: "Runner manifest YAML must parse to an object.".to_owned(),
        });
    };
    Ok(RawRunnerManifestIr {
        document,
        raw: yaml.to_owned(),
    })
}

pub fn validate_runner_manifest(
    raw: RawRunnerManifestIr,
) -> Result<SkillRunnerManifest, ValidationError> {
    FIELDS.reject_unknown_fields(&raw.document, "runner_manifest", MANIFEST_FIELDS)?;
    let input_definitions = validate_inputs(
        FIELDS
            .optional_object(raw.document.get("input_definitions"), "input_definitions")?
            .unwrap_or_default(),
        "input_definitions",
    )?;
    let skill = FIELDS.required_string(raw.document.get("skill"), "skill")?;
    let runners_record = FIELDS.required_object(raw.document.get("runners"), "runners")?;
    let mut runners = BTreeMap::new();
    for (name, value) in runners_record {
        let JsonValue::Object(runner) = value else {
            return Err(FIELDS.validation_error(format!("runners.{name} must be an object.")));
        };
        let runner = expand_input_definitions(name, runner, &input_definitions)?;
        runners.insert(
            name.clone(),
            validate_runner_definition(&skill, name, runner)?,
        );
    }

    let connectors = validate_connector_requirements(raw.document.get("connectors"))?;
    bind_operation_steps(&mut runners, &connectors)?;
    let tools = validate_limit_refs(raw.document.get("tools"), "tools", |tool| {
        let admission = runx_core::policy::admit_agent_tool_ref(tool);
        (!admission.allowed).then(|| admission.reason.to_owned())
    })?;
    let skills = validate_limit_refs(raw.document.get("skills"), "skills", |_| None)?;
    let asks = FIELDS
        .optional_bool(raw.document.get("asks"), "asks")?
        .unwrap_or(false);
    bind_turn_allowance(
        &mut runners,
        &turn_allowance(&connectors, &tools, &skills, asks),
        &connectors,
    );
    let marketplace = validate_marketplace(raw.document.get("marketplace"), &runners)?;

    let harness = validate_harness_manifest(
        FIELDS.optional_object(raw.document.get("harness"), "harness")?,
        "harness",
    )?;
    validate_harness_runners(&harness, &runners)?;

    let catalog = validate_catalog_metadata(
        FIELDS.optional_object(raw.document.get("catalog"), "catalog")?,
        "catalog",
    )?;
    Ok(SkillRunnerManifest {
        skill,
        version: FIELDS.optional_string(raw.document.get("version"), "version")?,
        catalog,
        connectors,
        tools,
        skills,
        asks,
        input_definitions,
        marketplace,
        runners,
        harness,
        raw,
    })
}

/// A list of distinct, non-empty refs; `refuse` names why a ref is not one.
fn validate_limit_refs(
    value: Option<&JsonValue>,
    field: &str,
    refuse: impl Fn(&str) -> Option<String>,
) -> Result<Vec<String>, ValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let refs = FIELDS.required_plain_array(Some(value), field)?;
    let mut seen = std::collections::BTreeSet::new();
    refs.iter()
        .enumerate()
        .map(|(index, value)| {
            let entry = format!("{field}[{index}]");
            let reference = FIELDS.required_string(Some(value), &entry)?;
            if let Some(reason) = refuse(&reference) {
                return Err(FIELDS.validation_error(format!("{entry} {reference:?}: {reason}.")));
            }
            if !seen.insert(reference.clone()) {
                return Err(FIELDS.validation_error(format!("{entry} repeats {reference:?}.")));
            }
            Ok(reference)
        })
        .collect()
}

/// The skill's limits as one turn's allowance.
fn turn_allowance(
    connectors: &BTreeMap<String, ConnectorRequirement>,
    tools: &[String],
    skills: &[String],
    asks: bool,
) -> TurnAllowance {
    TurnAllowance {
        operations: connectors
            .iter()
            .flat_map(|(requirement, declared)| {
                declared
                    .operations
                    .iter()
                    .map(|operation| AllowedOperation {
                        requirement: requirement.as_str().into(),
                        operation: operation.as_str().into(),
                    })
            })
            .collect(),
        tools: tools.iter().map(|tool| tool.as_str().into()).collect(),
        skills: skills.iter().map(|skill| skill.as_str().into()).collect(),
        asks,
    }
}

/// Every agent step of every runner may call the whole of the skill's
/// limits, admitted under the skill's own declarations.
fn bind_turn_allowance(
    runners: &mut BTreeMap<String, SkillRunnerDefinition>,
    allowance: &TurnAllowance,
    connectors: &BTreeMap<String, ConnectorRequirement>,
) {
    for step in runners
        .values_mut()
        .flat_map(|runner| runner.graph.steps.iter_mut())
    {
        if let crate::StepKind::Agent(agent) = &mut step.kind {
            agent.allowance = allowance.clone();
            agent.connectors = connectors.clone();
        }
    }
}

fn validate_marketplace(
    value: Option<&JsonValue>,
    runners: &BTreeMap<String, SkillRunnerDefinition>,
) -> Result<Option<MarketplaceManifest>, ValidationError> {
    let Some(marketplace) = FIELDS.optional_object(value, "marketplace")? else {
        return Ok(None);
    };
    FIELDS.reject_unknown_fields(&marketplace, "marketplace", &["offers"])?;
    let raw_offers = FIELDS.required_object(marketplace.get("offers"), "marketplace.offers")?;
    if raw_offers.is_empty() {
        return Err(FIELDS.validation_error("marketplace.offers must not be empty."));
    }

    let mut offers = BTreeMap::new();
    for (runner, value) in raw_offers {
        if !runners.contains_key(runner) {
            return Err(FIELDS.validation_error(format!(
                "marketplace.offers.{runner} references an undeclared runner."
            )));
        }
        let serialized = serde_json::to_value(value).map_err(|error| {
            FIELDS.validation_error(format!(
                "marketplace.offers.{runner} could not be materialized: {error}"
            ))
        })?;
        let terms = if serialized.get("amount_minor").is_some() {
            serde_json::from_value::<PaidSkillFixedOfferTerms>(serialized)
                .map(PaidSkillOfferTerms::Fixed)
        } else {
            serde_json::from_value::<PaidSkillPreparedOfferTerms>(serialized)
                .map(PaidSkillOfferTerms::Prepared)
        }
        .map_err(|error| {
            FIELDS.validation_error(format!("marketplace.offers.{runner} is invalid: {error}"))
        })?;
        if !terms.mediation_and_executor_are_consistent() {
            return Err(FIELDS.validation_error(format!(
                "marketplace.offers.{runner} must declare executor and mediation together."
            )));
        }
        offers.insert(runner.clone(), terms);
    }
    Ok(Some(MarketplaceManifest { offers }))
}

fn expand_input_definitions(
    runner_name: &str,
    runner: &JsonObject,
    definitions: &BTreeMap<String, crate::skill::SkillInput>,
) -> Result<JsonObject, ValidationError> {
    let Some(inputs) = runner.get("inputs") else {
        return Ok(runner.clone());
    };
    let inputs = FIELDS.required_object(Some(inputs), &format!("runners.{runner_name}.inputs"))?;
    let mut expanded = JsonObject::new();
    for (input_name, declaration) in inputs {
        let Some(reference) = declaration
            .as_object()
            .filter(|declaration| declaration.len() == 1)
            .and_then(|declaration| declaration.get("definition"))
            .and_then(JsonValue::as_str)
        else {
            expanded.insert(input_name.clone(), declaration.clone());
            continue;
        };
        let definition = definitions.get(reference).ok_or_else(|| {
            FIELDS.validation_error(format!(
                "runners.{runner_name}.inputs.{input_name}.definition references unknown input_definitions.{reference}"
            ))
        })?;
        let value = serde_json::to_value(definition)
            .and_then(serde_json::from_value)
            .map_err(|error| {
                FIELDS.validation_error(format!(
                    "input_definitions.{reference} could not be materialized: {error}"
                ))
            })?;
        expanded.insert(input_name.clone(), value);
    }
    let mut resolved = runner.clone();
    resolved.insert("inputs".to_owned(), JsonValue::Object(expanded));
    Ok(resolved)
}

fn validate_harness_runners(
    harness: &Option<RunnerHarnessManifest>,
    runners: &BTreeMap<String, SkillRunnerDefinition>,
) -> Result<(), ValidationError> {
    for entry in harness.iter().flat_map(|harness| harness.cases.iter()) {
        if let Some(runner) = &entry.runner
            && !runners.contains_key(runner)
        {
            return Err(FIELDS.validation_error(format!(
                "harness.cases runner {runner} is not declared in runners."
            )));
        }
    }
    Ok(())
}
