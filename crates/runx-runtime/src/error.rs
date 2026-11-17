use runx_contracts::{AuthorityVerb, JsonObject, JsonValue};
use thiserror::Error;

use crate::credentials::CredentialDeliveryError;
use crate::failure::Failure;

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

/// What a mismatch records of a value: the value as it is, or its digest
/// when it comes from a field the call marks never to persist.
#[derive(Clone, Debug, PartialEq)]
pub enum Recorded {
    Value(JsonValue),
    Digest(String),
}

impl std::fmt::Display for Recorded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(value) => formatter.write_str(
                &runx_contracts::canonical_stable_json(value)
                    .unwrap_or_else(|_| "an unprintable value".to_owned()),
            ),
            Self::Digest(digest) => write!(formatter, "a value with digest {digest}"),
        }
    }
}

/// A field a provider's answer states with another value than the call
/// expected: what was expected of it, and what it actually is, or none where
/// the provider says it has no single value.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    pub field: String,
    pub expected: Recorded,
    pub actual: Option<Recorded>,
}

impl Mismatch {
    /// The mismatch at `field`, a dotted path. Where the field meets a path
    /// in `withheld`, the result paths the call marks never to persist, each
    /// value is recorded by its digest.
    pub(crate) fn new(
        field: impl Into<String>,
        expected: JsonValue,
        actual: Option<JsonValue>,
        withheld: &[Vec<String>],
    ) -> Result<Self, String> {
        let field = field.into();
        let segments = field.split('.').collect::<Vec<_>>();
        let digested = withheld.iter().any(|path| {
            path.iter()
                .zip(&segments)
                .all(|(withheld, segment)| withheld == segment)
        });
        let recorded = |value: JsonValue| match digested {
            false => Ok(Recorded::Value(value)),
            true => runx_contracts::to_canonical_bytes(&value)
                .map(|bytes| Recorded::Digest(runx_contracts::sha256_prefixed(&bytes)))
                .map_err(|_| format!("the value at {field} cannot be digested")),
        };
        Ok(Self {
            expected: recorded(expected)?,
            actual: actual.map(recorded).transpose()?,
            field,
        })
    }
}

/// A provider's answer that contradicts what the call expected, in every
/// field it contradicts: a write's own answer, or a read that checks it. The
/// outcome is definite: the provider has what it states, so the call
/// settles failed, naming what it has for a person to clean up, and no
/// retry is offered.
#[derive(Clone, Debug, PartialEq)]
pub struct Contradiction {
    pub mismatches: Vec<Mismatch>,
}

impl std::fmt::Display for Contradiction {
    /// Each field, what it actually is and what was expected.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, mismatch) in self.mismatches.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            match &mismatch.actual {
                Some(actual) => write!(
                    formatter,
                    "{} is {actual} where {} was expected",
                    mismatch.field, mismatch.expected
                )?,
                None => write!(
                    formatter,
                    "{} has no single value where {} was expected",
                    mismatch.field, mismatch.expected
                )?,
            }
        }
        Ok(())
    }
}

impl Contradiction {
    pub(crate) fn reason(&self) -> String {
        format!("the provider's answer contradicts the call: {self}")
    }

    /// The contradiction with a presented credential removed from the values
    /// it records.
    pub(crate) fn scrub(
        self,
        secret: &crate::kernel::operation::credentials::CredentialSecret,
    ) -> Self {
        let scrubbed = |recorded: Recorded| match recorded {
            Recorded::Value(value) => Recorded::Value(secret.scrub(value)),
            digest @ Recorded::Digest(_) => digest,
        };
        Self {
            mismatches: self
                .mismatches
                .into_iter()
                .map(|mismatch| Mismatch {
                    field: secret.scrub_text(mismatch.field),
                    expected: scrubbed(mismatch.expected),
                    actual: mismatch.actual.map(scrubbed),
                })
                .collect(),
        }
    }

