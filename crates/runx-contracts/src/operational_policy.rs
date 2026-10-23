//! Operational policy contracts for governed source, runner, target, and owner routing.
//
// Type definitions live here; the validation, admission, and readback projection
// logic lives in the private `evaluate` submodule.
// Module rationale: the operational policy schema, rules,
// and decision shapes form one cross-language wire surface.
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::JsonValue;
use crate::schema::{
    Identity, IsoDateTime, NamedSchemas, NonEmptyString, Property, RunxSchema, object_schema,
    record_named, titled,
};

mod evaluate;

pub use evaluate::{
    admit_operational_policy_request, lint_operational_policy_contract,
    project_operational_policy_readback, validate_operational_policy_contract,
    validate_operational_policy_semantics,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum OperationalPolicySchema {
    #[serde(rename = "runx.operational_policy.v1")]
    V1,
}

impl fmt::Display for OperationalPolicySchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("runx.operational_policy.v1")
    }
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, RunxSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum OperationalPolicyAction {
    ReplyOnly,
    IssueIntake,
    WorkPlan,
    IssueToPr,
    ManualReview,
    PrReview,
    PrFixUp,
    MergeAssist,
}

impl fmt::Display for OperationalPolicyAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(action_name(*self))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "lowercase")]
pub enum OperationalPolicyRunnerState {
    Available,
    Disabled,
    Maintenance,
}

impl fmt::Display for OperationalPolicyRunnerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Available => "available",
            Self::Disabled => "disabled",
            Self::Maintenance => "maintenance",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationalPolicyDedupeStrategy {
    SourceDigest,
    ProviderSearch,
    Branch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationalPolicyOutcomeCloseMode {
    Never,
    WhenVerified,
    WhenTerminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "lowercase")]
pub enum OperationalPolicyPublishMode {
    Reply,
    Comment,
    None,
}

impl fmt::Display for OperationalPolicyPublishMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Reply => "reply",
            Self::Comment => "comment",
            Self::None => "none",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
pub enum OperationalPolicyMissingBehavior {
    #[serde(rename = "fail_closed")]
    FailClosed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "lowercase")]
