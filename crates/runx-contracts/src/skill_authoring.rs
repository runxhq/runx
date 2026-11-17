//! Portable contracts for one digest-bound skill-authoring transaction.

use serde::{Deserialize, Serialize};

use crate::list::{RunxListItemKind, RunxListStatus};
use crate::schema::{NonEmptyString, RunxSchema};
use crate::{Digest, JsonObject};

pub const SKILL_CHANGE_DRAFT_SCHEMA: &str = "runx.skill.change_draft.v1";
pub const SKILL_CHANGE_BUNDLE_SCHEMA: &str = "runx.skill.change_bundle.v1";
pub const SKILL_VALIDATION_RESULT_SCHEMA: &str = "runx.skill.validation_result.v1";
pub const SKILL_APPLY_RESULT_SCHEMA: &str = "runx.skill.apply_result.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum SkillChangeBundleSchema {
    #[serde(rename = "runx.skill.change_bundle.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum SkillChangeDraftSchema {
    #[serde(rename = "runx.skill.change_draft.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum SkillValidationResultSchema {
    #[serde(rename = "runx.skill.validation_result.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum SkillApplyResultSchema {
    #[serde(rename = "runx.skill.apply_result.v1")]
    V1,
}

/// What an authoring run does with the package: `design` checks a candidate
/// and writes nothing, `build` creates one where none stands, `improve`
/// changes one that does, and `test` adds to its tests and fixtures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillAuthoringMode {
    Design,
    Build,
    Improve,
    Test,
}

impl SkillAuthoringMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Design => "design",
            Self::Build => "build",
            Self::Improve => "improve",
            Self::Test => "test",
        }
    }
}

/// What an author decided: to `write` the change, that the package needs
/// `no_change`, that the work is `no_skill`'s, or that it `needs_core`: a
/// capability runx lacks, which its summary names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillChangeDecision {
    Write,
    NoSkill,
    NoChange,
    NeedsCore,
}

/// One file a change writes whole: its path from the workspace root, inside
/// the target package, as the authoring context lists it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillFileWrite {
    pub path: NonEmptyString,
    pub contents: String,
}

/// What an author returns: its decision, why, and the package files it
/// writes and deletes, named from the workspace root as the authoring
/// context names them. The files are the package's contract, and runx checks
/// them. It carries no digest: an author never calculates or copies one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.change_draft.v1")]
pub struct SkillChangeDraft {
    pub schema: SkillChangeDraftSchema,
    pub decision: SkillChangeDecision,
    pub summary: NonEmptyString,
    pub writes: Vec<SkillFileWrite>,
    pub deletes: Vec<NonEmptyString>,
}

