use runx_contracts::{AuthorityVerb, JsonObject, JsonValue};
use thiserror::Error;

use crate::credentials::CredentialDeliveryError;

#[derive(Debug, Error)]
#[error(
    "context edge into step '{to_step}' input '{input}' from step '{from_step}' could not resolve path '{output_path}': segment '{missing_segment}' is absent; available keys there: [{}]",
    .available_keys.join(", ")
)]
pub struct ContextEdgeUnresolvedError {
    to_step: String,
    input: String,
    from_step: String,
    output_path: String,
    missing_segment: String,
    available_keys: Vec<String>,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime I/O failed while {context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("graph parse failed: {0}")]
    ParseGraph(#[from] runx_parser::ParseError),
    #[error("graph validation failed: {0}")]
    ValidateGraph(#[from] runx_parser::ValidationError),
    #[error("skill package validation failed: {0}")]
    SkillPackage(#[from] runx_parser::SkillPackageError),
    #[error("workspace environment failed: {0}")]
    WorkspaceEnvironment(#[from] crate::WorkspaceEnvError),
    #[error("JSON serialization failed while {context}: {source}")]
    Json {
        context: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("graph step '{step_id}' is missing")]
    StepMissing { step_id: String },
    #[error("graph step '{step_id}' has no skill target")]
    StepMissingSkill { step_id: String },
    #[error("graph step '{step_id}' has invalid run configuration: {reason}")]
    InvalidRunStep { step_id: String, reason: String },
    #[error("graph step '{step_id}' uses unsupported run type '{run_type}'")]
    UnsupportedRunStep { step_id: String, run_type: String },
    #[error("graph step '{step_id}' is blocked: {reason}")]
    GraphBlocked { step_id: String, reason: String },
    #[error("graph step '{step_id}' is waiting for host resolution: {reason}")]
    ResolutionPending {
        step_id: String,
        reason: String,
        /// The request the innermost step asked its host.
        request: Box<runx_contracts::ResolutionRequest>,
        /// The child run that holds the request, when the step is a nested
        /// skill; `None` when the step asked its host itself.
        child: Option<String>,
    },
    /// A step sealed a governed closure other than `closed`: an agent act
    /// that declined, deferred, blocked, or was superseded. The graph stops
    /// there with that disposition; it is an outcome, not a fault.
    #[error("graph step '{step_id}' closed with {}: {summary}", disposition.label())]
    StepClosed {
        step_id: String,
        disposition: runx_contracts::ClosureDisposition,
        /// The step receipt's own seal reason, which the graph seal keeps.
        reason: Option<runx_contracts::SealReason>,
        summary: String,
    },
    #[error(transparent)]
    ContextEdgeUnresolved(Box<ContextEdgeUnresolvedError>),
    #[error("authority {verb:?} denied graph step '{step_id}': {reason}")]
    AuthorityDenied {
        verb: AuthorityVerb,
        step_id: String,
        reason: String,
    },
    #[error("graph step '{step_id}' failed planning: {reason}")]
    GraphPlanningFailed { step_id: String, reason: String },
    #[cfg(feature = "agent")]
    #[error("managed agent resolution '{request_id}' failed in graph step '{step_id}': {source}")]
    ManagedAgentResolution {
        step_id: String,
        request_id: String,
        #[source]
        source: Box<crate::adapters::agent::ManagedAgentError>,
    },
    #[error(
        "provider mutation was applied but independent readback is unavailable at graph step '{step_id}': {reason}"
    )]
    ProviderReadbackPending { step_id: String, reason: String },
    /// The driver was asked to stop, so `step_id` was not admitted; the run
    /// stays where it is, and its next driver continues it.
    #[error("run {run_id} stopped before {step_id}; runx resume continues it")]
    Stopped { run_id: String, step_id: String },
    #[error(transparent)]
    RunRecord(#[from] crate::execution::run_record::RunRecordError),
    #[error("unsupported adapter '{adapter_type}'")]
    UnsupportedAdapter { adapter_type: String },
    #[error("runtime engine failed while {context}: {source}")]
    EngineFailure {
        context: &'static str,
        #[source]
        source: Box<RuntimeError>,
    },
    #[error("runtime engine invariant failed while {context}: {message}")]
    EngineInvariant {
        context: &'static str,
        message: String,
    },
    #[error("this adapter does not run a {kind} executable")]
    UnsupportedExecutable { kind: String },
    #[error("runner selection '{runner}' is not supported by the native runtime yet")]
    UnsupportedRunnerSelection { runner: String },
    #[error("process invocation is invalid: {message}")]
    InvalidProcessInvocation { message: String },
    #[error(
        "required environment variable(s) are unavailable: {}",
        .names.join(", ")
    )]
    MissingEnvironment { names: Vec<String> },
    #[error(
        "environment variable(s) {} hold a connector credential, which a skill's declared environment never carries",
        .names.join(", ")
    )]
    CredentialEnvironment { names: Vec<String> },
    #[error("deterministic JavaScript worker failed: {message}")]
    JavaScriptWorker { message: String },
    #[error("credential delivery failed: {0}")]
    CredentialDelivery(#[from] CredentialDeliveryError),
    #[error("effect state failed while {context}: {message}")]
    EffectState { context: String, message: String },
    #[error(
        "provider effect outcome is unknown for plan {plan_digest} with idempotency key {idempotency_key}: {reason}"
    )]
    ProviderEffectUnknown {
        plan_digest: String,
        idempotency_key: String,
        reason: String,
    },
    /// The provider answered and applied nothing: a definite, non-retryable
    /// refusal of the request, unlike an unknown outcome.
    #[error(
        "provider rejected the effect for plan {plan_digest} with idempotency key {idempotency_key} ({provider_code}, HTTP {http_status}): {reason}"
    )]
    ProviderEffectRejected {
        plan_digest: String,
        idempotency_key: String,
        provider_code: String,
        http_status: u16,
        reason: String,
    },
    #[error("skill '{skill_name}' failed: {message}")]
    SkillFailed { skill_name: String, message: String },
    #[cfg(feature = "agent")]
    #[error("model provider failed: {detail}")]
    ModelProvider { detail: String },
    #[error("continuation rejected: {reason}")]
    ContinuationRejected { reason: String },
    #[error("{owner} input contract failed at '{path}': {message}")]
    InputContract {
        step_id: Option<String>,
        owner: &'static str,
        input: String,
        path: String,
        message: String,
        accepted_schema: Box<runx_contracts::JsonValue>,
    },
    #[error("receipt validation failed: {message}")]
    ReceiptInvalid { message: String },
}