pub enum OperationalPolicyDuplicateBehavior {
    Reuse,
    Comment,
    Block,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicy {
    pub schema: OperationalPolicySchema,
    pub schema_version: OperationalPolicySchema,
    pub policy_id: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<IsoDateTime>,
    pub sources: Vec<OperationalPolicySourceRule>,
    pub runners: Vec<OperationalPolicyRunnerRule>,
    pub owner_routes: Vec<OperationalPolicyOwnerRoute>,
    pub targets: Vec<OperationalPolicyTargetRule>,
    pub dedupe: OperationalPolicyDedupePolicy,
    pub outcomes: OperationalPolicyOutcomePolicy,
    pub permissions: OperationalPolicyAutomationPermissions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicySourceRule {
    pub source_id: NonEmptyString,
    /// Open provider identifier, the id of the connector the source arrives
    /// through. Any value an adapter publishes is accepted on the wire.
    pub provider: NonEmptyString,
    pub allowed_locators: Vec<NonEmptyString>,
    pub allowed_actions: Vec<OperationalPolicyAction>,
    pub source_thread: OperationalPolicySourceThreadPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_confidence: Option<f64>,
    /// Open per-provider adapter policy bag, keyed by adapter identifier
    /// (typically the source provider id). Adapters validate their own slice;
    /// the contract layer carries the JSON through untyped so new providers do
    /// not require a contract edit.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub adapter_policy: BTreeMap<NonEmptyString, JsonValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicySourceThreadPolicy {
    pub required: bool,
    pub publish_mode: OperationalPolicyPublishMode,
    pub missing_behavior: OperationalPolicyMissingBehavior,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyRunnerRule {
    pub runner_id: NonEmptyString,
    /// Open runner kind identifier. Any value an adapter publishes is accepted
    /// on the wire.
    pub kind: NonEmptyString,
    pub state: OperationalPolicyRunnerState,
    pub allowed_actions: Vec<OperationalPolicyAction>,
    pub target_repos: Vec<String>,
    pub scafld_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyOwnerRoute {
    pub route_id: NonEmptyString,
    pub owners: Vec<NonEmptyString>,
    pub target_repos: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<NonEmptyString>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyTargetRule {
    pub repo: String,
    pub runner_ids: Vec<NonEmptyString>,
    pub allowed_actions: Vec<OperationalPolicyAction>,
    pub default_owner_route: NonEmptyString,
    pub scafld_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<NonEmptyString>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyDedupePolicy {
    pub strategy: OperationalPolicyDedupeStrategy,
    pub key_fields: Vec<NonEmptyString>,
    pub on_duplicate: OperationalPolicyDuplicateBehavior,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyOutcomePolicy {
    pub observe_provider: bool,
    pub verification_required: bool,
    pub close_source_issue: OperationalPolicyOutcomeCloseMode,
    pub publish_final_source_thread_update: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyAutomationPermissions {
    pub auto_merge: bool,
    pub mutate_target_repo: bool,
    pub require_human_merge_gate: bool,
}

impl RunxSchema for OperationalPolicy {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("schema", OperationalPolicySchema::json_schema(), true),
                Property::new(
                    "schema_version",
                    OperationalPolicySchema::json_schema(),
                    true,
                ),
                Property::new("policy_id", id_schema(), true),
                Property::new("created_at", IsoDateTime::json_schema(), false),
                Property::new(
                    "sources",
                    non_empty_array(OperationalPolicySourceRule::json_schema()),
                    true,
                ),
                Property::new(
                    "runners",
                    non_empty_array(OperationalPolicyRunnerRule::json_schema()),
                    true,
                ),
                Property::new(
                    "owner_routes",
                    non_empty_array(OperationalPolicyOwnerRoute::json_schema()),
                    true,
                ),
                Property::new(
                    "targets",
                    non_empty_array(OperationalPolicyTargetRule::json_schema()),
                    true,
                ),
                Property::new("dedupe", OperationalPolicyDedupePolicy::json_schema(), true),
                Property::new(
                    "outcomes",
                    OperationalPolicyOutcomePolicy::json_schema(),
                    true,
                ),
                Property::new(
                    "permissions",
                    OperationalPolicyAutomationPermissions::json_schema(),
                    true,
                ),
            ],
            true,
            Some(Identity::Runx {
                logical: "runx.operational_policy.v1",
                url: None,
            }),
        );
        titled(schema, "OperationalPolicy")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "OperationalPolicy", Self::json_schema) {
            OperationalPolicySchema::named_schemas(named);
            OperationalPolicySourceRule::named_schemas(named);
            OperationalPolicyRunnerRule::named_schemas(named);
            OperationalPolicyOwnerRoute::named_schemas(named);
            OperationalPolicyTargetRule::named_schemas(named);
            OperationalPolicyDedupePolicy::named_schemas(named);
            OperationalPolicyOutcomePolicy::named_schemas(named);
            OperationalPolicyAutomationPermissions::named_schemas(named);
        }
    }
}

impl RunxSchema for OperationalPolicySourceRule {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("source_id", id_schema(), true),
                Property::new("provider", NonEmptyString::json_schema(), true),
                Property::new(
                    "allowed_locators",
                    non_empty_array(NonEmptyString::json_schema()),
                    true,
                ),
                Property::new(
                    "allowed_actions",
                    non_empty_array(OperationalPolicyAction::json_schema()),
                    true,
                ),
                Property::new(
                    "source_thread",
                    OperationalPolicySourceThreadPolicy::json_schema(),
                    true,
                ),
                Property::new("minimum_confidence", confidence_schema(), false),
                Property::new("adapter_policy", adapter_policy_schema(), false),
            ],
            true,
            None,
        );
        titled(schema, "OperationalPolicySourceRule")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "OperationalPolicySourceRule", Self::json_schema) {
            OperationalPolicyAction::named_schemas(named);
            OperationalPolicySourceThreadPolicy::named_schemas(named);
        }
    }
}

