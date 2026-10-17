// Module rationale: catalog enums, parsing, and cross-field capability validation form one public metadata contract.
use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::JsonObject;
use serde::{Deserialize, Serialize};

use crate::ValidationError;

use super::FIELDS;

const CATALOG_FIELDS: &[&str] = &[
    "approval",
    "audience",
    "canonical_skill",
    "category",
    "completion",
    "execution",
    "kind",
    "links",
    "owner",
    "part_of",
    "provider",
    "role",
    "tags",
    "visibility",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    Skill,
    Graph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogAudience {
    Public,
    Builder,
    Operator,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogVisibility {
    Public,
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogRole {
    Canonical,
    Branded,
    Context,
    GraphStage,
    RuntimePath,
    HarnessFixture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogExecution {
    Plan,
    Read,
    Execute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogCompletion {
    Plan,
    RuntimeReceipt,
    ProviderReadback,
    /// The provider acknowledged the effect and offers no read of it, as a
    /// webhook does.
    ProviderAcknowledgement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogApproval {
    None,
    Conditional,
    Required,
}

impl CatalogKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogKind::Skill => "skill",
            CatalogKind::Graph => "graph",
        }
    }
}

impl CatalogAudience {
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogAudience::Public => "public",
            CatalogAudience::Builder => "builder",
            CatalogAudience::Operator => "operator",
            CatalogAudience::System => "system",
        }
    }
}

impl CatalogVisibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogVisibility::Public => "public",
            CatalogVisibility::Internal => "internal",
        }
    }
}

impl CatalogRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogRole::Canonical => "canonical",
            CatalogRole::Branded => "branded",
            CatalogRole::Context => "context",
            CatalogRole::GraphStage => "graph-stage",
            CatalogRole::RuntimePath => "runtime-path",
            CatalogRole::HarnessFixture => "harness-fixture",
        }
    }
}

impl CatalogExecution {
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogExecution::Plan => "plan",
            CatalogExecution::Read => "read",
            CatalogExecution::Execute => "execute",
        }
    }
}

impl CatalogCompletion {
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogCompletion::Plan => "plan",
            CatalogCompletion::RuntimeReceipt => "runtime_receipt",
            CatalogCompletion::ProviderReadback => "provider_readback",
            CatalogCompletion::ProviderAcknowledgement => "provider_acknowledgement",
        }
    }
}

