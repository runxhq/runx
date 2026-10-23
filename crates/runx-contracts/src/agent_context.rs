//! Agent-context envelope (`runx.ai/spec/agent-context-envelope`): the bulky
//! per-act execution context referenced from a receipt act's `context_ref`
//! (instructions, inputs, current/historical artifact context, provenance, and
//! the resolved skill profiles).
//!
//! Identity is the legacy bare `runx.ai/spec` `$id` (no `x-runx-schema`).
use serde::{Deserialize, Serialize};

use crate::output::OutputField;
use crate::schema::{NonEmptyString, RunxSchema};
use crate::{AgentExecutionRequirements, JsonObject, JsonValue};
use std::collections::BTreeMap;

/// The artifact context entry version. Committed as `const: "1"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ContextEntryVersion {
    #[serde(rename = "1")]
    V1,
}

/// The producer of a context artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextArtifactProducer {
    pub skill: NonEmptyString,
    pub runner: NonEmptyString,
}

/// Metadata for a context artifact. `step_id`, `parent_artifact_id`, and
/// `receipt_id` are required-but-nullable (present on the wire, possibly null).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextArtifactMeta {
    pub artifact_id: NonEmptyString,
    pub run_id: NonEmptyString,
    pub step_id: Option<NonEmptyString>,
    pub producer: ContextArtifactProducer,
    pub created_at: NonEmptyString,
    pub digest: NonEmptyString,
    pub size_bytes: u64,
    pub parent_artifact_id: Option<NonEmptyString>,
    pub receipt_id: Option<NonEmptyString>,
    pub redacted: bool,
}

/// A single artifact context entry. `type` is required-but-nullable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextEntry {
    #[serde(rename = "type")]
    pub entry_type: Option<NonEmptyString>,
    /// What the entry is, in a line, as its producer declares it: what a
    /// model reads in an index to choose whether to read the entry's data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<NonEmptyString>,
    pub version: ContextEntryVersion,
    pub scope: ContextScope,
    pub data: JsonObject,
    pub meta: ContextArtifactMeta,
}

/// Declares how context may be interpreted and where it may be shared. These
/// labels never grant execution authority or manufacture a human decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextScope {
    pub purpose: ContextPurpose,
    pub trust: ContextTrust,
    pub sharing: ContextSharing,
    pub goal_id: Option<NonEmptyString>,
    pub source_uri: Option<NonEmptyString>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextPurpose {
    Procedure,
    Mandate,
    ConfirmedMemory,
    Observation,
    WorkContinuity,
    RetrievedHistory,
    Hypothesis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextTrust {
    OperatorConfirmed,
    SkillInstructions,
    UntrustedEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextSharing {
    LocalOnly,
    ConfiguredModel,
}

/// One input/output provenance edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceEntry {
    pub input: NonEmptyString,
    pub output: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
}

/// A resolved profile sourced from a workspace file (memory, conventions,
/// voice).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    pub sharing: ContextSharing,
    pub root_path: NonEmptyString,
    pub path: NonEmptyString,
    pub digest: NonEmptyString,
    pub content: String,
}

/// The optional conventions and project instructions context block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentContextProfiles {
    /// The workspace's `MEMORY.md`, in envelopes recorded before hosts
    /// carried memory themselves. runx writes none: memory a host supplies
    /// travels as admitted context entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<ProfileFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conventions: Option<ProfileFile>,
    /// The project's instructions: each `AGENTS.md` and `CLAUDE.md` from the
    /// repository root that contains the workspace down to the workspace, in
    /// that order, each `path` relative to the repository root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<ProfileFile>,
}

/// Where the skill executes from on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLocation {
    pub skill_directory: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_roots: Option<Vec<NonEmptyString>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.agent_context_envelope.v1")]
pub struct AgentContextEnvelope {
    pub run_id: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<NonEmptyString>,
    pub skill: NonEmptyString,
    /// The act its run's invoker stated: why the run exists, as the run's
    /// steps read `$act`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<crate::CallAct>,
    /// Digest of the exact owning `SKILL.md` bytes carried in `instructions`.
    pub instructions_digest: NonEmptyString,
    pub instructions: NonEmptyString,
    pub inputs: JsonObject,
    /// Exact non-secret execution requirements selected from the owning
    /// manifest, plus environment-name availability. Secret values never enter
    /// the agent context.
    pub requirements: AgentExecutionRequirements,
    pub current_context: Vec<ContextEntry>,
    pub historical_context: Vec<ContextEntry>,
    pub provenance: Vec<ProvenanceEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<AgentContextProfiles>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_profile: Option<ProfileFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_location: Option<ExecutionLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<BTreeMap<String, OutputField>>,
    /// Exact root JSON Schema for the final result, including any declared
    /// packet contract. `output` remains the concise field index used by hosts;
    /// this schema is the validator offered to managed agents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonValue>,
    pub trust_boundary: NonEmptyString,
}