impl RuntimeError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub(crate) fn json(context: impl Into<String>, source: serde_json::Error) -> Self {
        Self::Json {
            context: context.into(),
            source,
        }
    }

    pub(crate) fn effect_state(context: impl Into<String>, source: impl std::fmt::Display) -> Self {
        Self::EffectState {
            context: context.into(),
            message: source.to_string(),
        }
    }

    pub(crate) fn engine(context: &'static str, source: RuntimeError) -> Self {
        // Never absorb a governed flow outcome: the graph engine and refusal
        // sealing match on these exact variants, so wrapping one would turn a
        // provable denial into an anonymous engine fault.
        match source {
            source @ (Self::AuthorityDenied { .. }
            | Self::GraphBlocked { .. }
            | Self::ResolutionPending { .. }
            | Self::ProviderReadbackPending { .. }) => source,
            source => Self::EngineFailure {
                context,
                source: Box::new(source),
            },
        }
    }

    /// Classify the execution boundary exhaustively. Adding a new runtime
    /// error must make an explicit decision here before it can be sealed as a
    /// skill outcome or escape as an engine fault.
    pub(crate) fn is_fatal_step_fault(&self) -> bool {
        match self {
            Self::StepMissing { .. }
            | Self::GraphPlanningFailed { .. }
            | Self::RunRecord(_)
            | Self::EngineFailure { .. }
            | Self::EngineInvariant { .. }
            | Self::EffectState { .. }
            | Self::ProviderEffectUnknown { .. }
            | Self::ProviderReadbackPending { .. }
            | Self::ReceiptInvalid { .. }
            | Self::ContinuationRejected { .. }
            | Self::Stopped { .. } => true,
            Self::Io { .. }
            | Self::ParseGraph(_)
            | Self::ValidateGraph(_)
            | Self::SkillPackage(_)
            | Self::WorkspaceEnvironment(_)
            | Self::Json { .. }
            | Self::StepMissingSkill { .. }
            | Self::InvalidRunStep { .. }
            | Self::UnsupportedRunStep { .. }
            | Self::GraphBlocked { .. }
            | Self::ResolutionPending { .. }
            | Self::StepClosed { .. }
            | Self::ContextEdgeUnresolved(_)
            | Self::AuthorityDenied { .. }
            | Self::UnsupportedAdapter { .. }
            | Self::UnsupportedExecutable { .. }
            | Self::UnsupportedRunnerSelection { .. }
            | Self::InvalidProcessInvocation { .. }
            | Self::MissingEnvironment { .. }
            | Self::CredentialEnvironment { .. }
            | Self::JavaScriptWorker { .. }
            | Self::CredentialDelivery(_)
            | Self::ProviderEffectRejected { .. }
            | Self::SkillFailed { .. } => false,
            Self::InputContract { .. } => false,
            #[cfg(feature = "agent")]
            Self::ManagedAgentResolution { .. } | Self::ModelProvider { .. } => false,
        }
    }

    pub(crate) fn context_edge_unresolved(
        to_step: impl Into<String>,
        input: impl Into<String>,
        from_step: impl Into<String>,
        output_path: impl Into<String>,
        missing_segment: impl Into<String>,
        available_keys: Vec<String>,
    ) -> Self {
        Self::ContextEdgeUnresolved(Box::new(ContextEdgeUnresolvedError {
            to_step: to_step.into(),
            input: input.into(),
            from_step: from_step.into(),
            output_path: output_path.into(),
            missing_segment: missing_segment.into(),
            available_keys,
        }))
    }

    /// Safe structured failure returned by both direct and graph-backed skill
    /// fronts. The same projection is bound into the failed step receipt and
    /// returned to the caller, so diagnostics never depend on scraping prose.
    pub(crate) fn public_failure_projection(&self) -> JsonObject {
        let mut projection = JsonObject::from([
            (
                "code".to_owned(),
                JsonValue::String("runtime_error".to_owned()),
            ),
            ("message".to_owned(), JsonValue::String(self.to_string())),
        ]);
        match self {
            Self::InputContract {
                step_id,
                owner,
                input,
                path,
                accepted_schema,
                ..
            } => {
                projection.insert(
                    "code".to_owned(),
                    JsonValue::String("input_contract_invalid".to_owned()),
                );
                if let Some(step_id) = step_id {
                    projection.insert("step_id".to_owned(), JsonValue::String(step_id.clone()));
                }
                projection.insert("owner".to_owned(), JsonValue::String((*owner).to_owned()));
                projection.insert("input".to_owned(), JsonValue::String(input.clone()));
                projection.insert("path".to_owned(), JsonValue::String(path.clone()));
                projection.insert(
                    "accepted_schema".to_owned(),
                    accepted_schema.as_ref().clone(),
                );
            }
            #[cfg(feature = "agent")]
            Self::ManagedAgentResolution { source, .. } => {
                projection = source.public_failure_projection();
            }
            Self::ProviderEffectRejected {
                provider_code,
                http_status,
                reason,
                ..
            } => {
                projection.insert(
                    "code".to_owned(),
                    JsonValue::String("provider_rejected".to_owned()),
                );
                projection.insert(
                    "provider_code".to_owned(),
                    JsonValue::String(provider_code.clone()),
                );
                projection.insert(
                    "http_status".to_owned(),
                    JsonValue::Number(runx_contracts::JsonNumber::U64(u64::from(*http_status))),
                );
                projection.insert("retryable".to_owned(), JsonValue::Bool(false));
                projection.insert("reason".to_owned(), JsonValue::String(reason.clone()));
            }
            Self::AuthorityDenied {
                verb,
                step_id,
                reason,
            } => {
                projection.insert(
                    "code".to_owned(),
                    JsonValue::String("authority_denied".to_owned()),
                );
                projection.insert("step_id".to_owned(), JsonValue::String(step_id.clone()));
                if let Ok(verb) = JsonValue::from_serialize(verb) {
                    projection.insert("verb".to_owned(), verb);
                }
                projection.insert("reason".to_owned(), JsonValue::String(reason.clone()));
            }
            Self::ProviderReadbackPending { step_id, reason } => {
                projection.insert(
                    "code".to_owned(),
                    JsonValue::String("provider_readback_pending".to_owned()),
                );
                projection.insert("step_id".to_owned(), JsonValue::String(step_id.clone()));
                projection.insert(
                    "mutation_status".to_owned(),
                    JsonValue::String("applied_unconfirmed".to_owned()),
                );
                projection.insert("retryable".to_owned(), JsonValue::Bool(false));
                projection.insert("reason".to_owned(), JsonValue::String(reason.clone()));
            }
            _ => {}
        }
        projection
    }

    /// Authoritative graph-step identity attached by the dispatch chokepoint.
    /// Terminal sealing consumes this instead of guessing identity from error
    /// prose or nested capability names.
    pub(crate) fn graph_step_id(&self) -> Option<&str> {
        match self {
            Self::StepMissing { step_id }
            | Self::StepMissingSkill { step_id }
            | Self::InvalidRunStep { step_id, .. }
            | Self::UnsupportedRunStep { step_id, .. }
            | Self::GraphBlocked { step_id, .. }
            | Self::ResolutionPending { step_id, .. }
            | Self::StepClosed { step_id, .. }
            | Self::AuthorityDenied { step_id, .. }
            | Self::GraphPlanningFailed { step_id, .. }
            | Self::ProviderReadbackPending { step_id, .. } => Some(step_id),
            Self::InputContract { step_id, .. } => step_id.as_deref(),
            #[cfg(feature = "agent")]
            Self::ManagedAgentResolution { step_id, .. } => Some(step_id),
            _ => None,
        }
    }

    #[cfg(feature = "agent")]
    pub(crate) fn managed_agent_resolution(
        step_id: impl Into<String>,
        request_id: impl Into<String>,
        source: crate::adapters::agent::ManagedAgentError,
    ) -> Self {
        Self::ManagedAgentResolution {
            step_id: step_id.into(),
            request_id: request_id.into(),
            source: Box::new(source),
        }
    }

    /// Bind every sealable failure to the graph step that owns the act.
    ///
    /// Nested skills and adapters may name their own capability, which is useful
    /// diagnostic context but is not necessarily the outer graph step id. The
    /// dispatch chokepoint owns that identity, so it preserves governed control
    /// outcomes and normalizes all other sealable faults into one step-scoped
    /// failure before terminal receipt sealing.
    pub(crate) fn at_graph_step(self, step_id: &str) -> Self {
        match self {
            Self::GraphBlocked { reason, .. } => Self::GraphBlocked {
                step_id: step_id.to_owned(),
                reason,
            },
            Self::ResolutionPending {
                reason,
                request,
                child,
                ..
            } => Self::ResolutionPending {
                step_id: step_id.to_owned(),
                reason,
                request,
                child,
            },
            Self::AuthorityDenied { verb, reason, .. } => Self::AuthorityDenied {
                verb,
                step_id: step_id.to_owned(),
                reason,
            },
            Self::ProviderReadbackPending { reason, .. } => Self::ProviderReadbackPending {
                step_id: step_id.to_owned(),
                reason,
            },
            #[cfg(feature = "agent")]
            Self::ManagedAgentResolution {
                request_id, source, ..
            } => Self::ManagedAgentResolution {
                step_id: step_id.to_owned(),
                request_id,
                source,
            },
            Self::InputContract {
                owner,
                input,
                path,
                message,
                accepted_schema,
                ..
            } => Self::InputContract {
                step_id: Some(step_id.to_owned()),
                owner,
                input,
                path,
                message,
                accepted_schema,
            },
            error if error.is_fatal_step_fault() => error,
            error => Self::InvalidRunStep {
                step_id: step_id.to_owned(),
                reason: error.to_string(),
            },
        }
    }
}