impl CatalogApproval {
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogApproval::None => "none",
            CatalogApproval::Conditional => "conditional",
            CatalogApproval::Required => "required",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogMetadata {
    pub kind: CatalogKind,
    pub audience: CatalogAudience,
    pub visibility: CatalogVisibility,
    pub role: CatalogRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_skill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_of: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<CatalogExecution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<CatalogCompletion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<CatalogApproval>,
    /// The registry category the skill declares: a skill category of the
    /// registry taxonomy (`registry/taxonomy.json`), checked here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The registry namespace a contributed package publishes under; absent,
    /// the package belongs to the publishing owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Named reference URLs, such as the package's source repository.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSemanticCode {
    MissingColdSelectionProof,
    MissingStandaloneDefaultProof,
    MissingComposedReuseProof,
}

impl CatalogSemanticCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingColdSelectionProof => "missing_cold_selection_proof",
            Self::MissingStandaloneDefaultProof => "missing_standalone_default_proof",
            Self::MissingComposedReuseProof => "missing_composed_reuse_proof",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogOperatorReadiness {
    pub evaluated: bool,
    pub cold_selection: bool,
    pub standalone_default: bool,
    pub composed_reuse: bool,
    pub supplied_agent_answers: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cold_selection_confusors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standalone_case: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composed_case: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSemanticDiagnostic {
    pub code: CatalogSemanticCode,
    pub skill: String,
    pub runner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_execution: Option<CatalogExecution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_completion: Option<CatalogCompletion>,
    pub observed: Vec<String>,
    pub required_correction: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSemanticReport {
    pub mode: String,
    pub skill: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_runner: Option<String>,
    pub diagnostics: Vec<CatalogSemanticDiagnostic>,
    pub readiness: CatalogOperatorReadiness,
}

/// Project catalog metadata into a stable report. Runtime execution facts are
/// deliberately absent: only the resolved execution closure can establish
/// which native capabilities a composed runner reaches.
#[must_use]
pub fn analyze_catalog_semantics(
    skill: &str,
    manifest: &crate::SkillRunnerManifest,
) -> CatalogSemanticReport {
    let default = manifest
        .runners
        .values()
        .find(|runner| runner.default)
        .or_else(|| {
            (manifest.runners.len() == 1)
                .then(|| manifest.runners.values().next())
                .flatten()
        });
    let Some(runner) = default else {
        return CatalogSemanticReport {
            mode: "operator_readiness".to_owned(),
            skill: skill.to_owned(),
            default_runner: None,
            diagnostics: Vec::new(),
            readiness: CatalogOperatorReadiness::default(),
        };
    };
    let Some(_catalog) = manifest.catalog.as_ref() else {
        return CatalogSemanticReport {
            mode: "operator_readiness".to_owned(),
            skill: skill.to_owned(),
            default_runner: Some(runner.name.clone()),
            diagnostics: Vec::new(),
            readiness: CatalogOperatorReadiness::default(),
        };
    };
    CatalogSemanticReport {
        mode: "operator_readiness".to_owned(),
        skill: skill.to_owned(),
        default_runner: Some(runner.name.clone()),
        diagnostics: Vec::new(),
        readiness: CatalogOperatorReadiness::default(),
    }
}

/// Add package-fixture evidence to the structural catalog report. This is the
/// only readiness owner used by complete package admission and inspection;
/// callers with a raw manifest receive structural facts with
/// `readiness.evaluated = false` rather than invented journey proof.
#[must_use]
pub fn analyze_package_catalog_semantics(
    skill: &str,
    manifest: &crate::SkillRunnerManifest,
    fixtures: &BTreeMap<String, crate::harness_fixture::HarnessFixture>,
) -> CatalogSemanticReport {
    let mut report = analyze_catalog_semantics(skill, manifest);
    let Some(catalog) = manifest.catalog.as_ref() else {
        return report;
    };
    if catalog.visibility != CatalogVisibility::Public {
        return report;
    }
    let Some(default_runner) = report.default_runner.as_deref() else {
        return report;
    };
    let Some(runner) = manifest.runners.get(default_runner) else {
        return report;
    };
    let readiness = package_operator_readiness(skill, manifest, fixtures, default_runner);
    let observed = readiness_observations(&readiness);
    if !readiness.cold_selection {
        report.diagnostics.push(semantic_diagnostic(
            CatalogSemanticCode::MissingColdSelectionProof,
            skill,
            runner,
            catalog,
            &observed,
            "add a standalone journey for the actual default runner with at least two nearby public skill confusors",
        ));
    }
    if !readiness.standalone_default {
        report.diagnostics.push(semantic_diagnostic(
            CatalogSemanticCode::MissingStandaloneDefaultProof,
            skill,
            runner,
            catalog,
            &observed,
            "run a standalone operator journey against the actual default runner rather than a convenient phase runner",
        ));
    }
    if !readiness.composed_reuse {
        report.diagnostics.push(semantic_diagnostic(
            CatalogSemanticCode::MissingComposedReuseProof,
            skill,
            runner,
            catalog,
            &observed,
            "add a composed journey that names reused prior evidence and work that must not be repeated",
        ));
    }
    report.diagnostics.sort_by_key(|diagnostic| diagnostic.code);
    report.readiness = readiness;
    report
}

fn package_operator_readiness(
    skill: &str,
    manifest: &crate::SkillRunnerManifest,
    fixtures: &BTreeMap<String, crate::harness_fixture::HarnessFixture>,
    default_runner: &str,
) -> CatalogOperatorReadiness {
    let mut readiness = CatalogOperatorReadiness {
        evaluated: true,
        ..CatalogOperatorReadiness::default()
    };
    if let Some(harness) = manifest.harness.as_ref() {
        for case in &harness.cases {
            update_operator_readiness(
                skill,
                default_runner,
                &case.name,
                true,
                case.runner.as_deref(),
                &case.operator_journeys,
                !case.caller.answers.is_empty(),
                case.expect.status.as_ref(),
                &mut readiness,
            );
        }
    }
    for fixture in fixtures.values() {
        update_operator_readiness(
            skill,
            default_runner,
            &fixture.name,
            matches!(
                fixture.kind,
                crate::harness_fixture::HarnessFixtureKind::Skill
            ) && fixture.target == "..",
            fixture.runner.as_deref(),
            &fixture.operator_journeys,
            !fixture.caller.answers.is_empty(),
            fixture.expect.status.as_ref(),
            &mut readiness,
        );
    }
    readiness
}

#[allow(clippy::too_many_arguments)]
fn update_operator_readiness(
    skill: &str,
    default_runner: &str,
    case_name: &str,
    package_runner_case: bool,
    selected_runner: Option<&str>,
    journeys: &[crate::OperatorJourneyClaim],
    supplied_agent_answers: bool,
    expected_status: Option<&crate::harness_fixture::HarnessExpectedStatus>,
    readiness: &mut CatalogOperatorReadiness,
) {
    for journey in journeys {
        let completed = matches!(
            expected_status,
            Some(crate::harness_fixture::HarnessExpectedStatus::Sealed)
        );
        let exercises_default = if package_runner_case {
            selected_runner.is_none_or(|runner| runner == default_runner)
        } else {
            journey.exercises_runner.as_deref() == Some(default_runner)
        };
        match journey.mode {
            crate::OperatorJourneyMode::Standalone if exercises_default && completed => {
                readiness.standalone_default = true;
                readiness
                    .standalone_case
                    .get_or_insert_with(|| case_name.to_owned());
                readiness.supplied_agent_answers |= supplied_agent_answers;
                let confusors = journey
                    .confusors
                    .iter()
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty() && *value != skill)
                    .collect::<BTreeSet<_>>();
                if confusors.len() >= 2 {
                    readiness.cold_selection = true;
                    readiness.cold_selection_confusors =
                        confusors.into_iter().map(str::to_owned).collect();
                }
            }
            crate::OperatorJourneyMode::Composed if completed => {
                readiness.composed_reuse = true;
                readiness
                    .composed_case
                    .get_or_insert_with(|| case_name.to_owned());
            }
            crate::OperatorJourneyMode::Standalone
            | crate::OperatorJourneyMode::Composed
            | crate::OperatorJourneyMode::Refusal => {}
        }
    }
}

fn readiness_observations(readiness: &CatalogOperatorReadiness) -> Vec<String> {
    let mut observed = Vec::new();
    if readiness.cold_selection {
        observed.push("cold_selection_harness".to_owned());
    }
    if readiness.standalone_default {
        observed.push("standalone_default_harness".to_owned());
    }
    if readiness.composed_reuse {
        observed.push("composed_reuse_harness".to_owned());
    }
    if readiness.supplied_agent_answers {
        observed.push("supplied_agent_answers".to_owned());
    }
    observed
}

fn semantic_diagnostic(
    code: CatalogSemanticCode,
    skill: &str,
    runner: &super::SkillRunnerDefinition,
    catalog: &CatalogMetadata,
    observed: &[String],
    required_correction: &str,
) -> CatalogSemanticDiagnostic {
    CatalogSemanticDiagnostic {
        code,
        skill: skill.to_owned(),
        runner: runner.name.clone(),
        claimed_execution: catalog.execution,
        claimed_completion: catalog.completion,
        observed: observed.to_vec(),
        required_correction: required_correction.to_owned(),
    }
}

pub(crate) fn validate_catalog_metadata(
    value: Option<JsonObject>,
    label: &str,
) -> Result<Option<CatalogMetadata>, ValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    FIELDS.reject_unknown_fields(&value, label, CATALOG_FIELDS)?;
    let kind = parse_catalog_kind(&value, label)?;
    let audience = parse_catalog_audience(&value, label)?;
    let visibility = parse_catalog_visibility(&value, label)?;
    let role = parse_catalog_role(&value, label)?;
    validate_catalog_role(visibility, role, label)?;
    let canonical_skill = FIELDS.optional_string(
        value.get("canonical_skill"),
        &format!("{label}.canonical_skill"),
    )?;
    let provider = FIELDS.optional_string(value.get("provider"), &format!("{label}.provider"))?;
    let part_of = FIELDS
        .optional_string_array(value.get("part_of"), &format!("{label}.part_of"))?
        .unwrap_or_default();
    let execution = parse_catalog_execution(&value, label)?;
    let completion = parse_catalog_completion(&value, label)?;
    let approval = parse_catalog_approval(&value, label)?;
    let capability_fields = [
        execution.is_some(),
        completion.is_some(),
        approval.is_some(),
    ];
    if capability_fields.iter().any(|present| *present)
        && capability_fields.iter().any(|present| !*present)
    {
        return Err(FIELDS.validation_error(format!(
            "{label} capability metadata must declare execution, completion and approval together."
        )));
    }
    validate_catalog_bindings(role, &canonical_skill, &provider, &part_of, label)?;
    let category =
        FIELDS.optional_non_empty_string(value.get("category"), &format!("{label}.category"))?;
    if let Some(category) = &category {
        runx_contracts::validate_registry_category(
            runx_contracts::TaxonomySubject::Skill,
            category,
        )
        .map_err(|reason| FIELDS.validation_error(format!("{label}.category: {reason}")))?;
    }
    let tags = parse_catalog_tags(&value, label)?;
    let owner = parse_catalog_owner(&value, label)?;
    let links = parse_catalog_links(&value, label)?;
    Ok(Some(CatalogMetadata {
        kind,
        audience,
        visibility,
        role,
        canonical_skill,
        provider,
        part_of,
        execution,
        completion,
        approval,
        category,
        tags,
        owner,
        links,
    }))
}

fn parse_catalog_tags(value: &JsonObject, label: &str) -> Result<Vec<String>, ValidationError> {
    let field = format!("{label}.tags");
    let tags = FIELDS
        .optional_string_array(value.get("tags"), &field)?
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    for tag in &tags {
        if tag.trim().is_empty() || tag.trim() != tag {
            return Err(FIELDS.validation_error(format!(
                "{field} entries must be non-empty and carry no surrounding whitespace."
            )));
        }
        if !seen.insert(tag.as_str()) {
            return Err(FIELDS.validation_error(format!("{field} repeats '{tag}'.")));
        }
    }
    Ok(tags)
}

fn parse_catalog_owner(value: &JsonObject, label: &str) -> Result<Option<String>, ValidationError> {
    let field = format!("{label}.owner");
    let Some(owner) = FIELDS.optional_string(value.get("owner"), &field)? else {
        return Ok(None);
    };
    // `^[a-z0-9](?:[a-z0-9._-]*[a-z0-9])?$`
    let alphanumeric = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let canonical = owner.as_bytes().first().is_some_and(alphanumeric)
        && owner.as_bytes().last().is_some_and(alphanumeric)
        && owner
            .bytes()
            .all(|byte| alphanumeric(&byte) || matches!(byte, b'.' | b'_' | b'-'));
    if !canonical {
        return Err(FIELDS.validation_error(format!(
            "{field} must be a canonical lowercase registry namespace."
        )));
    }
    Ok(Some(owner))
}

fn parse_catalog_links(
    value: &JsonObject,
    label: &str,
) -> Result<BTreeMap<String, String>, ValidationError> {
    let field = format!("{label}.links");
    let Some(links) = FIELDS.optional_object(value.get("links"), &field)? else {
        return Ok(BTreeMap::new());
    };
    links
        .into_iter()
        .map(|(name, url)| {
            let url = FIELDS
                .optional_string(Some(&url), &format!("{field}.{name}"))?
                .filter(|url| url.starts_with("https://") && url.len() > "https://".len())
                .ok_or_else(|| {
                    FIELDS.validation_error(format!("{field}.{name} must be an https URL."))
                })?;
            Ok((name, url))
        })
        .collect()
}

fn parse_catalog_execution(
    value: &JsonObject,
    label: &str,
) -> Result<Option<CatalogExecution>, ValidationError> {
    match FIELDS
        .optional_string(value.get("execution"), &format!("{label}.execution"))?
        .as_deref()
    {
        Some("plan") => Ok(Some(CatalogExecution::Plan)),
        Some("read") => Ok(Some(CatalogExecution::Read)),
        Some("execute") => Ok(Some(CatalogExecution::Execute)),
        None => Ok(None),
        Some(_) => {
            Err(FIELDS
                .validation_error(format!("{label}.execution must be plan, read, or execute.")))
        }
    }
}

fn parse_catalog_completion(
    value: &JsonObject,
    label: &str,
) -> Result<Option<CatalogCompletion>, ValidationError> {
    match FIELDS
        .optional_string(value.get("completion"), &format!("{label}.completion"))?
        .as_deref()
    {
        Some("plan") => Ok(Some(CatalogCompletion::Plan)),
        Some("runtime_receipt") => Ok(Some(CatalogCompletion::RuntimeReceipt)),
        Some("provider_readback") => Ok(Some(CatalogCompletion::ProviderReadback)),
        Some("provider_acknowledgement") => Ok(Some(CatalogCompletion::ProviderAcknowledgement)),
        None => Ok(None),
        Some(_) => Err(FIELDS.validation_error(format!(
            "{label}.completion must be plan, runtime_receipt, provider_readback, or provider_acknowledgement."
        ))),
    }
}

fn parse_catalog_approval(
    value: &JsonObject,
    label: &str,
) -> Result<Option<CatalogApproval>, ValidationError> {
    match FIELDS
        .optional_string(value.get("approval"), &format!("{label}.approval"))?
        .as_deref()
    {
        Some("none") => Ok(Some(CatalogApproval::None)),
        Some("conditional") => Ok(Some(CatalogApproval::Conditional)),
        Some("required") => Ok(Some(CatalogApproval::Required)),
        None => Ok(None),
        Some(_) => Err(FIELDS.validation_error(format!(
            "{label}.approval must be none, conditional, or required."
        ))),
    }
}

fn parse_catalog_kind(value: &JsonObject, label: &str) -> Result<CatalogKind, ValidationError> {
    match FIELDS
        .required_string(value.get("kind"), &format!("{label}.kind"))?
        .as_str()
    {
        "skill" => Ok(CatalogKind::Skill),
        "graph" => Ok(CatalogKind::Graph),
        _ => Err(FIELDS.validation_error(format!("{label}.kind must be skill or graph."))),
    }
}

fn parse_catalog_audience(
    value: &JsonObject,
    label: &str,
) -> Result<CatalogAudience, ValidationError> {
    match FIELDS
        .required_string(value.get("audience"), &format!("{label}.audience"))?
        .as_str()
    {
        "public" => Ok(CatalogAudience::Public),
        "builder" => Ok(CatalogAudience::Builder),
        "operator" => Ok(CatalogAudience::Operator),
        "system" => Ok(CatalogAudience::System),
        _ => Err(FIELDS.validation_error(format!(
            "{label}.audience must be public, builder, operator, or system."
        ))),
    }
}

fn parse_catalog_visibility(
    value: &JsonObject,
    label: &str,
) -> Result<CatalogVisibility, ValidationError> {
    match FIELDS
        .optional_string(value.get("visibility"), &format!("{label}.visibility"))?
        .as_deref()
    {
        Some("public") | None => Ok(CatalogVisibility::Public),
        Some("internal") => Ok(CatalogVisibility::Internal),
        Some(_) => {
            Err(FIELDS.validation_error(format!("{label}.visibility must be public or internal.")))
        }
    }
}

fn parse_catalog_role(value: &JsonObject, label: &str) -> Result<CatalogRole, ValidationError> {
    match FIELDS
        .required_string(value.get("role"), &format!("{label}.role"))?
        .as_str()
    {
        "canonical" => Ok(CatalogRole::Canonical),
        "branded" => Ok(CatalogRole::Branded),
        "context" => Ok(CatalogRole::Context),
        "graph-stage" => Ok(CatalogRole::GraphStage),
        "runtime-path" => Ok(CatalogRole::RuntimePath),
        "harness-fixture" => Ok(CatalogRole::HarnessFixture),
        _ => Err(FIELDS.validation_error(format!(
            "{label}.role must be canonical, branded, context, graph-stage, runtime-path, or harness-fixture."
        ))),
    }
}

fn validate_catalog_role(
    visibility: CatalogVisibility,
    role: CatalogRole,
    label: &str,
) -> Result<(), ValidationError> {
    if visibility == CatalogVisibility::Public
        && matches!(
            role,
            CatalogRole::GraphStage | CatalogRole::RuntimePath | CatalogRole::HarnessFixture
        )
    {
        return Err(FIELDS.validation_error(format!(
            "{label}.role cannot be {} when visibility is public.",
            role.as_str()
        )));
    }
    Ok(())
}

fn validate_catalog_bindings(
    role: CatalogRole,
    canonical_skill: &Option<String>,
    provider: &Option<String>,
    part_of: &[String],
    label: &str,
) -> Result<(), ValidationError> {
    if role == CatalogRole::Branded {
        if canonical_skill.is_none() {
            return Err(FIELDS.validation_error(format!(
                "{label}.canonical_skill is required when catalog.role is branded."
            )));
        }
        if provider.is_none() {
            return Err(FIELDS.validation_error(format!(
                "{label}.provider is required when catalog.role is branded."
            )));
        }
    }
    if matches!(
        role,
        CatalogRole::GraphStage | CatalogRole::RuntimePath | CatalogRole::HarnessFixture
    ) && part_of.is_empty()
    {
        return Err(FIELDS.validation_error(format!(
            "{label}.part_of is required when catalog.role is {}.",
            role.as_str()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(source: &str) -> Result<crate::SkillRunnerManifest, String> {
        let parsed =
            crate::parse_runner_manifest_yaml(source).map_err(|error| error.to_string())?;
        crate::validate_runner_manifest(parsed).map_err(|error| error.to_string())
    }

    #[test]
    fn catalog_report_does_not_guess_runtime_effects_from_names() -> Result<(), String> {
        let manifest = manifest(
            r#"skill: github
catalog:
  kind: skill
  audience: public
  visibility: public
  role: canonical
  execution: execute
  completion: provider_readback
  approval: required
runners:
  plan:
    default: true
    steps:
      - id: plan
        tool: { module: plan.mjs }
        outputs:
          plan: object
        artifacts:
          named_emits: { plan: plan }
          packets: { plan: runx.github_sync.plan.v1 }
"#,
        )?;
        let report = analyze_catalog_semantics("github", &manifest);
        assert_eq!(report.mode, "operator_readiness");
        assert_eq!(report.default_runner.as_deref(), Some("plan"));
        assert!(report.diagnostics.is_empty());
        Ok(())
    }

    #[test]
    fn package_semantic_report_requires_executable_cold_direct_and_composed_proof()
    -> Result<(), String> {
        let manifest = manifest(
            r#"skill: digest-note
catalog:
  kind: graph
  audience: public
  visibility: public
  role: context
  execution: read
  completion: runtime_receipt
  approval: none
harness:
  cases:
    - name: direct-only
      runner: digest
      operator_journeys:
        - mode: standalone
          confusors: [extract, sign-receipt]
          request: Digest this exact note into a reusable identity.
          expected_outcome: Return the canonical digest for the supplied bytes.
      inputs: { note: hello }
      expect: { status: sealed }
runners:
  digest:
    default: true
    inputs:
      note: { type: string, required: true, description: Exact note bytes. }
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs: { value: $input.note }
"#,
        )?;

        let report = analyze_package_catalog_semantics("digest-note", &manifest, &BTreeMap::new());
        assert!(report.readiness.evaluated);
        assert!(report.readiness.cold_selection);
        assert!(report.readiness.standalone_default);
        assert!(!report.readiness.composed_reuse);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == CatalogSemanticCode::MissingComposedReuseProof
        }));
        Ok(())
    }

    #[test]
    fn package_semantic_report_records_supplied_agent_answers() -> Result<(), String> {
        let manifest = manifest(
            r#"skill: provider-reader
catalog:
  kind: graph
  audience: public
  visibility: public
  role: canonical
  execution: read
  completion: provider_readback
  approval: none
harness:
  cases:
    - name: supplied-answer-provider-case
      runner: read
      operator_journeys:
        - mode: standalone
          confusors: [github, slack]
          request: Read this exact provider resource through its configured account.
          expected_outcome: Return bounded provider readback for the exact target.
        - mode: composed
          request: Continue from this already selected provider target.
          expected_outcome: Reuse the selected target without repeating discovery.
          prior_evidence: [The exact provider and target selected upstream.]
          must_not_repeat: [Do not rediscover or replace the selected target.]
      caller:
        answers:
          read: { provider_result: { id: resource-1 } }
      expect: { status: sealed }
connectors:
  example:
    operations: [resource.read]
runners:
  read:
    default: true
    inputs: {}
    result_from: [readback]
    steps:
      - id: readback
        operation: example/resource.read
        input: { id: resource-1 }
"#,
        )?;

        let report =
            analyze_package_catalog_semantics("provider-reader", &manifest, &BTreeMap::new());
        assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
        assert!(report.readiness.supplied_agent_answers);
        Ok(())
    }
}
