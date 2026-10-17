use runx_contracts::JsonValue;

use crate::{ValidationError, json_fields::JsonFieldReader};

mod catalog;
mod connectors;
mod environment;
mod fixtures;
mod governance;
mod markdown;
mod runner_definition;
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
    HarnessCaller, HarnessExpectation, HarnessHttpExchangeFixture, HarnessHttpRequestBodyFixture,
    HarnessHttpResponseFixture, HarnessMcpCallFixture, HarnessProcessCallFixture,
    OperatorJourneyClaim, OperatorJourneyMode, ReceiptExpectation, RunnerHarnessCase,
    RunnerHarnessManifest,
};
pub use governance::validate_skill_artifact_contract;
pub use markdown::parse_skill_markdown;
pub use runner_definition::validate_input_examples;
pub use types::{
    ArtifactPageFraming, ArtifactPageSource, InputMode, RawSkillIr, SkillArtifactContract,
    SkillIdempotencyPolicy, SkillInput, SkillRunnerDefinition, ValidateSkillMode,
    ValidateSkillOptions, ValidatedSkill,
};

pub(crate) use catalog::validate_catalog_metadata;
pub(crate) use environment::validate_environment_requirements;
pub(crate) use fixtures::validate_harness_manifest;
pub(crate) use governance::validate_inputs;
pub(crate) use runner_definition::validate_runner_definition;

const FIELDS: JsonFieldReader = JsonFieldReader::new("skill");

/// The frontmatter a `SKILL.md` may carry: exactly the Agent Skills fields.
/// Execution and catalog metadata belong in `run.yaml`.
const FRONTMATTER_FIELDS: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];

const MAX_NAME_CHARS: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_COMPATIBILITY_CHARS: usize = 500;

pub fn validate_skill(raw: RawSkillIr) -> Result<ValidatedSkill, ValidationError> {
    validate_skill_with_options(raw, ValidateSkillOptions::default())
}

/// Validate a `SKILL.md`. Strict mode applies the Agent Skills rules to the
/// frontmatter; lenient mode reads a third-party manual as far as it can.
pub fn validate_skill_with_options(
    raw: RawSkillIr,
    options: ValidateSkillOptions,
) -> Result<ValidatedSkill, ValidationError> {
    if options.mode == ValidateSkillMode::Strict {
        validate_agent_skills_frontmatter(&raw)?;
    }
    Ok(ValidatedSkill {
        name: FIELDS.required_string(raw.frontmatter.get("name"), "name")?,
        description: FIELDS.optional_string(raw.frontmatter.get("description"), "description")?,
        body: raw.body.clone(),
        raw,
    })
}

/// The Agent Skills specification's frontmatter rules, as `skills-ref`
/// enforces them. The name's match with its directory is a package rule,
/// checked where the directory is known.
fn validate_agent_skills_frontmatter(raw: &RawSkillIr) -> Result<(), ValidationError> {
    let frontmatter = &raw.frontmatter;
    FIELDS.reject_unknown_fields(frontmatter, "SKILL.md", FRONTMATTER_FIELDS)?;
    let name = FIELDS.required_string(frontmatter.get("name"), "name")?;
    validate_skill_name(&name)?;
    let description = FIELDS
        .optional_non_empty_string(frontmatter.get("description"), "description")?
        .ok_or_else(|| ValidationError::MissingField {
            field: "description".to_owned(),
        })?;
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(FIELDS.validation_error(format!(
            "description must be at most {MAX_DESCRIPTION_CHARS} characters."
        )));
    }
    FIELDS.optional_string(frontmatter.get("license"), "license")?;
    if let Some(compatibility) =
        FIELDS.optional_string(frontmatter.get("compatibility"), "compatibility")?
        && compatibility.chars().count() > MAX_COMPATIBILITY_CHARS
    {
        return Err(FIELDS.validation_error(format!(
            "compatibility must be at most {MAX_COMPATIBILITY_CHARS} characters."
        )));
    }
    if let Some(metadata) = FIELDS.optional_object(frontmatter.get("metadata"), "metadata")?
        && !metadata
            .values()
            .all(|value| matches!(value, JsonValue::String(_)))
    {
        return Err(FIELDS.validation_error("metadata must map string keys to string values."));
    }
    FIELDS.optional_string(frontmatter.get("allowed-tools"), "allowed-tools")?;
    Ok(())
}

/// At most 64 characters of lowercase ASCII letters, digits and hyphens, with
/// no leading, trailing or consecutive hyphen: `^[a-z0-9]+(-[a-z0-9]+)*$`.
fn validate_skill_name(name: &str) -> Result<(), ValidationError> {
    let valid = name.len() <= MAX_NAME_CHARS
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });
    if valid {
        return Ok(());
    }
    Err(FIELDS.validation_error(format!(
        "name '{name}' must be at most {MAX_NAME_CHARS} lowercase letters, digits and hyphens, without a leading, trailing or doubled hyphen."
    )))
}
