use std::collections::BTreeMap;

use runx_contracts::{
    AllowedOperation, CatalogRule, ExecutionRequirements, JsonObject, JsonValue, PaidSkillActsOn,
    PaidSkillOfferTerms, PaidSkillPrice, SkillAllowance, TurnAllowance,
    canonical_json::canonical_stable_json,
};
use serde::{Deserialize, Serialize};

use crate::skill::{
    CatalogMetadata, ConnectorRequirement, RunnerHarnessManifest, SkillInput,
    SkillRunnerDefinition, bind_operation_steps, validate_catalog_metadata,
    validate_connector_requirements, validate_harness_manifest, validate_inputs,
    validate_runner_definition,
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
    /// The child skills an agent turn may run, and the catalog rules it may
    /// load packages under.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillAllowance>,
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
    let limits = TurnLimits::of(&raw.document, "")?;
    let runners_record = FIELDS.required_object(raw.document.get("runners"), "runners")?;
    let mut runners = BTreeMap::new();
    let mut runner_limits = BTreeMap::new();
    for (name, value) in runners_record {
        let JsonValue::Object(runner) = value else {
            return Err(FIELDS.validation_error(format!("runners.{name} must be an object.")));
        };
        runner_limits.insert(name.clone(), limits.narrowed(runner, name)?);
        let runner = expand_input_definitions(name, runner, &input_definitions)?;
        runners.insert(
            name.clone(),
            validate_runner_definition(&skill, name, runner)?,
        );
    }

    for (name, runner) in &runners {
        let mut seen = std::collections::BTreeSet::new();
        for continuation in &runner.continuations {
            if continuation == name
                || !runners.contains_key(continuation)
                || !seen.insert(continuation)
            {
                return Err(FIELDS.validation_error(format!(
                    "runners.{name}.continuations must name distinct other runners in this package; invalid {continuation:?}."
                )));
            }
        }
    }
    let connectors = validate_connector_requirements(raw.document.get("connectors"))?;
    bind_operation_steps(&mut runners, &connectors)?;
    bind_turn_allowance(&mut runners, &runner_limits, &connectors);
    let TurnLimits {
        tools,
        skills,
        asks,
    } = limits;
    let marketplace = validate_marketplace(raw.document.get("marketplace"), &runners, &connectors)?;

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

/// What an agent turn may call beyond the skill's connector operations: the
/// skill's limits, or a runner's narrowing of them.
struct TurnLimits {
    tools: Vec<String>,
    skills: Vec<SkillAllowance>,
    asks: bool,
}

impl TurnLimits {
    /// The limits `document` declares, its fields named under `prefix`.
    fn of(document: &JsonObject, prefix: &str) -> Result<Self, ValidationError> {
        let field = |name: &str| format!("{prefix}{name}");
        let tools = distinct_entries(document.get("tools"), &field("tools"), |entry, value| {
            let tool = FIELDS.required_string(Some(value), entry)?;
            let admission = runx_core::policy::admit_agent_tool(&tool);
            if !admission.allowed {
                return Err(
                    FIELDS.validation_error(format!("{entry} {tool:?}: {}.", admission.reason))
                );
            }
            Ok(tool)
        })?;
        let skills = distinct_entries(document.get("skills"), &field("skills"), skill_allowance)?;
        let asks = FIELDS
            .optional_bool(document.get("asks"), &field("asks"))?
            .unwrap_or(false);
        Ok(Self {
            tools,
            skills,
            asks,
        })
    }

    /// The limits runner `name` declares, each a part of the skill's; a
    /// runner that declares none of a kind keeps the skill's.
    fn narrowed(&self, runner: &JsonObject, name: &str) -> Result<Self, ValidationError> {
        let prefix = format!("runners.{name}.");
        let declared = Self::of(runner, &prefix)?;
        let beyond = |kind: &str, entry: String| {
            FIELDS.validation_error(format!(
                "{prefix}{kind} names {entry}, which the skill's {kind} do not."
            ))
        };
        if let Some(tool) = declared
            .tools
            .iter()
            .find(|tool| !self.tools.contains(tool))
        {
            return Err(beyond("tools", format!("{tool:?}")));
        }
        if let Some(skill) = declared
            .skills
            .iter()
            .find(|skill| !self.skills.contains(skill))
        {
            return Err(beyond(
                "skills",
                serde_json::to_string(skill).unwrap_or_default(),
            ));
        }
        if declared.asks && !self.asks {
            return Err(FIELDS.validation_error(format!(
                "{prefix}asks is true, and the skill's asks is not."
            )));
        }
        Ok(Self {
            tools: if runner.contains_key("tools") {
                declared.tools
            } else {
                self.tools.clone()
            },
            skills: if runner.contains_key("skills") {
                declared.skills
            } else {
                self.skills.clone()
            },
            asks: if runner.contains_key("asks") {
                declared.asks
            } else {
                self.asks
            },
        })
    }

    /// The limits as one turn's allowance, beside the skill's operations.
    fn allowance(&self, connectors: &BTreeMap<String, ConnectorRequirement>) -> TurnAllowance {
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
            tools: self.tools.iter().map(|tool| tool.as_str().into()).collect(),
            skills: self.skills.clone(),
            asks: self.asks,
        }
    }
}

