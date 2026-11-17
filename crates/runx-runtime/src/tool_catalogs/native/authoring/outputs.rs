use runx_contracts::{
    SkillApplyResult, SkillAuthoringBrief, SkillAuthoringContext, SkillChangeBundle,
    SkillValidation,
};
use serde::{Deserialize, Serialize};

use crate::CapabilityOutput;

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthoringInspectOutput {
    /// The record a change binds to.
    pub(super) authoring_context: SkillAuthoringContext,
    /// What the change's author is given of it.
    pub(super) authoring_brief: SkillAuthoringBrief,
}

impl CapabilityOutput for AuthoringInspectOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ValidateOutput {
    pub(super) skill_validation: SkillValidation,
}

impl CapabilityOutput for ValidateOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindOutput {
    pub(super) change_bundle: SkillChangeBundle,
}

impl CapabilityOutput for BindOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ApplyOutput {
    pub(super) apply_result: SkillApplyResult,
}

impl CapabilityOutput for ApplyOutput {}
