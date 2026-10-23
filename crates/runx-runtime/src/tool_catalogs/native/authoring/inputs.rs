use runx_contracts::{
    JsonObject, JsonValue, SkillArchitectureDecision, SkillArchitecturePlan, SkillChangeBundle,
    SkillChangeDraft,
};
use serde::{Deserialize, Serialize};

use crate::{CapabilityField, CapabilityInput};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InspectInput {
    pub(super) repo_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) target_dir: Option<String>,
}

impl CapabilityInput for InspectInput {
    fn defaults() -> JsonObject {
        root_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ApplyInput {
    pub(super) repo_root: String,
    pub(super) target_dir: String,
    pub(super) mode: String,
    pub(super) change_bundle: SkillChangeBundle,
}

impl CapabilityInput for ApplyInput {
    fn defaults() -> JsonObject {
        let mut defaults = root_default();
        defaults.insert("mode".to_owned(), JsonValue::String("build".to_owned()));
        defaults
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanInput {
    pub(super) base_digest: String,
    pub(super) architecture: SkillArchitectureDecision,
}

impl CapabilityInput for PlanInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BindInput {
    pub(super) architecture_plan: SkillArchitecturePlan,
    pub(super) change_draft: SkillChangeDraft,
}

impl CapabilityInput for BindInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ValidateInput {
    pub(super) repo_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) skill_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) candidate_files: Option<Vec<JsonValue>>,
    pub(super) allow_execute_harness: bool,
}

impl CapabilityInput for ValidateInput {
    fn defaults() -> JsonObject {
        let mut defaults = root_default();
        defaults.insert("allow_execute_harness".to_owned(), JsonValue::Bool(false));
        defaults
    }
}

pub(super) const INSPECT_FIELDS: &[CapabilityField] = &[
    root_field(),
    field(
        "target_dir",
        "Optional package path relative to the project root.",
    ),
];

pub(super) const APPLY_FIELDS: &[CapabilityField] = &[
    root_field(),
    field("target_dir", "Package path relative to the project root."),
    field("mode", "Authoring boundary: build, improve, or harness."),
    field(
        "change_bundle",
        "Digest-bound, closed skill change contract.",
    ),
];

pub(super) const PLAN_FIELDS: &[CapabilityField] = &[
    field(
        "base_digest",
        "Package digest returned by the matching runx.skill.inspect call.",
    ),
    field(
        "architecture",
        "Closed architecture decision to validate and bind to the inspected package.",
    ),
];

pub(super) const BIND_FIELDS: &[CapabilityField] = &[
    field(
        "architecture_plan",
        "Native digest-bound architecture plan returned by runx.skill.plan.",
    ),
    field(
        "change_draft",
        "Closed agent-authored file and intent draft without integrity fields.",
    ),
];

pub(super) const VALIDATE_FIELDS: &[CapabilityField] = &[
    root_field(),
    field(
        "skill_path",
        "Package path relative to the workspace or owning skill.",
    ),
    field(
        "candidate_files",
        "Bounded standalone candidate package files.",
    ),
    field(
        "allow_execute_harness",
        "Explicitly permit harnessing a package classified as execute.",
    ),
];

const fn field(name: &'static str, description: &'static str) -> CapabilityField {
    CapabilityField { name, description }
}

const fn root_field() -> CapabilityField {
    field("repo_root", "Workspace-relative project root.")
}

fn root_default() -> JsonObject {
    JsonObject::from([("repo_root".to_owned(), JsonValue::String(".".to_owned()))])
}