/// A list of distinct entries, each read by `entry`.
fn distinct_entries<T: PartialEq>(
    value: Option<&JsonValue>,
    field: &str,
    entry: impl Fn(&str, &JsonValue) -> Result<T, ValidationError>,
) -> Result<Vec<T>, ValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for (index, value) in FIELDS
        .required_plain_array(Some(value), field)?
        .iter()
        .enumerate()
    {
        let name = format!("{field}[{index}]");
        let read = entry(&name, value)?;
        if entries.contains(&read) {
            return Err(FIELDS.validation_error(format!(
                "{name} repeats {}.",
                canonical_stable_json(value).unwrap_or_default()
            )));
        }
        entries.push(read);
    }
    Ok(entries)
}

/// A `skills` entry: a child skill's ref, or a catalog rule.
fn skill_allowance(entry: &str, value: &JsonValue) -> Result<SkillAllowance, ValidationError> {
    if let JsonValue::Object(_) = value {
        let rule = value
            .clone()
            .deserialize_into::<CatalogRule>()
            .map_err(|error| {
                FIELDS.validation_error(format!("{entry} is not a catalog rule: {error}."))
            })?;
        rule.check()
            .map_err(|reason| FIELDS.validation_error(format!("{entry}: {reason}.")))?;
        return Ok(SkillAllowance::Rule(rule));
    }
    let reference = FIELDS.required_string(Some(value), entry)?;
    Ok(SkillAllowance::Skill(reference.as_str().into()))
}

/// Every agent step of a runner may call the whole of the runner's limits,
/// admitted under the skill's own declarations.
fn bind_turn_allowance(
    runners: &mut BTreeMap<String, SkillRunnerDefinition>,
    limits: &BTreeMap<String, TurnLimits>,
    connectors: &BTreeMap<String, ConnectorRequirement>,
) {
    for (name, runner) in runners.iter_mut() {
        let Some(limits) = limits.get(name) else {
            continue;
        };
        let allowance = limits.allowance(connectors);
        for step in &mut runner.graph.steps {
            if let crate::StepKind::Agent(agent) = &mut step.kind {
                agent.allowance = allowance.clone();
                agent.connectors = connectors.clone();
            }
        }
    }
}

fn validate_marketplace(
    value: Option<&JsonValue>,
    runners: &BTreeMap<String, SkillRunnerDefinition>,
    connectors: &BTreeMap<String, ConnectorRequirement>,
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
        let Some(definition) = runners.get(runner) else {
            return Err(FIELDS.validation_error(format!(
                "marketplace.offers.{runner} references an undeclared runner."
            )));
        };
        let serialized = serde_json::to_value(value).map_err(|error| {
            FIELDS.validation_error(format!(
                "marketplace.offers.{runner} could not be materialized: {error}"
            ))
        })?;
        let terms = serde_json::from_value::<PaidSkillOfferTerms>(serialized).map_err(|error| {
            FIELDS.validation_error(format!("marketplace.offers.{runner} is invalid: {error}"))
        })?;
        validate_priced_inputs(runner, &terms.price, &definition.inputs)?;
        if let Some(acts_on) = &terms.acts_on {
            validate_acts_on(runner, acts_on, &definition.inputs, connectors)?;
        }
        offers.insert(runner.clone(), terms);
    }
    Ok(Some(MarketplaceManifest { offers }))
}

