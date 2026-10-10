use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use runx_contracts::CredentialDeliveryObservation;
use runx_contracts::{ContextEntry, ExecutionRequirements, JsonObject, JsonValue, ProvenanceEntry};
use runx_parser::{AgentTask, ProcessTool, SkillArtifactContract, WorkerTool};
use serde::{Deserialize, Serialize};

use crate::RuntimeError;
use crate::credentials::CredentialDelivery;

/// Metadata key under which a skill's non-secret credential-delivery
/// observations are recorded on [`InvocationOutput::metadata`].
pub const CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA: &str = "credential_delivery_observations";
/// Structured, already-verified contract evidence that the receipt sealer binds
/// into signed criteria. Producers must populate this only after native
/// verification succeeds.
pub const CONTRACT_VERIFICATION_METADATA: &str = "contract_verification";
/// Runtime-enforced ceilings that shaped this invocation. The receipt sealer
/// copies only this structured, runtime-owned metadata key.
pub use runx_contracts::EXECUTION_LIMITS_METADATA;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvocationStatus {
    Success,
    Failure,
}

/// What an invocation runs: a deterministic worker module, a trusted host
/// process, or an agent act.
#[derive(Clone, Debug, PartialEq)]
pub enum Executable {
    Worker(WorkerTool),
    Process(ProcessTool),
    Agent(AgentTask),
}

impl Executable {
    /// The executable's kind, as receipts and diagnostics name it.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Worker(_) => "worker",
            Self::Process(_) => "process",
            Self::Agent(_) => "agent",
        }
    }

    #[must_use]
    pub const fn worker(&self) -> Option<&WorkerTool> {
        match self {
            Self::Worker(worker) => Some(worker),
            _ => None,
        }
    }

    #[must_use]
    pub const fn process(&self) -> Option<&ProcessTool> {
        match self {
            Self::Process(process) => Some(process),
            _ => None,
        }
    }

    #[must_use]
    pub const fn agent(&self) -> Option<&AgentTask> {
        match self {
            Self::Agent(agent) => Some(agent),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillInvocation {
    pub historical_context: Vec<ContextEntry>,
    pub skill_name: String,
    /// Graph step identity when the invocation is owned by a graph act.
    pub step_id: Option<String>,
    pub executable: Executable,
    /// Complete output declarations the invocation's result must satisfy.
    pub outputs: Option<JsonObject>,
    /// Exact non-secret manifest requirements for this executable act.
    pub requirements: ExecutionRequirements,
    /// Parser-validated artifact contract for this invocation.
    pub artifacts: Option<SkillArtifactContract>,
    pub inputs: JsonObject,
    pub resolved_inputs: JsonObject,
    pub current_context: Vec<ContextEntry>,
    /// Exact graph context edges that supplied invocation inputs.
    pub provenance: Vec<ProvenanceEntry>,
    pub skill_directory: PathBuf,
    pub env: BTreeMap<String, String>,
    pub credential_delivery: CredentialDelivery,
}

#[doc(hidden)]
#[derive(Clone, Default, PartialEq)]
pub struct EphemeralValue(Option<JsonValue>);

impl EphemeralValue {
    #[must_use]
    pub(crate) fn from_value(value: JsonValue) -> Self {
        if matches!(&value, JsonValue::Object(object) if object.is_empty()) {
            Self::default()
        } else {
            Self(Some(value))
        }
    }

    #[must_use]
    pub(crate) fn as_value(&self) -> Option<&JsonValue> {
        self.0.as_ref()
    }

    #[must_use]
    pub(crate) fn merged_with(&self, durable: &JsonValue) -> JsonValue {
        let mut merged = durable.clone();
        if let Some(ephemeral) = &self.0 {
            merge_json_overlay(&mut merged, ephemeral);
        }
        merged
    }
}

impl fmt::Debug for EphemeralValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(if self.0.is_some() {
            "EphemeralValue([redacted])"
        } else {
            "EphemeralValue(None)"
        })
    }
}