/// A draft bound to the package state it was authored against, so a change
/// to the package since then refuses it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.change_bundle.v1")]
pub struct SkillChangeBundle {
    pub schema: SkillChangeBundleSchema,
    pub decision: SkillChangeDecision,
    pub base_digest: NonEmptyString,
    pub summary: NonEmptyString,
    pub writes: Vec<SkillFileWrite>,
    pub deletes: Vec<NonEmptyString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillValidationCheckStatus {
    Passed,
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillValidationCheck {
    pub name: NonEmptyString,
    pub status: SkillValidationCheckStatus,
    pub detail: NonEmptyString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillPackageMetrics {
    pub files: u64,
    pub bytes: u64,
    pub production_lines: u64,
    pub test_lines: u64,
    pub generated_lines: u64,
    pub executable_files: u64,
    pub executable_lines: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillPackageDelta {
    pub files: i64,
    pub bytes: i64,
    pub production_lines: i64,
    pub test_lines: i64,
    pub generated_lines: i64,
    pub executable_files: i64,
    pub executable_lines: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.validation_result.v1")]
pub struct SkillValidationResult {
    pub schema: SkillValidationResultSchema,
    pub target_dir: NonEmptyString,
    pub base_digest: NonEmptyString,
    pub candidate_digest: NonEmptyString,
    pub checks: Vec<SkillValidationCheck>,
    pub before: SkillPackageMetrics,
    pub after: SkillPackageMetrics,
    pub delta: SkillPackageDelta,
    pub residual_risks: Vec<NonEmptyString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillApplyVerdict {
    Unchanged,
    NeedsCore,
    /// A design's candidate validated, and nothing was written.
    Validated,
    ValidatedAndApplied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.apply_result.v1")]
pub struct SkillApplyResult {
    pub schema: SkillApplyResultSchema,
    pub target_dir: NonEmptyString,
    pub decision: SkillChangeDecision,
    pub verdict: SkillApplyVerdict,
    pub base_digest: NonEmptyString,
    /// The package digest of the package standing at the target, as `runx
    /// skill inspect` reports it; none when no package stands there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<NonEmptyString>,
    /// Its distribution digest: the package digest of the package `runx
    /// registry package` makes of it, which every published, bundled or served
    /// copy carries, and which a publish reads back; none when it doesn't
    /// package, as a publish would refuse it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution_digest: Option<NonEmptyString>,
    /// The files written and deleted, from the workspace root.
    pub changed_paths: Vec<NonEmptyString>,
    pub deleted_paths: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<SkillValidationResult>,
    pub residual_risks: Vec<NonEmptyString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum SkillValidationSchema {
    #[serde(rename = "runx.skill.validation.v1")]
    V1,
}

/// How far validating a package got: `tested`, its `test` saying how each
/// case came out, or `validation_failed` before its cases could run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillValidationVerdict {
    Tested,
    ValidationFailed,
}

/// What `runx.skill.validate` returns: one package's inspection and test
/// outcome. A failed validation names its failure in both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.validation.v1")]
pub struct SkillValidation {
    pub schema: SkillValidationSchema,
    pub requested_path: NonEmptyString,
    pub resolved_path: NonEmptyString,
    pub verdict: SkillValidationVerdict,
    pub inspect: JsonObject,
    pub test: JsonObject,
}

/// What `runx.skill.inspect` returns: the workspace a skill change is
/// authored in, its target package as it stands, and the catalog it can
/// reuse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill_lab.authoring_context.v1")]
pub struct SkillAuthoringContext {
    /// The target package, from the workspace root; null when none was named.
    pub target_dir: Option<NonEmptyString>,
    pub target_exists: bool,
    /// The digest a change to the target binds to.
    pub base_digest: Digest,
    /// The target's files, from the workspace root, as the read tools and a
    /// change name them.
    pub target_files: Vec<SkillPackageFile>,
    pub target_metrics: SkillPackageMetrics,
    /// The existing target's inspection, or why it would not inspect; null
    /// when the target does not exist.
    pub target_inspection: Option<JsonObject>,
    /// Where catalog skills live, from the workspace root.
    pub catalog_root: NonEmptyString,
    pub catalog_skills: Vec<SkillCatalogEntry>,
    pub core_tools: Vec<SkillCatalogEntry>,
}

/// What a skill's author is given of the workspace (rule 1): the target's
/// files and what inspecting it found, and what it can reuse, with nothing
/// runx keeps for itself, no digest, closure or path on disk. The record a
/// change binds to is the [`SkillAuthoringContext`] it is made from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillAuthoringBrief {
    /// The target package, from the workspace root; null when none was named.
    pub target_dir: Option<NonEmptyString>,
    pub target_exists: bool,
    /// The target's files, as the read tools and a change name them.
    pub target_files: Vec<SkillPackageFile>,
    pub target_metrics: SkillPackageMetrics,
    /// What inspecting the existing target found, or why it would not
    /// inspect; absent when the target does not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SkillTargetBrief>,
    /// Where catalog skills live.
    pub catalog_root: NonEmptyString,
    pub catalog_skills: Vec<SkillCatalogEntry>,
    pub core_tools: Vec<SkillCatalogEntry>,
}

/// The existing target as its author reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SkillTargetBrief {
    /// It inspects: what it is and what each method takes and returns.
    Ok {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        catalog: Option<crate::SkillCatalog>,
        methods: Vec<SkillMethodBrief>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        eval_queries: Vec<crate::package_evals::EvalQuery>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evals: Vec<crate::package_evals::PackageEval>,
    },
    /// It would not inspect, and why.
    Invalid { error: String },
}

/// One method of the target as its author reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillMethodBrief {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its inputs as one JSON schema, with the method's examples.
    pub input_schema: JsonObject,
    pub outputs: Vec<crate::InspectedOutput>,
}

impl From<&SkillAuthoringContext> for SkillAuthoringBrief {
    fn from(context: &SkillAuthoringContext) -> Self {
        let target = context.target_inspection.as_ref().map(|inspection| {
            match crate::JsonValue::Object(inspection.clone())
                .deserialize_into::<crate::SkillInspection>()
            {
                Ok(inspected) => SkillTargetBrief::Ok {
                    name: inspected.name,
                    description: inspected.description,
                    version: inspected.version,
                    catalog: inspected.catalog,
                    methods: inspected
                        .method_inspections
                        .into_iter()
                        .map(|entry| SkillMethodBrief {
                            name: entry.method.name,
                            description: entry.method.description,
                            input_schema: entry.method.input_schema,
                            outputs: entry.method.outputs,
                        })
                        .collect(),
                    eval_queries: inspected.eval_queries,
                    evals: inspected.evals,
                },
                Err(_) => SkillTargetBrief::Invalid {
                    error: inspection
                        .get("error")
                        .and_then(crate::JsonValue::as_str)
                        .unwrap_or("it does not inspect")
                        .to_owned(),
                },
            }
        });
        Self {
            target_dir: context.target_dir.clone(),
            target_exists: context.target_exists,
            target_files: context.target_files.clone(),
            target_metrics: context.target_metrics.clone(),
            target,
            catalog_root: context.catalog_root.clone(),
            catalog_skills: context.catalog_skills.clone(),
            core_tools: context.core_tools.clone(),
        }
    }
}

/// One file of a skill package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillPackageFile {
    pub path: NonEmptyString,
    pub bytes: u64,
}

/// One skill or tool an authored skill can reuse.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillCatalogEntry {
    pub name: NonEmptyString,
    pub kind: RunxListItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<NonEmptyString>,
    pub path: NonEmptyString,
    pub status: RunxListStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixtures: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_cases: Option<u64>,
}

#[cfg(test)]
mod tests;
