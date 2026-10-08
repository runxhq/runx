//! Portable contracts for one digest-bound skill-authoring transaction.

use serde::{Deserialize, Serialize};

use crate::schema::{NonEmptyString, RunxSchema};

pub const SKILL_ARCHITECTURE_DECISION_SCHEMA: &str = "runx.skill.architecture_decision.v1";
pub const SKILL_ARCHITECTURE_PLAN_SCHEMA: &str = "runx.skill.architecture_plan.v1";
pub const SKILL_CHANGE_DRAFT_SCHEMA: &str = "runx.skill.change_draft.v1";
pub const SKILL_CHANGE_BUNDLE_SCHEMA: &str = "runx.skill.change_bundle.v1";
pub const SKILL_VALIDATION_RESULT_SCHEMA: &str = "runx.skill.validation_result.v1";
pub const SKILL_APPLY_RESULT_SCHEMA: &str = "runx.skill.apply_result.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum SkillArchitectureDecisionSchema {
    #[serde(rename = "runx.skill.architecture_decision.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum SkillArchitecturePlanSchema {
    #[serde(rename = "runx.skill.architecture_plan.v1")]
    V1,
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillArchitectureDisposition {
    Build,
    ExtendExisting,
    NoSkill,
    NeedsCore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillExecutionLane {
    Manual,
    Graph,
    Agent,
    NativeCapability,
    DomainModule,
    Process,
    Operation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillEffectClass {
    None,
    Read,
    Draft,
    Mutate,
    Financial,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillApprovalRequirement {
    None,
    Policy,
    Human,
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, RunxSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SkillProofKind {
    Contract,
    Harness,
    Regression,
    Security,
    Performance,
    SelectionTrial,
    StandaloneOperatorJourney,
    ComposedOperatorJourney,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillIdentityAction {
    Keep,
    Rename,
    Create,
    Internalize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillPackageVisibility {
    Public,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillIdentityDecision {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_name: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_name: Option<NonEmptyString>,
    pub action: SkillIdentityAction,
    pub visibility: SkillPackageVisibility,
    pub rationale: NonEmptyString,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDirectUseContract {
    pub trigger_requests: Vec<NonEmptyString>,
    pub non_trigger_requests: Vec<NonEmptyString>,
    pub default_outcome: NonEmptyString,
    pub routine_host_work: Vec<NonEmptyString>,
    pub runx_boundary: NonEmptyString,
    pub terminal_result: NonEmptyString,
    pub blocker_behavior: NonEmptyString,
    pub native_escape: NonEmptyString,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillChainUseContract {
    pub accepted_inputs: Vec<NonEmptyString>,
    pub result: NonEmptyString,
    pub reused_evidence: Vec<NonEmptyString>,
    pub reused_effects: Vec<NonEmptyString>,
    pub must_not_repeat: Vec<NonEmptyString>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillKnowledgeContract {
    pub purpose: NonEmptyString,
    pub evidence_required: Vec<NonEmptyString>,
    pub decision_logic: Vec<NonEmptyString>,
    pub stop_conditions: Vec<NonEmptyString>,
    pub recovery: Vec<NonEmptyString>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillBehaviorDecision {
    pub id: NonEmptyString,
    pub outcome: NonEmptyString,
    pub lane: SkillExecutionLane,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse_ref: Option<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain_module_justification: Option<NonEmptyString>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillNativeReuseEvidence {
    pub inspected_capabilities: Vec<NonEmptyString>,
    pub selected_capabilities: Vec<NonEmptyString>,
    pub missing_capabilities: Vec<NonEmptyString>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillEffectRequirement {
    pub effect: SkillEffectClass,
    pub authority_scopes: Vec<NonEmptyString>,
    pub approval: SkillApprovalRequirement,
    pub provider_boundary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillChainPlan {
    pub context_skills: Vec<NonEmptyString>,
    pub routes: Vec<NonEmptyString>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillResourceBudget {
    pub max_files: u64,
    pub max_executable_lines: u64,
    pub max_process_spawns: u64,
    pub network_allowed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillProofRequirement {
    pub name: NonEmptyString,
    pub kind: SkillProofKind,
    pub expected: NonEmptyString,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.architecture_decision.v1")]
pub struct SkillArchitectureDecision {
    pub schema: SkillArchitectureDecisionSchema,
    pub disposition: SkillArchitectureDisposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<SkillIdentityDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_use: Option<SkillDirectUseContract>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_use: Option<SkillChainUseContract>,
    pub objective: NonEmptyString,
    pub operator_value: NonEmptyString,
    pub knowledge_contract: SkillKnowledgeContract,
    pub required_behaviors: Vec<SkillBehaviorDecision>,
    pub native_reuse: SkillNativeReuseEvidence,
    pub effects: Vec<SkillEffectRequirement>,
    pub skill_chain: SkillChainPlan,
    pub resource_budget: SkillResourceBudget,
    pub preservation_obligations: Vec<NonEmptyString>,
    pub deletions: Vec<NonEmptyString>,
    pub proof_plan: Vec<SkillProofRequirement>,
}

/// The exact architecture decision admitted against one inspected package
/// state. `plan_digest` binds both fields so a change bundle cannot silently
/// reuse the decision after the target package changes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.architecture_plan.v1")]
pub struct SkillArchitecturePlan {
    pub schema: SkillArchitecturePlanSchema,
    pub base_digest: NonEmptyString,
    pub plan_digest: NonEmptyString,
    pub architecture: SkillArchitectureDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillChangeDecision {
    Write,
    NoSkill,
    NoChange,
    NeedsCore,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillFileWrite {
    pub path: NonEmptyString,
    pub contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillExpectedOutput {
    pub name: NonEmptyString,
    pub value_type: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet: Option<NonEmptyString>,
}

/// Agent-authored package bytes and intent before the runtime binds them to an
/// inspected package state. This contract deliberately contains no digest:
/// agents never calculate or copy runtime integrity values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.change_draft.v1")]
pub struct SkillChangeDraft {
    pub schema: SkillChangeDraftSchema,
    pub decision: SkillChangeDecision,
    pub summary: NonEmptyString,
    pub non_goals: Vec<NonEmptyString>,
    pub writes: Vec<SkillFileWrite>,
    pub deletes: Vec<NonEmptyString>,
    pub expected_outputs: Vec<SkillExpectedOutput>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.skill.change_bundle.v1")]
pub struct SkillChangeBundle {
    pub schema: SkillChangeBundleSchema,
    pub decision: SkillChangeDecision,
    pub base_digest: NonEmptyString,
    pub plan_digest: NonEmptyString,
    pub architecture: SkillArchitectureDecision,
    pub summary: NonEmptyString,
    pub non_goals: Vec<NonEmptyString>,
    pub writes: Vec<SkillFileWrite>,
    pub deletes: Vec<NonEmptyString>,
    pub expected_outputs: Vec<SkillExpectedOutput>,
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
    pub plan_digest: NonEmptyString,
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
    pub plan_digest: NonEmptyString,
    pub package_digest: NonEmptyString,
    pub changed_paths: Vec<NonEmptyString>,
    pub deleted_paths: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<SkillValidationResult>,
    pub residual_risks: Vec<NonEmptyString>,
}

#[cfg(test)]
mod tests;
