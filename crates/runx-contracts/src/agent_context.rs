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

/// A context entry's schema, `runx.context_entry.v1`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ContextEntrySchema {
    #[serde(rename = "runx.context_entry.v1")]
    V1,
}

/// The skill and method that produced a context entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextProducer {
    pub skill: NonEmptyString,
    pub method: NonEmptyString,
}

/// Where a context entry came from: the artifact that holds it, the run and
/// step that produced it, and its bytes' digest and size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextOrigin {
    pub artifact_id: NonEmptyString,
    pub run_id: NonEmptyString,
    pub step_id: Option<NonEmptyString>,
    pub producer: ContextProducer,
    pub created_at: NonEmptyString,
    pub digest: NonEmptyString,
    pub size_bytes: u64,
}

/// One entry of context a turn is given (`runx.context_entry.v1`): memory, a
/// skill's instructions, or any other context, with the labels that say how
/// it may be read and where it may go.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextEntry {
    pub schema: ContextEntrySchema,
    /// What the entry is.
    #[serde(rename = "type")]
    pub r#type: NonEmptyString,
    /// What the entry is, in a line, as its producer declares it: what a
    /// model reads in an index to choose whether to read the entry's data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<NonEmptyString>,
    pub data: JsonObject,
    pub labels: ContextLabels,
    /// Where its content came from (D35).
    pub sources: Vec<NonEmptyString>,
    pub origin: ContextOrigin,
}

/// How context may be read and where it may be shared. These labels never
/// grant execution authority or manufacture a human decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextLabels {
    pub purpose: ContextPurpose,
    pub trust: ContextTrust,
    pub sharing: ContextSharing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextPurpose {
    Procedure,
    Mandate,
    ConfirmedMemory,
    Observation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextTrust {
    PersonConfirmed,
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
    /// steps read `$run.act`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<crate::CallAct>,
    /// Digest of the exact bytes carried in `instructions`.
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