impl RunxSchema for OperationalPolicyRunnerRule {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("runner_id", id_schema(), true),
                Property::new("kind", NonEmptyString::json_schema(), true),
                Property::new("state", OperationalPolicyRunnerState::json_schema(), true),
                Property::new(
                    "allowed_actions",
                    non_empty_array(OperationalPolicyAction::json_schema()),
                    true,
                ),
                Property::new("target_repos", non_empty_array(repo_slug_schema()), true),
                Property::new("scafld_required", bool::json_schema(), true),
            ],
            true,
            None,
        );
        titled(schema, "OperationalPolicyRunnerRule")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "OperationalPolicyRunnerRule", Self::json_schema) {
            OperationalPolicyRunnerState::named_schemas(named);
            OperationalPolicyAction::named_schemas(named);
        }
    }
}

impl RunxSchema for OperationalPolicyOwnerRoute {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("route_id", id_schema(), true),
                Property::new(
                    "owners",
                    non_empty_array(NonEmptyString::json_schema()),
                    true,
                ),
                Property::new("target_repos", non_empty_array(repo_slug_schema()), true),
                Property::new("labels", Vec::<NonEmptyString>::json_schema(), false),
                Property::new("project", NonEmptyString::json_schema(), false),
            ],
            true,
            None,
        );
        titled(schema, "OperationalPolicyOwnerRoute")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        record_named(named, "OperationalPolicyOwnerRoute", Self::json_schema);
    }
}

impl RunxSchema for OperationalPolicyTargetRule {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("repo", repo_slug_schema(), true),
                Property::new("runner_ids", non_empty_array(id_schema()), true),
                Property::new(
                    "allowed_actions",
                    non_empty_array(OperationalPolicyAction::json_schema()),
                    true,
                ),
                Property::new("default_owner_route", id_schema(), true),
                Property::new("scafld_required", bool::json_schema(), true),
                Property::new("base_branch", NonEmptyString::json_schema(), false),
            ],
            true,
            None,
        );
        titled(schema, "OperationalPolicyTargetRule")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "OperationalPolicyTargetRule", Self::json_schema) {
            OperationalPolicyAction::named_schemas(named);
        }
    }
}

impl RunxSchema for OperationalPolicyDedupePolicy {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new(
                    "strategy",
                    OperationalPolicyDedupeStrategy::json_schema(),
                    true,
                ),
                Property::new(
                    "key_fields",
                    non_empty_array(NonEmptyString::json_schema()),
                    true,
                ),
                Property::new(
                    "on_duplicate",
                    OperationalPolicyDuplicateBehavior::json_schema(),
                    true,
                ),
            ],
            true,
            None,
        );
        titled(schema, "OperationalPolicyDedupePolicy")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "OperationalPolicyDedupePolicy", Self::json_schema) {
            OperationalPolicyDedupeStrategy::named_schemas(named);
            OperationalPolicyDuplicateBehavior::named_schemas(named);
        }
    }
}

impl RunxSchema for OperationalPolicyAutomationPermissions {
    fn json_schema() -> Value {
        let schema = object_schema(
            vec![
                Property::new("auto_merge", const_bool(false), true),
                Property::new("mutate_target_repo", bool::json_schema(), true),
                Property::new("require_human_merge_gate", const_bool(true), true),
            ],
            true,
            None,
        );
        titled(schema, "OperationalPolicyAutomationPermissions")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        record_named(
            named,
            "OperationalPolicyAutomationPermissions",
            Self::json_schema,
        );
    }
}

fn repo_slug_schema() -> Value {
    json!({
        "minLength": 3,
        "pattern": "^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$",
        "type": "string"
    })
}

fn id_schema() -> Value {
    json!({
        "minLength": 1,
        "pattern": "^[A-Za-z0-9_.:-]+$",
        "type": "string"
    })
}

