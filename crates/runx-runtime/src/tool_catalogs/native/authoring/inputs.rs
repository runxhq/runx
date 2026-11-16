use runx_contracts::{
    JsonObject, JsonValue, SkillAuthoringContext, SkillAuthoringMode, SkillChangeBundle,
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
    pub(super) mode: SkillAuthoringMode,
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
pub(super) struct BindInput {
    pub(super) authoring_context: SkillAuthoringContext,
    pub(super) mode: SkillAuthoringMode,
    pub(super) change_draft: SkillChangeDraft,
    /// For a package kept from a turn's script call: the call as
    /// `call.read` gave it, whose source the package's module is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) script_call: Option<runx_contracts::Artifact>,
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
    pub(super) allow_execute_test: bool,
}

impl CapabilityInput for ValidateInput {
    fn defaults() -> JsonObject {
        let mut defaults = root_default();
        defaults.insert("allow_execute_test".to_owned(), JsonValue::Bool(false));
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
    field(
        "mode",
        "design validates the change and writes nothing; build creates a package where none stands; improve changes one that does; test adds to its tests and fixtures.",
    ),
    field(
        "change_bundle",
        "Digest-bound, closed skill change contract.",
    ),
];

pub(super) const BIND_FIELDS: &[CapabilityField] = &[
    field(
        "authoring_context",
        "What the matching runx.skill.inspect call returned: the change binds its base digest.",
    ),
    field(
        "mode",
        "The authoring mode, which the change's decision must fit.",
    ),
    field(
        "change_draft",
        "The author's decision, summary, writes and deletes.",
    ),
    field(
        "script_call",
        "For a package kept from a turn's script call, the call as call.read gave it: runx writes its source as the package's script.mjs, byte for byte, and refuses a draft that writes that file otherwise.",
    ),
];

pub(super) const VALIDATE_FIELDS: &[CapabilityField] = &[
    root_field(),
    field(
        "skill_path",
        "Package path relative to the workspace or owning skill, or a registry reference, <publisher>/<name>@<version>, read into runx's cache.",
    ),
    field(
        "candidate_files",
        "Bounded standalone candidate package files.",
    ),
    field(
        "allow_execute_test",
        "Let a case run a trusted host process. Without it, a case that reaches one is refused there, reported inconclusive and never run, and the package's other cases run.",
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