/// A price reads only inputs its runner requires: each `input_choice`
/// selector an `enum` input, with a case for every combination of their
/// values, and the `artifact_bytes` input a JSON value holding the stored
/// artifact's reference. A packet envelope (`artifact: true`) is not a stored
/// artifact and has no committed size.
/// What a paid runner acts on is held in one of the skill's own connectors,
/// and named by inputs every purchase of the runner carries.
fn validate_acts_on(
    runner: &str,
    acts_on: &PaidSkillActsOn,
    inputs: &BTreeMap<String, SkillInput>,
    connectors: &BTreeMap<String, ConnectorRequirement>,
) -> Result<(), ValidationError> {
    let field = format!("marketplace.offers.{runner}.acts_on");
    if !connectors.contains_key(acts_on.connector.as_str()) {
        return Err(FIELDS.validation_error(format!(
            "{field} names connector {}, which the skill does not declare.",
            acts_on.connector
        )));
    }
    for pointer in acts_on.target.argument_pointers() {
        let input = pointer
            .strip_prefix('/')
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_default();
        if !inputs.get(input).is_some_and(|input| input.required) {
            return Err(FIELDS.validation_error(format!(
                "{field} reads input {input}, which runner {runner} does not require."
            )));
        }
    }
    Ok(())
}

fn validate_priced_inputs(
    runner: &str,
    price: &PaidSkillPrice,
    inputs: &BTreeMap<String, SkillInput>,
) -> Result<(), ValidationError> {
    let field = format!("marketplace.offers.{runner}.price");
    for name in price.priced_inputs() {
        if !inputs.get(name).is_some_and(|input| input.required) {
            return Err(FIELDS.validation_error(format!(
                "{field} reads input {name}, which runner {runner} does not require."
            )));
        }
    }
    match price {
        PaidSkillPrice::Fixed { .. } => Ok(()),
        PaidSkillPrice::ArtifactBytes { artifact_input, .. } => {
            let input = &inputs[artifact_input.as_str()];
            if matches!(input.input_type.as_str(), "json" | "object")
                && input.artifact != Some(true)
            {
                Ok(())
            } else {
                Err(FIELDS.validation_error(format!(
                    "{field} prices input {artifact_input} by the bytes of the stored artifact it references, so runner {runner} declares it a json or object input."
                )))
            }
        }
        PaidSkillPrice::InputChoice { selectors, cases } => {
            let mut combinations = 1_usize;
            for (index, selector) in selectors.iter().enumerate() {
                let choices = enum_choices(&inputs[selector.as_str()]).ok_or_else(|| {
                    FIELDS.validation_error(format!(
                        "{field} selects on input {selector}, so runner {runner} declares its enum."
                    ))
                })?;
                combinations = combinations.saturating_mul(choices.len());
                if let Some(case) = cases
                    .iter()
                    .find(|case| !choices.contains(&case.when[index].as_value()))
                {
                    return Err(FIELDS.validation_error(format!(
                        "{field} has a case for {selector} = {}, which is not one of its enum values.",
                        canonical_stable_json(case.when[index].as_value()).unwrap_or_default()
                    )));
                }
            }
            // Cases are unique and each value is a declared one, so a case per
            // combination is an exact cover.
            if cases.len() == combinations {
                Ok(())
            } else {
                Err(FIELDS.validation_error(format!(
                    "{field} prices {} of the {combinations} combinations of its selector values; it prices each once.",
                    cases.len()
                )))
            }
        }
    }
}

/// The distinct values an input's schema enumerates, if it enumerates any.
fn enum_choices(input: &SkillInput) -> Option<Vec<&JsonValue>> {
    let JsonValue::Array(values) = input.schema.as_ref()?.get("enum")? else {
        return None;
    };
    let mut choices = Vec::<&JsonValue>::new();
    for value in values {
        if !choices.contains(&value) {
            choices.push(value);
        }
    }
    (!choices.is_empty()).then_some(choices)
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