fn confidence_schema() -> Value {
    json!({ "minimum": 0, "maximum": 1, "type": "number" })
}

fn non_empty_array(item_schema: Value) -> Value {
    json!({ "items": item_schema, "minItems": 1, "type": "array" })
}

fn const_bool(value: bool) -> Value {
    json!({ "const": value, "type": "boolean" })
}

fn adapter_policy_schema() -> Value {
    json!({
        "additionalProperties": JsonValue::json_schema(),
        "propertyNames": NonEmptyString::json_schema(),
        "type": "object",
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyValidationFinding {
    pub code: NonEmptyString,
    pub path: NonEmptyString,
    pub message: NonEmptyString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationalPolicyAdmissionRequest {
    pub source_id: Option<String>,
    pub target_repo: Option<String>,
    pub action: OperationalPolicyAction,
    pub runner_id: Option<String>,
    pub source_thread_locator: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OperationalPolicyAdmission {
    pub status: OperationalPolicyAdmissionStatus,
    pub findings: Vec<OperationalPolicyValidationFinding>,
    pub policy_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_route_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owners: Option<Vec<String>>,
    pub dedupe_strategy: OperationalPolicyDedupeStrategy,
    pub outcome_close_mode: OperationalPolicyOutcomeCloseMode,
    pub source_thread_required: bool,
    pub mutate_target_repo: bool,
    pub require_human_merge_gate: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationalPolicyAdmissionStatus {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationalPolicyError {
    Contract(OperationalPolicyValidationFinding),
    Semantic(OperationalPolicyValidationFinding),
}

impl OperationalPolicyError {
    pub fn finding(&self) -> &OperationalPolicyValidationFinding {
        match self {
            Self::Contract(finding) | Self::Semantic(finding) => finding,
        }
    }
}

impl fmt::Display for OperationalPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let finding = self.finding();
        write!(
            formatter,
            "{} failed validation ({}): {}",
            finding.path, finding.code, finding.message
        )
    }
}

impl std::error::Error for OperationalPolicyError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyReadback {
    pub policy_id: String,
    pub schema_version: OperationalPolicySchema,
    pub valid: bool,
    pub findings: Vec<OperationalPolicyValidationFinding>,
    pub sources: Vec<OperationalPolicySourceReadback>,
    pub runners: Vec<OperationalPolicyRunnerReadback>,
    pub targets: Vec<OperationalPolicyTargetReadback>,
    pub outcomes: OperationalPolicyOutcomePolicy,
    pub permissions: OperationalPolicyAutomationPermissions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicySourceReadback {
    pub source_id: String,
    pub provider: NonEmptyString,
    pub locator_count: usize,
    pub allowed_actions: Vec<OperationalPolicyAction>,
    pub source_thread_required: bool,
    pub publish_mode: OperationalPolicyPublishMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyRunnerReadback {
    pub runner_id: String,
    pub kind: NonEmptyString,
    pub state: OperationalPolicyRunnerState,
    pub target_repos: Vec<String>,
    pub allowed_actions: Vec<OperationalPolicyAction>,
    pub scafld_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationalPolicyTargetReadback {
    pub repo: String,
    pub runner_ids: Vec<String>,
    pub default_owner_route: String,
    pub owner_count: usize,
    pub allowed_actions: Vec<OperationalPolicyAction>,
    pub scafld_required: bool,
    pub available_runner_count: usize,
}

fn action_name(action: OperationalPolicyAction) -> &'static str {
    match action {
        OperationalPolicyAction::ReplyOnly => "reply-only",
        OperationalPolicyAction::IssueIntake => "issue-intake",
        OperationalPolicyAction::WorkPlan => "work-plan",
        OperationalPolicyAction::IssueToPr => "issue-to-pr",
        OperationalPolicyAction::ManualReview => "manual-review",
        OperationalPolicyAction::PrReview => "pr-review",
        OperationalPolicyAction::PrFixUp => "pr-fix-up",
        OperationalPolicyAction::MergeAssist => "merge-assist",
    }
}