fn merge_json_overlay(target: &mut JsonValue, overlay: &JsonValue) {
    match (target, overlay) {
        (JsonValue::Object(target), JsonValue::Object(overlay)) => {
            for (key, value) in overlay {
                match target.get_mut(key) {
                    Some(existing) => merge_json_overlay(existing, value),
                    None => {
                        target.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (target, overlay) => *target = overlay.clone(),
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct InvocationOutput {
    pub status: InvocationStatus,
    /// The typed value produced by the invocation. Structured runtimes keep
    /// their native JSON tree; real process adapters parse JSON once or retain
    /// non-JSON stdout as a string.
    pub value: JsonValue,
    pub diagnostics: InvocationDiagnostics,
    pub metadata: JsonObject,
    /// Caller-only output that may cross the immediate process boundary but is
    /// never serialized, checkpointed, sealed, or admitted into graph context.
    #[serde(skip, default)]
    pub(crate) ephemeral: EphemeralValue,
}

impl fmt::Debug for InvocationOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvocationOutput")
            .field("status", &self.status)
            .field("value", &self.value)
            .field("diagnostics", &self.diagnostics)
            .field("metadata", &self.metadata)
            .field("ephemeral", &self.ephemeral)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvocationDiagnostics {
    Runtime {
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
    },
    Process {
        duration_ms: u64,
        exit_code: Option<i32>,
        stderr: String,
    },
}

impl InvocationOutput {
    #[must_use]
    pub fn runtime_success(value: JsonValue, duration_ms: u64, metadata: JsonObject) -> Self {
        Self {
            status: InvocationStatus::Success,
            value,
            diagnostics: InvocationDiagnostics::Runtime {
                duration_ms,
                failure: None,
            },
            metadata,
            ephemeral: EphemeralValue::default(),
        }
    }

    #[must_use]
    pub fn runtime_failure(
        value: JsonValue,
        message: impl Into<String>,
        duration_ms: u64,
        metadata: JsonObject,
    ) -> Self {
        Self {
            status: InvocationStatus::Failure,
            value,
            diagnostics: InvocationDiagnostics::Runtime {
                duration_ms,
                failure: Some(message.into()),
            },
            metadata,
            ephemeral: EphemeralValue::default(),
        }
    }

    #[must_use]
    pub fn process(
        status: InvocationStatus,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        duration_ms: u64,
        metadata: JsonObject,
    ) -> Self {
        let value = serde_json::from_str(&stdout).unwrap_or(JsonValue::String(stdout));
        Self::process_value(status, value, stderr, exit_code, duration_ms, metadata)
    }

    #[must_use]
    pub fn process_value(
        status: InvocationStatus,
        value: JsonValue,
        stderr: String,
        exit_code: Option<i32>,
        duration_ms: u64,
        metadata: JsonObject,
    ) -> Self {
        Self {
            status,
            value,
            diagnostics: InvocationDiagnostics::Process {
                duration_ms,
                exit_code,
                stderr,
            },
            metadata,
            ephemeral: EphemeralValue::default(),
        }
    }

    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.status == InvocationStatus::Success
    }

    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        match &self.diagnostics {
            InvocationDiagnostics::Runtime { duration_ms, .. }
            | InvocationDiagnostics::Process { duration_ms, .. } => *duration_ms,
        }
    }

    pub fn set_duration_ms(&mut self, duration_ms: u64) {
        match &mut self.diagnostics {
            InvocationDiagnostics::Runtime {
                duration_ms: current,
                ..
            }
            | InvocationDiagnostics::Process {
                duration_ms: current,
                ..
            } => *current = duration_ms,
        }
    }

    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        match &self.diagnostics {
            InvocationDiagnostics::Runtime { .. } => None,
            InvocationDiagnostics::Process { exit_code, .. } => *exit_code,
        }
    }

    #[must_use]
    pub fn process_stderr(&self) -> Option<&str> {
        match &self.diagnostics {
            InvocationDiagnostics::Runtime { .. } => None,
            InvocationDiagnostics::Process { stderr, .. } => Some(stderr),
        }
    }

    #[must_use]
    pub fn failure_message(&self) -> Option<String> {
        if self.succeeded() {
            return None;
        }
        match &self.diagnostics {
            InvocationDiagnostics::Runtime { failure, .. } => failure.clone(),
            InvocationDiagnostics::Process { stderr, .. } if !stderr.trim().is_empty() => {
                Some(stderr.clone())
            }
            InvocationDiagnostics::Process { .. } => match &self.value {
                JsonValue::String(value) if !value.trim().is_empty() => Some(value.clone()),
                JsonValue::Null => Some("process failed without diagnostic output".to_owned()),
                value => serde_json::to_string(value).ok(),
            },
        }
    }

    #[must_use]
    pub fn rendered_value(&self) -> String {
        match &self.value {
            JsonValue::String(value) => value.clone(),
            value => serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned()),
        }
    }

    pub fn reject(&mut self, message: impl Into<String>) {
        self.status = InvocationStatus::Failure;
        self.value = JsonValue::Null;
        self.ephemeral = EphemeralValue::default();
        self.diagnostics = InvocationDiagnostics::Runtime {
            duration_ms: self.duration_ms(),
            failure: Some(message.into()),
        };
    }

    pub(crate) fn set_ephemeral(&mut self, value: JsonValue) {
        self.ephemeral = EphemeralValue::from_value(value);
    }

    /// Append one non-secret credential observation for receipt sealing. This
    /// records the runtime boundary observation as supplied; it does not imply
    /// that credential material entered the invoked subprocess.
    pub(crate) fn record_credential_observation(
        &mut self,
        observation: &CredentialDeliveryObservation,
    ) -> Result<(), RuntimeError> {
        let value: JsonValue = serde_json::to_value(observation)
            .and_then(serde_json::from_value)
            .map_err(|source| {
                RuntimeError::json("serializing credential delivery observation", source)
            })?;
        let observation_id = value
            .as_object()
            .and_then(|object| object.get("observation_id"))
            .and_then(JsonValue::as_str)
            .ok_or_else(|| RuntimeError::ReceiptInvalid {
                message: "credential delivery observation omitted observation_id".to_owned(),
            })?;
        match self
            .metadata
            .get_mut(CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA)
        {
            Some(JsonValue::Array(observations)) => {
                if let Some(existing) = observations.iter().find(|existing| {
                    existing
                        .as_object()
                        .and_then(|object| object.get("observation_id"))
                        .and_then(JsonValue::as_str)
                        == Some(observation_id)
                }) {
                    if existing == &value {
                        return Ok(());
                    }
                    return Err(RuntimeError::ReceiptInvalid {
                        message: format!(
                            "credential delivery observation {observation_id:?} was recorded with conflicting evidence"
                        ),
                    });
                }
                observations.push(value);
            }
            Some(_) => {
                return Err(RuntimeError::ReceiptInvalid {
                    message: format!(
                        "{CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA} metadata must be an array"
                    ),
                });
            }
            None => {
                self.metadata.insert(
                    CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA.to_owned(),
                    JsonValue::Array(vec![value]),
                );
            }
        }
        Ok(())
    }
}

pub trait SkillAdapter {
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError>;
}

pub(crate) struct BorrowedSkillAdapter<'a, A>
where
    A: SkillAdapter + ?Sized,
{
    adapter: &'a A,
}

impl<'a, A> BorrowedSkillAdapter<'a, A>
where
    A: SkillAdapter + ?Sized,
{
    pub(crate) fn new(adapter: &'a A) -> Self {
        Self { adapter }
    }
}

impl<A> SkillAdapter for BorrowedSkillAdapter<'_, A>
where
    A: SkillAdapter + ?Sized,
{
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        self.adapter.invoke(request)
    }
}

impl<A> SkillAdapter for Box<A>
where
    A: SkillAdapter + ?Sized,
{
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        self.as_ref().invoke(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ephemeral_output_is_caller_only_and_redacted_from_durable_shapes()
    -> Result<(), Box<dyn std::error::Error>> {
        const SENTINEL: &str = "auc_secret_capability";
        let mut output = InvocationOutput::runtime_success(
            JsonValue::Object(JsonObject::from([(
                "resource".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "status".to_owned(),
                    JsonValue::String("ready".to_owned()),
                )])),
            )])),
            0,
            JsonObject::new(),
        );
        output.set_ephemeral(JsonValue::Object(JsonObject::from([(
            "resource".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "access".to_owned(),
                JsonValue::String(SENTINEL.to_owned()),
            )])),
        )])));

        let serialized = serde_json::to_string(&output)?;
        assert!(!serialized.contains(SENTINEL));
        assert!(!format!("{output:?}").contains(SENTINEL));
        assert!(
            serde_json::to_string(&output.ephemeral.merged_with(&output.value))?.contains(SENTINEL)
        );
        Ok(())
    }

    #[test]
    fn credential_observation_is_idempotent_and_rejects_conflicting_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let delivery = CredentialDelivery::process_env(
            "example",
            "api_key",
            "EXAMPLE_TOKEN",
            "local:example:test",
            "secret",
        )?;
        let observation = delivery
            .public_observation()
            .cloned()
            .ok_or("local delivery omitted its observation")?;
        let mut output = InvocationOutput::runtime_success(JsonValue::Null, 0, JsonObject::new());

        output.record_credential_observation(&observation)?;
        output.record_credential_observation(&observation)?;
        assert_eq!(
            output
                .metadata
                .get(CREDENTIAL_DELIVERY_OBSERVATIONS_METADATA)
                .and_then(JsonValue::as_array)
                .map(Vec::len),
            Some(1)
        );

        let mut conflicting = observation;
        conflicting.provider = "different".into();
        assert!(output.record_credential_observation(&conflicting).is_err());
        Ok(())
    }
}
