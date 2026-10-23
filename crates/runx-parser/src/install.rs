use serde::{Deserialize, Serialize};

use crate::{ParseError, ValidatedSkill, ValidationError, parse_skill_markdown, validate_skill};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInstallOrigin {
    pub source: String,
    pub source_label: String,
    pub package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_tier: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidatedSkillInstall {
    pub skill: ValidatedSkill,
    pub origin: SkillInstallOrigin,
    pub markdown: String,
}

pub fn validate_skill_install(
    markdown: &str,
    origin: SkillInstallOrigin,
) -> Result<ValidatedSkillInstall, SkillInstallError> {
    let raw = parse_skill_markdown(markdown).map_err(SkillInstallError::Parse)?;
    let skill = validate_skill(raw).map_err(SkillInstallError::Validation)?;
    Ok(ValidatedSkillInstall {
        skill,
        origin,
        markdown: markdown.to_owned(),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum SkillInstallError {
    #[error("{0}")]
    Parse(ParseError),
    #[error("{0}")]
    Validation(ValidationError),
}