    /// The failure record's list: each field, with each value as it is or by
    /// its digest.
    fn record(&self) -> JsonValue {
        JsonValue::Array(
            self.mismatches
                .iter()
                .map(|mismatch| {
                    let mut record = JsonObject::from([(
                        "field".to_owned(),
                        JsonValue::String(mismatch.field.clone()),
                    )]);
                    for (name, recorded) in [
                        ("expected", Some(&mismatch.expected)),
                        ("actual", mismatch.actual.as_ref()),
                    ]
                    .into_iter()
                    .filter_map(|(name, recorded)| Some((name, recorded?)))
                    {
                        match recorded {
                            Recorded::Value(value) => {
                                record.insert(name.to_owned(), value.clone());
                            }
                            Recorded::Digest(digest) => {
                                record.insert(
                                    format!("{name}_digest"),
                                    JsonValue::String(digest.clone()),
                                );
                            }
                        }
                    }
                    JsonValue::Object(record)
                })
                .collect(),
        )
    }
}

/// What stops a run, a step or a call. Each says itself as one `Failure`
/// (`failure`), which its `Display` renders, so every surface says it once.
#[derive(Debug)]
pub enum RuntimeError {
    Io {
        context: String,
        source: std::io::Error,
    },
    ParseWorkflow(runx_parser::ParseError),
    ValidateWorkflow(runx_parser::ValidationError),
    SkillPackage(runx_parser::SkillPackageError),
    WorkspaceEnvironment(crate::WorkspaceEnvError),
    Json {
        context: String,
        source: serde_json::Error,
    },
    StepMissing {
        step_id: String,
    },
    StepMissingSkill {
        step_id: String,
    },
    /// What a tool, an operation, a skill or runx decided, said once.
    Failed(Box<Failure>),
    /// A failure that states its own record, such as an operation whose
    /// provider answered with what contradicts the call. Its run returns
    /// that record, as the call returned it.
    FailedRecorded {
        failure: Box<Failure>,
        record: JsonObject,
    },
    UnsupportedRunStep {
        step_id: String,
        run_type: String,
    },
    WorkflowBlocked {
        step_id: String,
        reason: String,
    },
    /// Its caller's host cancelled the call that ran this child run, which
    /// stops where it stands, at `step_id`, and seals killed.
    Cancelled {
        step_id: String,
    },
    ResolutionPending {
        step_id: String,
        reason: String,
        /// The request the innermost step asked its host.
        request: Box<runx_contracts::ResolutionRequest>,
        /// The child run that holds the request, when the step is a nested
        /// skill; `None` when the step asked its host itself.
        child: Option<String>,
    },
    /// A step sealed a governed outcome other than `completed`: a turn
    /// that declined, deferred, blocked, or was superseded. The workflow stops
    /// there with that outcome; it is an outcome, not a fault.
    StepClosed {
        step_id: String,
        outcome: runx_contracts::RunOutcome,
        /// The step receipt's own seal reason, which the workflow seal keeps.
        reason: Option<runx_contracts::SealReason>,
        /// What ended it, as its child run said.
        failure: Box<Failure>,
        /// Why a guard or an authority refused the child run the step ran,
        /// when one did, which its run returns to its caller.
        refusal: Option<Box<runx_contracts::JsonObject>>,
        /// Why the person declined a gate that stopped the child run, when
        /// they did.
        declined: Option<String>,
    },
    ContextEdgeUnresolved(Box<ContextEdgeUnresolvedError>),
    AuthorityDenied {
        verb: AuthorityVerb,
        step_id: String,
        reason: String,
    },
    /// A person declined the approval the step waited on: their no, not a
    /// refusal of runx's.
    Declined {
        step_id: String,
        reason: String,
    },
    /// The run's money limit doesn't cover a call: it would spend more than
    /// is left, its cost can't be counted, or it quotes a charge in a run
    /// that states no limit. Nothing was sent.
    CostLimitExceeded {
        step_id: String,
        reason: String,
    },
    /// A step or call would take the root run's receipt tree past its
    /// bound; nothing ran.
    ReceiptBudgetExceeded {
        step_id: String,
        bound: usize,
    },
    WorkflowPlanningFailed {
        step_id: String,
        reason: String,
    },
    #[cfg(feature = "agent")]
    DriveResolution {
        step_id: String,
        request_id: String,
        source: Box<crate::adapters::agent::DriveError>,
    },
    ProviderReadbackPending {
        step_id: String,
        reason: String,
        /// Reading back may itself charge the provider account. Pending
        /// finality is not permission to repeat an unpriced request.
        resend: runx_contracts::RetryResend,
    },
    /// The driver was asked to stop, so `step_id` was not admitted; the run
    /// stays where it is, and its next driver continues it.
    Stopped {
        run_id: String,
        step_id: String,
    },
    RunRecord(crate::execution::run_record::RunRecordError),
    UnsupportedAdapter {
        adapter_type: String,
    },
    EngineFailure {
        context: &'static str,
        source: Box<RuntimeError>,
    },
    EngineInvariant {
        context: &'static str,
        message: String,
    },
    UnsupportedExecutable {
        kind: String,
    },
    UnsupportedMethodSelection {
        method: String,
    },
    InvalidProcessInvocation {
        message: String,
    },
    MissingEnvironment {
        names: Vec<String>,
    },
    CredentialEnvironment {
        names: Vec<String>,
    },
    JavaScriptWorker {
        message: String,
    },
    CredentialDelivery(CredentialDeliveryError),
    EffectState {
        context: String,
        message: String,
    },
    /// A claimed step taken further with nobody asked found nothing that
    /// settles its act: it waits on its retry as it did.
    Unsettled {
        step_id: String,
    },
    ProviderEffectUnknown {
        step_id: String,
        plan_digest: String,
        idempotency_key: String,
        reason: String,
    },
    /// The provider answered and applied nothing: a definite, non-retryable
    /// refusal of the request, unlike an unknown outcome.
    ProviderEffectRejected {
        plan_digest: String,
        idempotency_key: String,
        provider_code: String,
        http_status: u16,
        reason: String,
        /// What the provider states its idempotency key is bound to, against
        /// the call, when it refused the key as another request's and said
        /// so.
        conflict: Option<Box<Contradiction>>,
    },
    /// The provider's answer, a write's own or a read that checks it,
    /// contradicts what the call expected: a definite outcome, whatever the
    /// provider applied. It names each field, what was expected and what it
    /// actually is, for a person to clean up, and offers no retry.
    ProviderEffectContradicted {
        plan_digest: String,
        idempotency_key: String,
        contradiction: Contradiction,
    },
    /// An authored skill change runx refused, at its bind or its apply: the
    /// step that refused it says which.
    SkillChangeRefused {
        message: String,
    },
    #[cfg(feature = "agent")]
    ModelProvider {
        detail: String,
    },
    /// The model's provider couldn't answer now and may later: a 408, 429,
    /// 5xx or 529, or no connection. The driver asks again (contracts/driver.md).
    #[cfg(feature = "agent")]
    ModelUnavailable {
        detail: String,
        /// The provider's `Retry-After`, when it sent one.
        retry_after: Option<std::time::Duration>,
    },
    /// The model's reply was cut at its output cap of `output_cap` tokens. The
    /// driver asks again with more room, keeping what the cut ask used.
    #[cfg(feature = "agent")]
    ModelReplyCut {
        output_cap: u32,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    ContinuationRejected {
        reason: String,
    },
    InputContract {
        step_id: Option<String>,
        declared_by: &'static str,
        input: String,
        path: String,
        message: String,
        accepted_schema: Box<runx_contracts::JsonValue>,
    },
    ReceiptInvalid {
        message: String,
    },
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.failure().fmt(formatter)
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::ParseWorkflow(source) => Some(source),
            Self::ValidateWorkflow(source) => Some(source),
            Self::SkillPackage(source) => Some(source),
            Self::WorkspaceEnvironment(source) => Some(source),
            Self::RunRecord(source) => Some(source),
            Self::CredentialDelivery(source) => Some(source),
            Self::EngineFailure { source, .. } => Some(source.as_ref()),
            #[cfg(feature = "agent")]
            Self::DriveResolution { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

macro_rules! runtime_error_from {
    ($($source:ty => $variant:ident),* $(,)?) => {
        $(impl From<$source> for RuntimeError {
            fn from(source: $source) -> Self {
                Self::$variant(source)
            }
        })*
    };
}

runtime_error_from! {
    runx_parser::ParseError => ParseWorkflow,
    runx_parser::ValidationError => ValidateWorkflow,
    runx_parser::SkillPackageError => SkillPackage,
    crate::WorkspaceEnvError => WorkspaceEnvironment,
    crate::execution::run_record::RunRecordError => RunRecord,
    CredentialDeliveryError => CredentialDelivery,
}

/// Why a step or call is refused at the run's receipt budget: it names the
/// bound.
pub(crate) fn receipt_budget_reason(bound: usize) -> String {
    format!("it would take the run's receipt tree past its bound of {bound} receipts")
}

impl RuntimeError {
    /// What `decided_by`, a tool, an operation, a skill or one of runx's
    /// layers by the name its reader knows, decided, for `reason`.
    pub(crate) fn failed(decided_by: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Failed(Box::new(Failure::new(decided_by, reason)))
    }

    /// What runx decided of step `step_id`, for `reason`.
    pub(crate) fn step_failed(step_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Failed(Box::new(
            Failure::new("runx", reason).at_step(&step_id.into()),
        ))
    }

    /// The failure as one sentence's parts: why, who decided, where, and
    /// what to do next. Every surface renders this, and nothing else.
    #[must_use]
    pub fn failure(&self) -> Failure {
        let runx = |reason: String| Failure::new("runx", reason);
        let package = |reason: String| Failure::new("the skill's package", reason);
        let provider = |reason: String| Failure::new("the provider", reason);
        match self {
            Self::Failed(failure) | Self::FailedRecorded { failure, .. } => failure.as_ref().clone(),
            Self::StepClosed { failure, step_id, .. } => failure.as_ref().clone().at_step(step_id),
            Self::Io { context, source } => runx(format!("I/O failed while {context}: {source}")),
            Self::Json { context, source } => {
                runx(format!("JSON failed while {context}: {source}"))
            }
            Self::ParseWorkflow(error) => package(format!("its workflow does not parse: {error}")),
            Self::ValidateWorkflow(error) => package(format!("its workflow is invalid: {error}")),
            Self::SkillPackage(error) => package(format!("it is invalid: {error}")),
            Self::WorkspaceEnvironment(error) => {
                runx(format!("the workspace's environment failed: {error}"))
            }
            Self::StepMissing { step_id } => runx(format!("the workflow has no step {step_id}")),
            Self::StepMissingSkill { step_id } => {
                package("the step names no skill".to_owned()).at_step(step_id)
            }
            Self::UnsupportedRunStep { step_id, run_type } => {
                runx(format!("it runs no {run_type} step")).at_step(step_id)
            }
            Self::WorkflowBlocked { step_id, reason } => {
                runx(format!("blocked: {reason}")).at_step(step_id)
            }
            Self::Cancelled { step_id } => {
                Failure::new("its caller", "it cancelled the call that ran this run")
                    .at_step(step_id)
            }
            Self::ResolutionPending {
                step_id, reason, ..
            } => runx(format!("it waits on its host: {reason}")).at_step(step_id),
            Self::ContextEdgeUnresolved(error) => {
                runx(error.to_string()).at_step(&error.to_step)
            }
            Self::AuthorityDenied {
                verb,
                step_id,
                reason,
            } => Failure::new("the run's authority", format!("{verb:?} denied: {reason}"))
                .at_step(step_id),
            Self::Declined { step_id, reason } => {
                Failure::new("a person", format!("declined: {reason}")).at_step(step_id)
            }
            Self::CostLimitExceeded { step_id, reason } => {
                Failure::new("the run's cost limit", reason.clone()).at_step(step_id)
            }
            Self::ReceiptBudgetExceeded { step_id, bound } => {
                Failure::new("the run's receipt budget", receipt_budget_reason(*bound))
                    .at_step(step_id)
            }
            Self::WorkflowPlanningFailed { step_id, reason } => {
                runx(format!("planning failed: {reason}")).at_step(step_id)
            }
            #[cfg(feature = "agent")]
            Self::DriveResolution {
                step_id, source, ..
            } => {
                let failure = Failure::new("the driven turn", source.to_string()).at_step(step_id);
                match (source.next_step(), source.reason()) {
                    (Some(next), _) => failure.next(next),
                    (None, runx_contracts::SealReason::DriveRoundBudgetExhausted) => failure
                        .next("give it a narrower task, or more rounds with --drive-rounds"),
                    _ => failure,
                }
            }
            Self::ProviderReadbackPending {
                step_id, reason, ..
            } => provider(format!(
                "the change was applied, and reading it back is unavailable: {reason}"
            ))
            .at_step(step_id),
            Self::Stopped { run_id, step_id } => {
                runx(format!("run {run_id} stopped before {step_id}"))
                    .next(format!("runx resume {run_id} continues it"))
            }
            Self::RunRecord(error) => Failure::new("the run's record", error.to_string()),
            Self::UnsupportedAdapter { adapter_type } => {
                runx(format!("it runs no {adapter_type} adapter"))
            }
            Self::EngineFailure { context, source } => {
                let failure = source.failure();
                Failure {
                    reason: format!("while {context}, {}", failure.reason),
                    ..failure
                }
            }
            Self::EngineInvariant { context, message } => {
                runx(format!("an invariant failed while {context}: {message}"))
            }
            Self::UnsupportedExecutable { kind } => {
                runx(format!("this adapter runs no {kind} executable"))
            }
            Self::UnsupportedMethodSelection { method } => {
                runx(format!("it runs no method selection {method} yet"))
            }
            Self::InvalidProcessInvocation { message } => {
                runx(format!("the process invocation is invalid: {message}"))
            }
            Self::MissingEnvironment { names } => Failure::new(
                "the skill's environment",
                format!("it needs {}, which is unset", names.join(", ")),
            )
            .next(format!("set {} and run it again", names.join(", "))),
            Self::CredentialEnvironment { names } => runx(format!(
                "{} hold a connector credential, which a skill's declared environment never carries",
                names.join(", ")
            )),
            Self::JavaScriptWorker { message } => Failure::new("the worker", message.clone()),
            Self::CredentialDelivery(error) => {
                Failure::new("the credential's delivery", error.to_string())
            }
            Self::EffectState { context, message } => {
                runx(format!("effect state failed while {context}: {message}"))
            }
            Self::Unsettled { step_id } => runx(
                "it has not settled, and nothing custody holds of its act settles it, so it waits on its retry"
                    .to_owned(),
            )
            .at_step(step_id),
            // runx can't tell whether the effect happened; the provider's own
            // words say why.
            Self::ProviderEffectUnknown {
                step_id,
                idempotency_key,
                reason,
                ..
            } => runx(format!("the effect's outcome is unknown: {reason}"))
                .next(format!(
                    "runx resume retries it under idempotency key {idempotency_key}"
                ))
                .at_step(step_id),
            Self::ProviderEffectRejected {
                plan_digest,
                idempotency_key,
                provider_code,
                http_status,
                reason,
                ..
            } => provider(format!(
                "it rejected the effect for plan {plan_digest} with idempotency key {idempotency_key} ({provider_code}, HTTP {http_status}): {reason}"
            )),
            Self::ProviderEffectContradicted {
                plan_digest,
                idempotency_key,
                contradiction,
            } => provider(format!(
                "its answer contradicts the call for plan {plan_digest} with idempotency key {idempotency_key}: {contradiction}"
            )),
            Self::SkillChangeRefused { message } => {
                runx(format!("the skill change is refused: {message}"))
            }
            #[cfg(feature = "agent")]
            Self::ModelProvider { detail } | Self::ModelUnavailable { detail, .. } => {
                Failure::new("the model provider", detail.clone())
            }
            #[cfg(feature = "agent")]
            Self::ModelReplyCut { output_cap, .. } => Failure::new(
                "the model provider",
                format!("its reply was cut at its output cap of {output_cap} tokens"),
            ),
            Self::ContinuationRejected { reason } => runx(reason.clone()),
            Self::InputContract {
                step_id,
                declared_by,
                input,
                path,
                message,
                ..
            } => {
                let failure = Failure::new(
                    format!("the {declared_by}'s input contract"),
                    format!("input {input} at {path}: {message}"),
                )
                .next(format!("pass {input} as the contract's schema accepts it"));
                match step_id {
                    Some(step_id) => failure.at_step(step_id),
                    None => failure,
                }
            }
            Self::ReceiptInvalid { message } => {
                runx(format!("the receipt is invalid: {message}"))
            }
        }
    }

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
        // Never absorb a governed flow outcome: the workflow engine and refusal
        // sealing match on these exact variants, so wrapping one would turn a
        // provable denial into an anonymous engine fault.
        match source {
            source @ (Self::AuthorityDenied { .. }
            | Self::Declined { .. }
            | Self::CostLimitExceeded { .. }
            | Self::ReceiptBudgetExceeded { .. }
            | Self::WorkflowBlocked { .. }
            | Self::Cancelled { .. }
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
            | Self::WorkflowPlanningFailed { .. }
            | Self::RunRecord(_)
            | Self::EngineFailure { .. }
            | Self::EngineInvariant { .. }
            | Self::EffectState { .. }
            | Self::Unsettled { .. }
            | Self::ProviderEffectUnknown { .. }
            | Self::ProviderReadbackPending { .. }
            | Self::ReceiptInvalid { .. }
            | Self::ContinuationRejected { .. }
            | Self::Stopped { .. } => true,
            Self::Io { .. }
            | Self::ParseWorkflow(_)
            | Self::ValidateWorkflow(_)
            | Self::SkillPackage(_)
            | Self::WorkspaceEnvironment(_)
            | Self::Json { .. }
            | Self::StepMissingSkill { .. }
            | Self::Failed(_)
            | Self::FailedRecorded { .. }
            | Self::UnsupportedRunStep { .. }
            | Self::WorkflowBlocked { .. }
            | Self::Cancelled { .. }
            | Self::ResolutionPending { .. }
            | Self::StepClosed { .. }
            | Self::ContextEdgeUnresolved(_)
            | Self::AuthorityDenied { .. }
            | Self::Declined { .. }
            | Self::CostLimitExceeded { .. }
            | Self::ReceiptBudgetExceeded { .. }
            | Self::UnsupportedAdapter { .. }
            | Self::UnsupportedExecutable { .. }
            | Self::UnsupportedMethodSelection { .. }
            | Self::InvalidProcessInvocation { .. }
            | Self::MissingEnvironment { .. }
            | Self::CredentialEnvironment { .. }
            | Self::JavaScriptWorker { .. }
            | Self::CredentialDelivery(_)
            | Self::ProviderEffectRejected { .. }
            | Self::ProviderEffectContradicted { .. }
            | Self::SkillChangeRefused { .. } => false,
            Self::InputContract { .. } => false,
            #[cfg(feature = "agent")]
            Self::DriveResolution { .. }
            | Self::ModelProvider { .. }
            | Self::ModelUnavailable { .. }
            | Self::ModelReplyCut { .. } => false,
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

    /// Safe structured failure returned by both direct and workflow-backed skill
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
                declared_by,
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
                projection.insert(
                    "declared_by".to_owned(),
                    JsonValue::String((*declared_by).to_owned()),
                );
                projection.insert("input".to_owned(), JsonValue::String(input.clone()));
                projection.insert("path".to_owned(), JsonValue::String(path.clone()));
                projection.insert(
                    "accepted_schema".to_owned(),
                    accepted_schema.as_ref().clone(),
                );
            }
            #[cfg(feature = "agent")]
            Self::DriveResolution { source, .. } => {
                projection = source.public_failure_projection();
            }
            Self::ProviderEffectRejected {
                provider_code,
                http_status,
                reason,
                conflict,
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
                if let Some(conflict) = conflict {
                    projection.insert("mismatches".to_owned(), conflict.record());
                }
            }
            Self::FailedRecorded { record, .. } => {
                projection = record.clone();
            }
            Self::ProviderEffectContradicted { contradiction, .. } => {
                projection.insert(
                    "code".to_owned(),
                    JsonValue::String("provider_contradicted".to_owned()),
                );
                projection.insert("retryable".to_owned(), JsonValue::Bool(false));
                projection.insert("mismatches".to_owned(), contradiction.record());
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
            Self::Declined { step_id, reason } => {
                projection.insert("code".to_owned(), JsonValue::String("declined".to_owned()));
                projection.insert("step_id".to_owned(), JsonValue::String(step_id.clone()));
                projection.insert("reason".to_owned(), JsonValue::String(reason.clone()));
            }
            Self::CostLimitExceeded { step_id, reason } => {
                projection.insert(
                    "code".to_owned(),
                    JsonValue::String("cost_limit_exceeded".to_owned()),
                );
                projection.insert("step_id".to_owned(), JsonValue::String(step_id.clone()));
                projection.insert("retryable".to_owned(), JsonValue::Bool(false));
                projection.insert("reason".to_owned(), JsonValue::String(reason.clone()));
            }
            Self::ReceiptBudgetExceeded { step_id, bound } => {
                projection.insert(
                    "code".to_owned(),
                    JsonValue::String("receipt_budget_exceeded".to_owned()),
                );
                projection.insert("step_id".to_owned(), JsonValue::String(step_id.clone()));
                projection.insert("retryable".to_owned(), JsonValue::Bool(false));
                projection.insert(
                    "bound".to_owned(),
                    JsonValue::Number(runx_contracts::JsonNumber::U64(
                        u64::try_from(*bound).unwrap_or(u64::MAX),
                    )),
                );
                projection.insert(
                    "reason".to_owned(),
                    JsonValue::String(receipt_budget_reason(*bound)),
                );
            }
            Self::ProviderReadbackPending {
                step_id,
                reason,
                resend,
            } => {
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
                projection.insert(
                    "resend".to_owned(),
                    JsonValue::String(
                        match resend {
                            runx_contracts::RetryResend::Safe => "safe",
                            runx_contracts::RetryResend::Unsafe => "unsafe",
                        }
                        .to_owned(),
                    ),
                );
                projection.insert("reason".to_owned(), JsonValue::String(reason.clone()));
            }
            _ => {}
        }
        projection
    }

    /// Authoritative workflow-step identity attached by the dispatch chokepoint.
    /// Terminal sealing consumes this instead of guessing identity from error
    /// prose or nested capability names.
    pub(crate) fn workflow_step_id(&self) -> Option<&str> {
        match self {
            Self::Failed(failure) | Self::FailedRecorded { failure, .. } => failure.step(),
            Self::StepMissing { step_id }
            | Self::StepMissingSkill { step_id }
            | Self::UnsupportedRunStep { step_id, .. }
            | Self::WorkflowBlocked { step_id, .. }
            | Self::Cancelled { step_id }
            | Self::ResolutionPending { step_id, .. }
            | Self::StepClosed { step_id, .. }
            | Self::AuthorityDenied { step_id, .. }
            | Self::Declined { step_id, .. }
            | Self::CostLimitExceeded { step_id, .. }
            | Self::ReceiptBudgetExceeded { step_id, .. }
            | Self::WorkflowPlanningFailed { step_id, .. }
            | Self::ProviderReadbackPending { step_id, .. } => Some(step_id),
            Self::InputContract { step_id, .. } => step_id.as_deref(),
            #[cfg(feature = "agent")]
            Self::DriveResolution { step_id, .. } => Some(step_id),
            _ => None,
        }
    }

    #[cfg(feature = "agent")]
    pub(crate) fn drive_resolution(
        step_id: impl Into<String>,
        request_id: impl Into<String>,
        source: crate::adapters::agent::DriveError,
    ) -> Self {
        Self::DriveResolution {
            step_id: step_id.into(),
            request_id: request_id.into(),
            source: Box::new(source),
        }
    }

    /// Bind every sealable failure to the workflow step that owns the act.
    ///
    /// Nested skills and adapters may name their own capability, which is useful
    /// diagnostic context but is not necessarily the outer workflow step id. The
    /// dispatch chokepoint owns that identity, so it preserves governed control
    /// outcomes and normalizes all other sealable faults into one step-scoped
    /// failure before terminal receipt sealing.
    pub(crate) fn at_workflow_step(self, step_id: &str) -> Self {
        match self {
            Self::WorkflowBlocked { reason, .. } => Self::WorkflowBlocked {
                step_id: step_id.to_owned(),
                reason,
            },
            Self::Cancelled { .. } => Self::Cancelled {
                step_id: step_id.to_owned(),
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
            Self::Declined { reason, .. } => Self::Declined {
                step_id: step_id.to_owned(),
                reason,
            },
            Self::CostLimitExceeded { reason, .. } => Self::CostLimitExceeded {
                step_id: step_id.to_owned(),
                reason,
            },
            Self::ReceiptBudgetExceeded { bound, .. } => Self::ReceiptBudgetExceeded {
                step_id: step_id.to_owned(),
                bound,
            },
            Self::ProviderReadbackPending { reason, resend, .. } => Self::ProviderReadbackPending {
                step_id: step_id.to_owned(),
                reason,
                resend,
            },
            #[cfg(feature = "agent")]
            Self::DriveResolution {
                request_id, source, ..
            } => Self::DriveResolution {
                step_id: step_id.to_owned(),
                request_id,
                source,
            },
            Self::InputContract {
                declared_by,
                input,
                path,
                message,
                accepted_schema,
                ..
            } => Self::InputContract {
                step_id: Some(step_id.to_owned()),
                declared_by,
                input,
                path,
                message,
                accepted_schema,
            },
            error if error.is_fatal_step_fault() => error,
            // Already a failure: placed at the step once, never wrapped.
            Self::Failed(failure) => Self::Failed(Box::new(failure.at_step(step_id))),
            Self::FailedRecorded { failure, record } => Self::FailedRecorded {
                failure: Box::new(failure.at_step(step_id)),
                record,
            },
            error => Self::Failed(Box::new(error.failure().at_step(step_id))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Failure, RuntimeError};

    /// A failed step names its step once, however many layers scope it.
    #[test]
    fn a_step_error_scoped_again_is_never_wrapped_in_itself() {
        let failed = RuntimeError::failed("data.digest", "the turn requires fresh_context");
        let scoped = failed.at_workflow_step("answer").at_workflow_step("answer");
        assert_eq!(
            scoped.to_string(),
            "the turn requires fresh_context (data.digest, at answer)."
        );
    }

    /// A step's recorded failure is what its run returns, however many layers
    /// scope it.
    #[test]
    fn a_recorded_step_failure_projects_its_own_record() {
        let record = runx_contracts::JsonObject::from([(
            "code".to_owned(),
            runx_contracts::JsonValue::String("provider_contradicted".to_owned()),
        )]);
        let failed = RuntimeError::FailedRecorded {
            failure: Box::new(Failure::new(
                "the provider",
                "the provider's answer contradicts the call",
            )),
            record: record.clone(),
        };
        let scoped = failed.at_workflow_step("outer");
        assert_eq!(scoped.workflow_step_id(), Some("outer"));
        assert_eq!(scoped.public_failure_projection(), record);
    }
}
