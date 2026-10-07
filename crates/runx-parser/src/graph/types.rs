use std::collections::BTreeMap;

use runx_contracts::connector::OperationName;
use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::ValidationError;
use crate::skill::{SkillArtifactContract, SkillSource, SourceKind, validate_inline_graph_source};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawGraphIr {
    pub document: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphContextEdge {
    pub input: String,
    pub from_step: String,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphRetryPolicy {
    pub max_attempts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanoutSyncStrategy {
    All,
    Any,
    Quorum,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanoutBranchFailurePolicy {
    Halt,
    Continue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanoutThresholdAction {
    Pause,
    Escalate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FanoutThresholdGate {
    pub step: String,
    pub field: String,
    pub above: f64,
    pub action: FanoutThresholdAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanoutConflictAction {
    Pause,
    Escalate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FanoutConflictGate {
    pub field: String,
    pub steps: Vec<String>,
    pub action: FanoutConflictAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FanoutGroupPolicy {
    pub group_id: String,
    pub strategy: FanoutSyncStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_success: Option<u64>,
    pub on_branch_failure: FanoutBranchFailurePolicy,
    pub threshold_gates: Vec<FanoutThresholdGate>,
    pub conflict_gates: Vec<FanoutConflictGate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphGuard {
    pub step: String,
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equals: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<JsonValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphPolicy {
    pub guards: Vec<GraphGuard>,
}

/// Per-step conditional selection. When present and the condition does not hold
/// (or the field is unresolved), the step is skipped and the graph continues;
/// sibling steps with complementary conditions form a branch. Unlike a guard,
/// a `when` never blocks the run, it selects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphWhen {
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equals: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_equals: Option<JsonValue>,
}

/// Declarative request to MINT (compute) an act's child authority term from the
/// graph charter, off the model path, rather than receive a pre-built term. The
/// requested child scope is read from the trusted input this names; the mint
/// fails closed when it exceeds the charter. Only coherent when the graph (or
/// runner) declares `charter_from`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MintAuthorityDirective {
    pub requested_scope_from: String,
}

/// Parser-owned meaning of an inline graph `run:` target. The wire form stays
/// the author-facing source object, while every consumer receives either the
/// graph control step or a fully validated skill source.
#[derive(Clone, Debug, PartialEq)]
pub enum GraphRunTarget {
    Approval,
    Source(Box<SkillSource>),
}

impl GraphRunTarget {
    pub(crate) fn validate(raw: JsonObject, field: &str) -> Result<Self, ValidationError> {
        if raw.get("type").and_then(JsonValue::as_str) == Some("approval") {
            if raw.len() == 1 {
                return Ok(Self::Approval);
            }
            let fields = raw
                .keys()
                .filter(|key| key.as_str() != "type")
                .cloned()
                .collect::<Vec<_>>();
            return Err(ValidationError::InvalidField {
                field: field.to_owned(),
                message: format!(
                    "{field} approval control accepts only type; unsupported fields: {}",
                    fields.join(", ")
                ),
            });
        }
        let source =
            validate_inline_graph_source(&raw).map_err(|source| ValidationError::InvalidField {
                field: field.to_owned(),
                message: format!("{field} is not a valid inline source: {source}"),
            })?;
        if !matches!(
            source.source_type,
            SourceKind::AgentStep | SourceKind::CliTool | SourceKind::JavaScript
        ) {
            return Err(ValidationError::InvalidField {
                field: field.to_owned(),
                message: format!(
                    "{field}.type '{}' is not an inline graph source; use a skill or tool target for supervised adapters",
                    source.source_type
                ),
            });
        }
        Ok(Self::Source(Box::new(source)))
    }

    #[must_use]
    pub fn source(&self) -> Option<&SkillSource> {
        match self {
            Self::Approval => None,
            Self::Source(source) => Some(source.as_ref()),
        }
    }

    #[must_use]
    pub fn source_type(&self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Source(source) => source.source_type.as_str(),
        }
    }
}

impl Serialize for GraphRunTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Approval => {
                JsonObject::from([("type".to_owned(), JsonValue::String("approval".to_owned()))])
                    .serialize(serializer)
            }
            Self::Source(source) => source.raw.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for GraphRunTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = JsonObject::deserialize(deserializer)?;
        Self::validate(raw, "run").map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphStep {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<GraphRunTarget>,
    /// A connector operation, admitted and executed by the kernel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationStepTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<SkillArtifactContract>,
    /// Complete output declarations owned by this graph step. Preserve these
    /// in typed IR so inspection, packet generation, and registry packaging do
    /// not have to reparse the raw manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    pub inputs: JsonObject,
    pub context: BTreeMap<String, String>,
    pub context_edges: Vec<GraphContextEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_skills: Vec<String>,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<GraphRetryPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fanout_group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<GraphWhen>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// What this use accepts when an operation's outcome is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_uncertain: Option<UncertainOutcome>,
    /// Whether this use tolerates a visible duplicate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<DuplicatePolicy>,
}

/// An operation step's target: a key of the skill's `connectors:` block and,
/// unless the step binds it as its `operation` input, one operation on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationStepTarget {
    pub requirement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationName>,
    /// The requirement as the enclosing skill declares it, bound when the
    /// skill is parsed. A graph outside a skill has none, and its operation
    /// steps are refused at admission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<crate::ConnectorRequirement>,
}

/// What a use accepts when an operation's outcome is unknown. The kernel
/// refuses a tolerance the operation's guarantees cannot support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum UncertainOutcome {
    /// Hold for reconciliation; never re-send blind. The default.
    Hold,
    /// Settle the outcome through the operation's reconcile read.
    Reconcile,
    /// Give up after this long and let the caller supersede the work.
    ExpireAfter { seconds: u64 },
}

impl UncertainOutcome {
    /// `hold`, `reconcile`, or `expire_after <n><s|m|h|d>`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "hold" => Some(Self::Hold),
            "reconcile" => Some(Self::Reconcile),
            other => {
                let duration = other.strip_prefix("expire_after")?.trim();
                let (count, unit) = duration.split_at(duration.len().checked_sub(1)?);
                let count = count.parse::<u64>().ok().filter(|count| *count > 0)?;
                let unit = match unit {
                    "s" => 1,
                    "m" => 60,
                    "h" => 3_600,
                    "d" => 86_400,
                    _ => return None,
                };
                count
                    .checked_mul(unit)
                    .map(|seconds| Self::ExpireAfter { seconds })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicatePolicy {
    /// No duplicate may become visible. The default.
    Forbidden,
    /// A duplicate is acceptable to this use.
    Visible,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionGraph {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Steps whose complete declared output contracts form the graph's public
    /// result. Multiple entries support mutually exclusive terminal branches
    /// and workflows that intentionally return both an act result and its
    /// durable readback. Diagnostic/base fields are never exported.
    pub result_from: Vec<String>,
    /// The input key carrying the parent charter authority term that steps with a
    /// `mint_authority` directive attenuate from. Declared once at the graph (or
    /// runner) level, replacing per-skill re-threading of the parent authority. A
    /// step's `mint_authority` is only coherent when this is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charter_from: Option<String>,
    pub steps: Vec<GraphStep>,
    pub fanout_groups: BTreeMap<String, FanoutGroupPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<GraphPolicy>,
    pub raw: RawGraphIr,
}
