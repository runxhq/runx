use runx_contracts::{ExecutionSemantics, JsonObject, JsonValue};

use crate::{ValidationError, json_fields::JsonFieldReader};

mod catalog;
mod connectors;
mod environment;
mod execution_semantics;
mod fixtures;
mod governance;
mod markdown;
mod runner_definition;
mod source;
mod types;

pub use catalog::{
    CatalogApproval, CatalogAudience, CatalogCompletion, CatalogExecution, CatalogKind,
    CatalogMetadata, CatalogOperatorReadiness, CatalogRole, CatalogSemanticCode,
    CatalogSemanticDiagnostic, CatalogSemanticReport, CatalogVisibility, analyze_catalog_semantics,
    analyze_package_catalog_semantics,
};
pub use connectors::ConnectorRequirement;
pub(crate) use connectors::{bind_operation_steps, validate_connector_requirements};
pub use fixtures::{
    HarnessCallerFixture, HarnessExpectation, HarnessHttpExchangeFixture,
    HarnessHttpRequestBodyFixture, HarnessHttpResponseFixture, HarnessMcpCallFixture,
    OperatorJourneyClaim, OperatorJourneyMode, ReceiptExpectation, RunnerHarnessCase,
    RunnerHarnessManifest,
};
pub use governance::validate_skill_artifact_contract;
pub use markdown::parse_skill_markdown;
pub use runner_definition::validate_input_examples;
pub use source::validate_skill_source;
pub use types::{
    ActDeclaration, ArtifactPageFraming, ArtifactPageSource, InputMode, RawSkillIr,
    SkillArtifactContract, SkillIdempotencyPolicy, SkillInput, SkillRetryPolicy,
    SkillRunnerDefinition, SkillSource, SourceKind, ValidateSkillMode, ValidateSkillOptions,
    ValidatedSkill,
};

pub(crate) use catalog::validate_catalog_metadata;
pub(crate) use environment::validate_environment_requirements;
pub(crate) use fixtures::validate_harness_manifest;
pub(crate) use governance::validate_inputs;
pub(crate) use runner_definition::validate_runner_definition;
pub(crate) use source::validate_inline_graph_source;

use execution_semantics::validate_execution_semantics;
use governance::validate_skill_governance;
use governance::{
    validate_allowed_tools, validate_artifact_contract, validate_idempotency, validate_retry,
};
use source::default_agent_source;
use source::flattened_source_record;
use source::validate_source;
use source::validate_source_fields;

const FIELDS: JsonFieldReader = JsonFieldReader::new("skill");

pub(super) use crate::json_fields::{field_value, first_value};

struct SkillGovernance {
    retry: Option<SkillRetryPolicy>,
    idempotency: Option<SkillIdempotencyPolicy>,
    artifacts: Option<SkillArtifactContract>,
    allowed_tools: Option<Vec<String>>,
    execution: Option<ExecutionSemantics>,
}

pub fn validate_skill(raw: RawSkillIr) -> Result<ValidatedSkill, ValidationError> {
    validate_skill_with_options(raw, ValidateSkillOptions::default())
}

pub fn validate_skill_with_options(
    raw: RawSkillIr,
    options: ValidateSkillOptions,
) -> Result<ValidatedSkill, ValidationError> {
    let runx = validate_runx_metadata(raw.frontmatter.get("runx"), options.mode)?;
    let source = raw
        .frontmatter
        .get("source")
        .map(|value| FIELDS.optional_object(Some(value), "source"))
        .transpose()?
        .flatten()
        .unwrap_or_else(default_agent_source);
    reject_runtime_effect_claims(&raw, runx.as_ref())?;
    let risk = raw.frontmatter.get("risk").cloned();
    let governance = validate_skill_governance(&raw, runx.as_ref())?;
    let category = validate_portable_skill_category(&raw)?;
    let runx_category = validate_runx_skill_category(runx.as_ref())?;
    let registry_owner = validate_registry_owner(&raw)?;

    let source = validate_source(&source)?;
    Ok(ValidatedSkill {
        name: FIELDS.required_string(raw.frontmatter.get("name"), "name")?,
        description: FIELDS.optional_string(raw.frontmatter.get("description"), "description")?,
        category,
        runx_category,
        registry_owner,
        body: raw.body.clone(),
        source,
        inputs: validate_inputs(
            FIELDS
                .optional_object(raw.frontmatter.get("inputs"), "inputs")?
                .unwrap_or_default(),
            "inputs",
        )?,
        auth: raw.frontmatter.get("auth").cloned(),
        risk,
        runtime: raw.frontmatter.get("runtime").cloned(),
        retry: governance.retry,
        idempotency: governance.idempotency,
        artifacts: governance.artifacts,
        allowed_tools: governance.allowed_tools,
        execution: governance.execution,
        runx,
        raw,
    })
}

fn reject_runtime_effect_claims(
    raw: &RawSkillIr,
    runx: Option<&JsonObject>,
) -> Result<(), ValidationError> {
    let risk_claim = raw
        .frontmatter
        .get("risk")
        .and_then(JsonValue::as_object)
        .is_some_and(|risk| risk.contains_key("mutating"));
    if raw.frontmatter.contains_key("mutating")
        || risk_claim
        || runx.is_some_and(|metadata| metadata.contains_key("mutating"))
    {
        return Err(FIELDS.validation_error(
            "mutating is not supported; approval and authority are owned by invoked capabilities",
        ));
    }
    Ok(())
}

fn validate_registry_owner(raw: &RawSkillIr) -> Result<Option<String>, ValidationError> {
    let owner = FIELDS.optional_string(raw.frontmatter.get("registry_owner"), "registry_owner")?;
    let Some(owner) = owner else {
        return Ok(None);
    };
    if owner != owner.to_ascii_lowercase()
        || owner.starts_with('-')
        || owner.ends_with('-')
        || !owner.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(FIELDS
            .validation_error("registry_owner must be a canonical lowercase registry namespace."));
    }
    Ok(Some(owner))
}

fn validate_portable_skill_category(raw: &RawSkillIr) -> Result<Option<String>, ValidationError> {
    Ok(normalize_optional_category(FIELDS.optional_string(
        raw.frontmatter.get("category"),
        "category",
    )?))
}

fn validate_runx_skill_category(
    runx: Option<&JsonObject>,
) -> Result<Option<String>, ValidationError> {
    Ok(normalize_optional_category(FIELDS.optional_string(
        field_value(runx, "category"),
        "runx.category",
    )?))
}

fn normalize_optional_category(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn validate_runx_metadata(
    value: Option<&JsonValue>,
    mode: ValidateSkillMode,
) -> Result<Option<JsonObject>, ValidationError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Object(value)) => Ok(Some(value.clone())),
        Some(_) if mode == ValidateSkillMode::Lenient => Ok(None),
        Some(_) => Err(ValidationError::InvalidField {
            field: "runx".to_owned(),
            message: "runx must be an object when present.".to_owned(),
        }),
    }
}
