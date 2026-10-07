// Module rationale: RuntimeOptions, checkpoint resume, and
// the public graph runner surface are still audited as one Rust cutover unit.
//! The act engine for runx: the single admit -> execute -> seal path every run
//! takes. A standalone skill is a one-act plan and a graph a multi-act plan;
//! both run through this one engine.
//!
//! The public surface lives here: [`Runtime`], [`RuntimeOptions`], [`StepRun`],
//! [`GraphRun`], and [`GraphCheckpoint`]. The internal state machine and the
//! per-step execution helpers live in private submodules.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{ClosureDisposition, FanoutReceiptSyncPoint, JsonObject, JsonValue, Receipt};
use runx_core::state_machine::{
    GraphStatus, GraphStepStatus, SequentialGraphState, StepAdmissionWitness,
};
use runx_parser::ExecutionGraph;
use serde::{Deserialize, Serialize};

use super::graph::load_graph;
use crate::RuntimeError;
use crate::adapter::{
    EphemeralValue, InvocationDiagnostics, InvocationOutput, InvocationStatus, SkillAdapter,
};
use crate::effects::RuntimeEffectRegistry;
use crate::host::{Host, NoopHost};
use crate::journal::ExecutionJournal;
use crate::lifecycle::LifecycleEvent;
use crate::receipts::signing::strip_receipt_signing_env;
use crate::receipts::{
    RuntimeReceiptSignatureConfig, RuntimeReceiptSignaturePolicy,
    graph_receipt_with_disposition_and_policy, graph_receipt_with_effects_and_signature_policy,
};
use crate::services::ReceiptServices;

mod admission;
mod dispatch;
mod graph_engine;
mod scheduler;
mod step_handlers;
mod sync;

use graph_engine::GraphExecution;

pub const RUNX_MAX_FANOUT_CONCURRENCY_ENV: &str = "RUNX_MAX_FANOUT_CONCURRENCY";
pub const RUNX_RUN_ID_ENV: &str = "RUNX_RUN_ID";

#[derive(Clone, Debug)]
pub struct RuntimeOptions {
    pub current_context: Vec<runx_contracts::ContextEntry>,
    pub historical_context: Vec<runx_contracts::ContextEntry>,
    pub run_authority: Option<crate::RunAuthority>,
    pub created_at: String,
    pub env: BTreeMap<String, String>,
    pub receipt_signature: RuntimeReceiptSignatureConfig,
    pub effects: RuntimeEffectRegistry,
}

impl RuntimeOptions {
    #[must_use]
    pub fn local_development(env: BTreeMap<String, String>) -> Self {
        Self::from_env_and_signature(env, RuntimeReceiptSignatureConfig::local_development())
    }

    pub fn from_env(env: BTreeMap<String, String>) -> Result<Self, RuntimeError> {
        let receipt_services =
            ReceiptServices::from_env(&env).map_err(|error| RuntimeError::ReceiptInvalid {
                message: error.to_string(),
            })?;
        Ok(Self::from_env_and_signature(
            env,
            receipt_services.signature_config().clone(),
        ))
    }

    pub fn from_env_or_local_development(
        env: BTreeMap<String, String>,
    ) -> Result<Self, RuntimeError> {
        let receipt_services =
            ReceiptServices::from_env_or_local_development(&env).map_err(|error| {
                RuntimeError::ReceiptInvalid {
                    message: error.to_string(),
                }
            })?;
        Ok(Self::from_env_and_signature(
            env,
            receipt_services.signature_config().clone(),
        ))
    }

    fn from_env_and_signature(
        mut env: BTreeMap<String, String>,
        receipt_signature: RuntimeReceiptSignatureConfig,
    ) -> Self {
        strip_receipt_signing_env(&mut env);
        Self {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            created_at: crate::time::now_iso8601(),
            env,
            receipt_signature,
            effects: RuntimeEffectRegistry::default(),
        }
    }

    #[cfg(feature = "cli-tool")]
    pub(crate) fn receipt_services(&self) -> ReceiptServices {
        ReceiptServices::from_signature_config(self.receipt_signature.clone())
    }

    #[must_use]
    pub fn signature_policy(&self) -> RuntimeReceiptSignaturePolicy<'_> {
        self.receipt_signature.signature_policy()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepOutcome {
    pub status: InvocationStatus,
    pub diagnostics: InvocationDiagnostics,
    pub metadata: JsonObject,
}

impl StepOutcome {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.status == InvocationStatus::Success
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
            InvocationDiagnostics::Process { exit_code, .. } => {
                Some(format!("process failed with exit code {exit_code:?}"))
            }
        }
    }
}

impl From<InvocationOutput> for StepOutcome {
    fn from(output: InvocationOutput) -> Self {
        Self {
            status: output.status,
            diagnostics: output.diagnostics,
            metadata: output.metadata,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepRun {
    pub step_id: String,
    pub attempt: u32,
    pub skill: String,
    pub runner: Option<String>,
    pub fanout_group: Option<String>,
    /// The one semantic, addressable output surface retained by graph state.
    pub contract: JsonObject,
    /// Immediate caller-only overlay. It exists only in the live process and is
    /// deliberately absent from checkpoints, receipts, and graph context.
    #[doc(hidden)]
    #[serde(skip, default)]
    pub ephemeral_contract: EphemeralValue,
    /// Bounded execution diagnostics and verification metadata. The adapter's
    /// raw value is consumed during projection and receipt sealing.
    pub outcome: StepOutcome,
    pub receipt: Receipt,
    /// The flattened receipt subtree below this step, ordered descendants
    /// before their parent roots for durable persistence. The step receipt
    /// carries the direct-child references; this collection supplies the
    /// referenced receipts without embedding child graph output payloads.
    pub nested_receipts: Vec<Receipt>,
    pub admission_witness: StepAdmissionWitness,
}

#[derive(Clone, Debug)]
pub struct GraphRun {
    pub graph: ExecutionGraph,
    pub state: SequentialGraphState,
    pub steps: Vec<StepRun>,
    pub sync_points: Vec<FanoutReceiptSyncPoint>,
    pub receipt: Receipt,
    pub journal: ExecutionJournal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphCheckpoint {
    pub graph_name: String,
    pub state: SequentialGraphState,
    pub steps: Vec<StepRun>,
    pub sync_points: Vec<FanoutReceiptSyncPoint>,
    pub journal: ExecutionJournal,
    /// A host-suspended step retains its attempt and any unfinished child graph.
    /// This is allocated only when execution yields, never for completed steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended_step: Option<Box<SuspendedGraphStep>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuspendedGraphStep {
    pub step_id: String,
    pub child: Option<Box<GraphCheckpoint>>,
}

pub struct Runtime<A> {
    // The configured adapter owns pluggable source kinds. JavaScript execution
    // and local artifacts remain concrete runtime services because their
    // isolation, credential projection, and receipt semantics are enforced by
    // the native runtime rather than delegated to an adapter.
    configured_adapter: A,
    javascript: crate::adapters::javascript::JavaScriptAdapter,
    local_artifacts: crate::services::LocalArtifactService,
    options: Arc<RuntimeOptions>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GraphTerminalOutcome {
    Propagate,
    SealReceipt,
}

impl<A> Runtime<A>
where
    A: SkillAdapter,
{
    pub fn new(adapter: A, options: RuntimeOptions) -> Self {
        let max_concurrency = scheduler::configured_max_concurrency(&options.env);
        Self {
            configured_adapter: adapter,
            javascript: crate::adapters::javascript::JavaScriptAdapter::with_max_concurrency(
                max_concurrency,
            ),
            local_artifacts: crate::services::LocalArtifactService::default(),
            options: Arc::new(options),
        }
    }

    pub(crate) fn with_native_services(
        adapter: A,
        options: impl Into<Arc<RuntimeOptions>>,
        javascript: crate::adapters::javascript::JavaScriptAdapter,
        local_artifacts: crate::services::LocalArtifactService,
    ) -> Self {
        Self {
            configured_adapter: adapter,
            javascript,
            local_artifacts,
            options: options.into(),
        }
    }

    pub(crate) fn options(&self) -> &RuntimeOptions {
        self.options.as_ref()
    }

    #[must_use]
    pub fn javascript_session_stats(&self) -> crate::adapters::javascript::JavaScriptSessionStats {
        self.javascript.session_stats()
    }

    pub fn run_graph_file(&self, graph_path: &Path) -> Result<GraphRun, RuntimeError> {
        let mut host = NoopHost;
        self.run_graph_file_with_host(graph_path, &mut host)
    }

    pub fn run_graph_file_with_host(
        &self,
        graph_path: &Path,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        let graph_path = self.resolved_graph_path(graph_path)?;
        let graph = load_graph(&graph_path)?;
        let graph_dir = graph_path.parent().unwrap_or_else(|| Path::new("/"));
        self.run_graph_with_host_outcome(graph_dir, graph, host, GraphTerminalOutcome::Propagate)
    }

    pub(crate) fn run_graph_file_for_harness(
        &self,
        graph_path: &Path,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        let graph_path = self.resolved_graph_path(graph_path)?;
        let graph = load_graph(&graph_path)?;
        let graph_dir = graph_path.parent().unwrap_or_else(|| Path::new("/"));
        self.run_graph_with_host_outcome(graph_dir, graph, host, GraphTerminalOutcome::SealReceipt)
    }

    pub fn run_graph_with_host(
        &self,
        graph_dir: &Path,
        graph: ExecutionGraph,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        self.run_graph_with_host_outcome(graph_dir, graph, host, GraphTerminalOutcome::Propagate)
    }

    pub(crate) fn run_graph_for_harness(
        &self,
        graph_dir: &Path,
        graph: ExecutionGraph,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        self.run_graph_with_host_outcome(graph_dir, graph, host, GraphTerminalOutcome::SealReceipt)
    }

    // Function rationale: graph execution drives one ordered
    // ready-node loop (admit, dispatch to host, fold outcomes, advance frontier)
    // whose step sequencing must stay in a single scope to keep the run auditable.
    fn run_graph_with_host_outcome(
        &self,
        graph_dir: &Path,
        graph: ExecutionGraph,
        host: &mut dyn Host,
        terminal_outcome: GraphTerminalOutcome,
    ) -> Result<GraphRun, RuntimeError> {
        let mut execution = GraphExecution::new(&graph);
        match execution.run(self, graph_dir, &graph, host, None) {
            Ok(()) => {
                let receipt = graph_receipt_with_effects_and_signature_policy(
                    &graph.name,
                    &mut execution.runs,
                    &execution.sync_points,
                    &self.options.created_at,
                    self.options.effects.clone(),
                    self.options.signature_policy(),
                )?;
                execution.record_lifecycle(
                    host,
                    LifecycleEvent::graph_completed(&graph.name, &receipt),
                )?;
                Ok(execution.finish(graph, receipt))
            }
            Err(RuntimeError::GraphBlocked { step_id, reason })
                if terminal_outcome == GraphTerminalOutcome::SealReceipt =>
            {
                let receipt = graph_receipt_with_disposition_and_policy(
                    &graph.name,
                    &mut execution.runs,
                    &execution.sync_points,
                    &self.options.created_at,
                    crate::receipts::GraphClosure {
                        disposition: ClosureDisposition::Blocked,
                        reason_code: "graph_blocked".to_owned(),
                        summary: format!("graph {} blocked at {step_id}: {reason}", graph.name),
                    },
                    self.options.effects.clone(),
                    self.options.signature_policy(),
                )?;
                execution.record_lifecycle(
                    host,
                    LifecycleEvent::graph_blocked(&graph.name, &step_id, &receipt),
                )?;
                Ok(execution.finish(graph, receipt))
            }
            // A governed authority denial is a policy block, not a runtime fault:
            // under the receipt-sealing outcome it seals a signed blocked receipt,
            // the same as any other graph block, so the refusal is provable.
            Err(RuntimeError::AuthorityDenied {
                verb,
                step_id,
                reason,
            }) if terminal_outcome == GraphTerminalOutcome::SealReceipt => {
                let receipt = graph_receipt_with_disposition_and_policy(
                    &graph.name,
                    &mut execution.runs,
                    &execution.sync_points,
                    &self.options.created_at,
                    crate::receipts::GraphClosure {
                        disposition: ClosureDisposition::Blocked,
                        reason_code: "authority_denied".to_owned(),
                        summary: format!(
                            "graph {} denied {verb:?} at {step_id}: {reason}",
                            graph.name
                        ),
                    },
                    self.options.effects.clone(),
                    self.options.signature_policy(),
                )?;
                execution.record_lifecycle(
                    host,
                    LifecycleEvent::graph_blocked(&graph.name, &step_id, &receipt),
                )?;
                Ok(execution.finish(graph, receipt))
            }
            Err(RuntimeError::ResolutionPending {
                step_id, reason, ..
            }) if terminal_outcome == GraphTerminalOutcome::SealReceipt => {
                let receipt = graph_receipt_with_disposition_and_policy(
                    &graph.name,
                    &mut execution.runs,
                    &execution.sync_points,
                    &self.options.created_at,
                    crate::receipts::GraphClosure {
                        disposition: ClosureDisposition::Deferred,
                        reason_code: "resolution_pending".to_owned(),
                        summary: format!("graph {} deferred at {step_id}: {reason}", graph.name),
                    },
                    self.options.effects.clone(),
                    self.options.signature_policy(),
                )?;
                execution.record_lifecycle(
                    host,
                    LifecycleEvent::graph_deferred(&graph.name, &step_id, &receipt),
                )?;
                Ok(execution.finish(graph, receipt))
            }
            Err(error) => Err(execution.checkpoint_error(&graph.name, error)),
        }
    }

    pub fn run_graph_file_until_steps(
        &self,
        graph_path: &Path,
        max_steps: usize,
    ) -> Result<GraphCheckpoint, RuntimeError> {
        let mut host = NoopHost;
        self.run_graph_file_until_steps_with_host(graph_path, max_steps, &mut host)
    }

    pub fn run_graph_file_until_steps_with_host(
        &self,
        graph_path: &Path,
        max_steps: usize,
        host: &mut dyn Host,
    ) -> Result<GraphCheckpoint, RuntimeError> {
        let graph_path = self.resolved_graph_path(graph_path)?;
        let graph = load_graph(&graph_path)?;
        let graph_dir = graph_path.parent().unwrap_or_else(|| Path::new("/"));
        self.run_graph_until_steps_with_host(graph_dir, &graph, max_steps, host)
    }

    pub fn run_graph_until_steps_with_host(
        &self,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        max_steps: usize,
        host: &mut dyn Host,
    ) -> Result<GraphCheckpoint, RuntimeError> {
        let mut execution = GraphExecution::new(graph);
        if let Err(error) = execution.run(self, graph_dir, graph, host, Some(max_steps)) {
            return Err(execution.checkpoint_error(&graph.name, error));
        }
        Ok(execution.checkpoint(graph.name.clone()))
    }

    pub fn resume_graph_file(
        &self,
        graph_path: &Path,
        checkpoint: GraphCheckpoint,
    ) -> Result<GraphRun, RuntimeError> {
        let mut host = NoopHost;
        self.resume_graph_file_with_host(graph_path, checkpoint, &mut host)
    }

    pub fn resume_graph_file_with_host(
        &self,
        graph_path: &Path,
        checkpoint: GraphCheckpoint,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        let graph_path = self.resolved_graph_path(graph_path)?;
        let graph = load_graph(&graph_path)?;
        let graph_dir = graph_path.parent().unwrap_or_else(|| Path::new("/"));
        self.resume_graph_with_host(graph_dir, graph, checkpoint, host)
    }

    fn resolved_graph_path(&self, graph_path: &Path) -> Result<PathBuf, RuntimeError> {
        if graph_path.is_absolute() {
            return Ok(crate::path_util::lexical_normalize(graph_path));
        }
        let workspace = self
            .options
            .env
            .get(crate::receipts::paths::RUNX_CWD_ENV)
            .map(PathBuf::from)
            .ok_or_else(|| RuntimeError::InvalidProcessInvocation {
                message: format!(
                    "relative graph path '{}' requires RUNX_CWD",
                    graph_path.display()
                ),
            })?;
        if !workspace.is_absolute() {
            return Err(RuntimeError::InvalidProcessInvocation {
                message: format!(
                    "RUNX_CWD must be an absolute path, got '{}'",
                    workspace.display()
                ),
            });
        }
        Ok(crate::path_util::lexical_normalize(
            &workspace.join(graph_path),
        ))
    }

    pub fn resume_graph_with_host(
        &self,
        graph_dir: &Path,
        graph: ExecutionGraph,
        checkpoint: GraphCheckpoint,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        let mut execution = GraphExecution::from_checkpoint(&graph, checkpoint)?;
        if let Err(error) = execution.run(self, graph_dir, &graph, host, None) {
            return Err(execution.checkpoint_error(&graph.name, error));
        }
        let receipt = graph_receipt_with_effects_and_signature_policy(
            &graph.name,
            &mut execution.runs,
            &execution.sync_points,
            &self.options.created_at,
            self.options.effects.clone(),
            self.options.signature_policy(),
        )?;
        execution.record_lifecycle(host, LifecycleEvent::graph_completed(&graph.name, &receipt))?;
        Ok(execution.finish(graph, receipt))
    }

    pub(crate) fn seal_completed_graph_checkpoint_with_host(
        &self,
        graph: ExecutionGraph,
        checkpoint: GraphCheckpoint,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        if checkpoint.state.status != GraphStatus::Succeeded {
            return Err(RuntimeError::GraphBlocked {
                step_id: "graph".to_owned(),
                reason: format!(
                    "cannot seal graph checkpoint with status {:?}",
                    checkpoint.state.status
                ),
            });
        }
        let mut execution = GraphExecution::from_checkpoint(&graph, checkpoint)?;
        let receipt = graph_receipt_with_effects_and_signature_policy(
            &graph.name,
            &mut execution.runs,
            &execution.sync_points,
            &self.options.created_at,
            self.options.effects.clone(),
            self.options.signature_policy(),
        )?;
        execution.record_lifecycle(host, LifecycleEvent::graph_completed(&graph.name, &receipt))?;
        Ok(execution.finish(graph, receipt))
    }

    pub(crate) fn seal_blocked_graph_checkpoint_with_host(
        &self,
        graph: ExecutionGraph,
        checkpoint: GraphCheckpoint,
        step_id: &str,
        reason_code: impl Into<String>,
        summary: impl Into<String>,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        let mut execution = GraphExecution::from_checkpoint(&graph, checkpoint)?;
        let receipt = graph_receipt_with_disposition_and_policy(
            &graph.name,
            &mut execution.runs,
            &execution.sync_points,
            &self.options.created_at,
            crate::receipts::GraphClosure {
                disposition: ClosureDisposition::Blocked,
                reason_code: reason_code.into(),
                summary: summary.into(),
            },
            self.options.effects.clone(),
            self.options.signature_policy(),
        )?;
        execution.record_lifecycle(
            host,
            LifecycleEvent::graph_blocked(&graph.name, step_id, &receipt),
        )?;
        Ok(execution.finish(graph, receipt))
    }

    pub(crate) fn seal_deferred_graph_checkpoint_with_host(
        &self,
        graph: ExecutionGraph,
        checkpoint: GraphCheckpoint,
        step_id: &str,
        reason_code: impl Into<String>,
        summary: impl Into<String>,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        let mut execution = GraphExecution::from_checkpoint(&graph, checkpoint)?;
        let receipt = graph_receipt_with_disposition_and_policy(
            &graph.name,
            &mut execution.runs,
            &execution.sync_points,
            &self.options.created_at,
            crate::receipts::GraphClosure {
                disposition: ClosureDisposition::Deferred,
                reason_code: reason_code.into(),
                summary: summary.into(),
            },
            self.options.effects.clone(),
            self.options.signature_policy(),
        )?;
        execution.record_lifecycle(
            host,
            LifecycleEvent::graph_deferred(&graph.name, step_id, &receipt),
        )?;
        Ok(execution.finish(graph, receipt))
    }

    pub(crate) fn seal_failed_graph_checkpoint_with_host(
        &self,
        graph: ExecutionGraph,
        checkpoint: GraphCheckpoint,
        step_id: &str,
        error: RuntimeError,
        closure: crate::receipts::GraphClosure,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        if closure.disposition != ClosureDisposition::Failed {
            return Err(RuntimeError::ReceiptInvalid {
                message: "failed graph checkpoint requires a failed closure".to_owned(),
            });
        }
        let attempt = checkpoint
            .state
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .map_or(1, |step| {
                if step.status == GraphStepStatus::Running {
                    step.attempts
                } else {
                    step.attempts.saturating_add(1).max(1)
                }
            });
        let step = graph
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .ok_or_else(|| RuntimeError::StepMissing {
                step_id: step_id.to_owned(),
            })?;
        let failed_run =
            step_handlers::runtime_error_step_run(self, &graph.name, step, attempt, error)?;
        let mut execution = GraphExecution::from_checkpoint(&graph, checkpoint)?;
        execution.record_terminal_step_failure(self, host, step_id, failed_run)?;
        let receipt = graph_receipt_with_disposition_and_policy(
            &graph.name,
            &mut execution.runs,
            &execution.sync_points,
            &self.options.created_at,
            closure,
            self.options.effects.clone(),
            self.options.signature_policy(),
        )?;
        execution.record_lifecycle(
            host,
            LifecycleEvent::graph_failed(&graph.name, step_id, &receipt),
        )?;
        Ok(execution.finish(graph, receipt))
    }

    pub fn resume_graph_until_steps_with_host(
        &self,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        checkpoint: GraphCheckpoint,
        max_steps: usize,
        host: &mut dyn Host,
    ) -> Result<GraphCheckpoint, RuntimeError> {
        let mut execution = GraphExecution::from_checkpoint(graph, checkpoint)?;
        if let Err(error) = execution.run(self, graph_dir, graph, host, Some(max_steps)) {
            return Err(execution.checkpoint_error(&graph.name, error));
        }
        Ok(execution.checkpoint(graph.name.clone()))
    }
}

/// Build the graph's public result from its explicit result producers.
///
/// A producer contributes its complete declared output contract, byte-for-byte.
/// In particular, artifact `{ data: ... }` envelopes are not unwrapped: those
/// envelopes are the addressable contract consumed by parent graphs. Mutually
/// exclusive terminal branches may name the same output because only one runs;
/// two successful producers emitting the same key are ambiguous and fail.
pub(crate) fn graph_run_result(run: &GraphRun) -> Result<JsonValue, RuntimeError> {
    let runs = run
        .steps
        .iter()
        .map(|step| (step.step_id.as_str(), step))
        .collect::<BTreeMap<_, _>>();
    let mut result = JsonObject::new();
    let mut contributing_steps = 0_usize;

    for step_id in &run.graph.result_from {
        let Some(step) = runs.get(step_id.as_str()) else {
            // A conditional terminal branch that did not run contributes
            // nothing. Parser validation guarantees the step exists in the
            // graph definition.
            continue;
        };
        if !step.outcome.succeeded() {
            continue;
        }
        let outputs = declared_step_outputs(step);
        if outputs.is_empty() {
            return Err(RuntimeError::InvalidRunStep {
                step_id: step_id.clone(),
                reason: "graph result producer emitted no declared outputs".to_owned(),
            });
        }
        contributing_steps += 1;
        for (name, value) in outputs {
            if result.insert(name.clone(), value).is_some() {
                return Err(RuntimeError::InvalidRunStep {
                    step_id: step_id.clone(),
                    reason: format!(
                        "graph result output {name:?} is emitted by more than one successful result producer"
                    ),
                });
            }
        }
    }

    if run.state.status == GraphStatus::Succeeded && contributing_steps == 0 {
        return Err(RuntimeError::SkillFailed {
            skill_name: run.graph.name.clone(),
            message: "graph succeeded without running a declared result producer".to_owned(),
        });
    }

    Ok(JsonValue::Object(result))
}

pub(crate) fn graph_run_ephemeral_result(run: &GraphRun) -> JsonValue {
    let runs = run
        .steps
        .iter()
        .map(|step| (step.step_id.as_str(), step))
        .collect::<BTreeMap<_, _>>();
    let mut result = JsonObject::new();
    for step_id in &run.graph.result_from {
        let Some(step) = runs
            .get(step_id.as_str())
            .filter(|step| step.outcome.succeeded())
        else {
            continue;
        };
        let Some(outputs) = step
            .ephemeral_contract
            .as_value()
            .and_then(JsonValue::as_object)
        else {
            continue;
        };
        for (name, value) in outputs {
            result.insert(name.clone(), value.clone());
        }
    }
    JsonValue::Object(result)
}

/// Preserve every declared semantic step output for the caller without
/// repeating transport stdout, parsed claims, stderr, or status diagnostics.
/// Typed invocation diagnostics remain on each step outcome and signed receipt.
pub(crate) fn graph_run_context(run: &GraphRun) -> JsonValue {
    let step_outputs = run
        .steps
        .iter()
        .filter_map(|step| {
            let outputs = declared_step_outputs(step);
            (!outputs.is_empty()).then(|| (step.step_id.clone(), JsonValue::Object(outputs)))
        })
        .collect::<JsonObject>();
    JsonValue::Object(JsonObject::from([(
        "step_outputs".to_owned(),
        JsonValue::Object(step_outputs),
    )]))
}

fn declared_step_outputs(step: &StepRun) -> JsonObject {
    step.contract.clone()
}

/// Compact graph provenance for the public skill-run envelope. Declared step
/// outputs remain in graph state and their claims are bound by signed receipts;
/// callers need stable step and receipt references, not another payload copy.
pub(crate) fn graph_run_trace(run: &GraphRun) -> JsonValue {
    let mut trace = JsonObject::new();
    trace.insert(
        "graph".to_owned(),
        JsonValue::String(run.graph.name.clone()),
    );
    trace.insert(
        "status".to_owned(),
        JsonValue::String(match run.receipt.seal.disposition {
            ClosureDisposition::Blocked => "blocked".to_owned(),
            ClosureDisposition::Deferred => "deferred".to_owned(),
            _ => format!("{:?}", run.state.status).to_ascii_lowercase(),
        }),
    );
    let mut step_summaries = Vec::new();
    for step in &run.steps {
        let mut summary = JsonObject::new();
        summary.insert(
            "step_id".to_owned(),
            JsonValue::String(step.step_id.clone()),
        );
        summary.insert("skill".to_owned(), JsonValue::String(step.skill.clone()));
        summary.insert(
            "status".to_owned(),
            JsonValue::String(if step.outcome.succeeded() {
                "success".to_owned()
            } else {
                "failure".to_owned()
            }),
        );
        summary.insert(
            "receipt_id".to_owned(),
            JsonValue::String(step.receipt.id.to_string()),
        );
        step_summaries.push(JsonValue::Object(summary));
    }
    trace.insert("steps".to_owned(), JsonValue::Array(step_summaries));
    JsonValue::Object(trace)
}

pub(crate) fn graph_run_skill_output(
    result: &JsonValue,
    run: &GraphRun,
) -> Result<InvocationOutput, RuntimeError> {
    let mut output = if run.state.status == GraphStatus::Succeeded {
        InvocationOutput::runtime_success(result.clone(), 0, JsonObject::new())
    } else {
        InvocationOutput::runtime_failure(
            result.clone(),
            format!("graph {} did not succeed", run.graph.name),
            0,
            JsonObject::new(),
        )
    };
    output.set_ephemeral(graph_run_ephemeral_result(run));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{
        GraphCheckpoint, GraphRun, RuntimeOptions, StepRun, graph_run_context,
        graph_run_ephemeral_result, graph_run_result,
    };
    use crate::adapter::{EphemeralValue, InvocationOutput};
    use crate::journal::ExecutionJournal;
    use crate::receipts::{
        RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
        RUNX_RECEIPT_SIGN_KID_ENV, graph_receipt, step_receipt,
    };
    use runx_contracts::{JsonObject, JsonValue};
    use runx_core::state_machine::{GraphStatus, SequentialGraphState, StepAdmissionWitness};
    use runx_parser::{ExecutionGraph, RawGraphIr};
    use std::collections::BTreeMap;

    const TEST_CREATED_AT: &str = "2026-07-24T00:00:00Z";

    #[test]
    fn runtime_options_reject_incomplete_production_signing_env() -> Result<(), String> {
        let env = [(RUNX_RECEIPT_SIGN_KID_ENV.to_owned(), "kid_prod".to_owned())]
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        let error = RuntimeOptions::from_env(env)
            .err()
            .ok_or_else(|| "incomplete signing env unexpectedly succeeded".to_owned())?;
        assert!(
            error
                .to_string()
                .contains("production receipt signing requires")
        );
        Ok(())
    }

    #[test]
    fn runtime_options_reject_missing_production_signing_env() -> Result<(), String> {
        let error = RuntimeOptions::from_env(BTreeMap::new())
            .err()
            .ok_or_else(|| "missing signing env unexpectedly succeeded".to_owned())?;
        assert!(
            error
                .to_string()
                .contains("governed runtime receipt signing")
        );
        Ok(())
    }

    #[test]
    fn runtime_options_reject_malformed_production_signing_seed() -> Result<(), String> {
        let env = [
            (RUNX_RECEIPT_SIGN_KID_ENV.to_owned(), "kid_prod".to_owned()),
            (
                RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV.to_owned(),
                "not-base64".to_owned(),
            ),
            (
                RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV.to_owned(),
                "hosted".to_owned(),
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let error = RuntimeOptions::from_env(env)
            .err()
            .ok_or_else(|| "malformed signing env unexpectedly succeeded".to_owned())?;
        assert!(
            error
                .to_string()
                .contains("production receipt signer key material is malformed")
        );
        Ok(())
    }

    #[test]
    fn runtime_options_strip_receipt_signing_env_after_signer_construction() -> Result<(), String> {
        let env = [
            (RUNX_RECEIPT_SIGN_KID_ENV.to_owned(), "kid_prod".to_owned()),
            (
                RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV.to_owned(),
                "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=".to_owned(),
            ),
            (
                RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV.to_owned(),
                "hosted".to_owned(),
            ),
            ("RUNX_CWD".to_owned(), "/workspace".to_owned()),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let options = RuntimeOptions::from_env(env).map_err(|error| error.to_string())?;

        assert!(!options.env.contains_key(RUNX_RECEIPT_SIGN_KID_ENV));
        assert!(
            !options
                .env
                .contains_key(RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV)
        );
        assert!(!options.env.contains_key(RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV));
        assert_eq!(options.env.get("RUNX_CWD"), Some(&"/workspace".to_owned()));
        Ok(())
    }

    #[test]
    fn graph_result_preserves_declared_packet_and_context_preserves_prior_contracts()
    -> Result<(), Box<dyn std::error::Error>> {
        let packet = JsonValue::Object(JsonObject::from([
            (
                "schema".to_owned(),
                JsonValue::String("runx.delivery.v1".to_owned()),
            ),
            (
                "data".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "message".to_owned(),
                    JsonValue::String("delivered".to_owned()),
                )])),
            ),
        ]));
        let run = test_graph_run(
            vec!["deliver"],
            vec![
                test_step(
                    "graph-result",
                    "research",
                    JsonObject::from([(
                        "research_packet".to_owned(),
                        JsonValue::String("evidence".to_owned()),
                    )]),
                )?,
                test_step(
                    "graph-result",
                    "deliver",
                    JsonObject::from([("delivery_receipt".to_owned(), packet.clone())]),
                )?,
            ],
            GraphStatus::Succeeded,
        )?;

        let result = graph_run_result(&run)?;
        assert_eq!(
            result
                .as_object()
                .and_then(|value| value.get("delivery_receipt")),
            Some(&packet)
        );
        assert!(
            result
                .as_object()
                .is_some_and(|value| !value.contains_key("research_packet"))
        );

        let context = graph_run_context(&run);
        let step_outputs = context
            .as_object()
            .and_then(|value| value.get("step_outputs"))
            .and_then(JsonValue::as_object)
            .ok_or("missing step_outputs")?;
        assert!(
            step_outputs
                .get("research")
                .and_then(JsonValue::as_object)
                .is_some_and(|value| value.contains_key("research_packet"))
        );
        assert!(
            step_outputs
                .get("deliver")
                .and_then(JsonValue::as_object)
                .is_some_and(|value| value.get("delivery_receipt") == Some(&packet))
        );
        Ok(())
    }

    #[test]
    fn graph_result_accepts_one_executed_conditional_producer()
    -> Result<(), Box<dyn std::error::Error>> {
        let run = test_graph_run(
            vec!["skipped_branch", "selected_branch"],
            vec![test_step(
                "conditional-result",
                "selected_branch",
                JsonObject::from([(
                    "decision".to_owned(),
                    JsonValue::String("selected".to_owned()),
                )]),
            )?],
            GraphStatus::Succeeded,
        )?;

        assert_eq!(
            graph_run_result(&run)?
                .as_object()
                .and_then(|value| value.get("decision"))
                .and_then(JsonValue::as_str),
            Some("selected")
        );
        Ok(())
    }

    #[test]
    fn graph_result_rejects_duplicate_keys_from_successful_producers()
    -> Result<(), Box<dyn std::error::Error>> {
        let run = test_graph_run(
            vec!["first", "second"],
            vec![
                test_step(
                    "duplicate-result",
                    "first",
                    JsonObject::from([("result".to_owned(), JsonValue::Bool(true))]),
                )?,
                test_step(
                    "duplicate-result",
                    "second",
                    JsonObject::from([("result".to_owned(), JsonValue::Bool(false))]),
                )?,
            ],
            GraphStatus::Succeeded,
        )?;

        let error = graph_run_result(&run)
            .err()
            .ok_or("duplicate result keys unexpectedly succeeded")?;
        assert!(
            error
                .to_string()
                .contains("more than one successful result producer")
        );
        Ok(())
    }

    #[test]
    fn succeeded_graph_requires_a_contributing_result_producer()
    -> Result<(), Box<dyn std::error::Error>> {
        let run = test_graph_run(
            vec!["conditional_branch"],
            vec![test_step(
                "missing-result",
                "setup",
                JsonObject::from([("setup".to_owned(), JsonValue::Bool(true))]),
            )?],
            GraphStatus::Succeeded,
        )?;

        let error = graph_run_result(&run)
            .err()
            .ok_or("graph without a result unexpectedly succeeded")?;
        assert!(
            error
                .to_string()
                .contains("succeeded without running a declared result producer")
        );
        Ok(())
    }

    #[test]
    fn checkpoint_serializes_each_step_contract_once() -> Result<(), Box<dyn std::error::Error>> {
        let mut run = test_graph_run(
            vec!["result"],
            vec![test_step(
                "checkpoint-shape",
                "result",
                JsonObject::from([(
                    "result".to_owned(),
                    JsonValue::String("complete".to_owned()),
                )]),
            )?],
            GraphStatus::Succeeded,
        )?;
        const SENTINEL: &str = "auc_secret_capability";
        run.steps[0].ephemeral_contract = EphemeralValue::from_value(JsonValue::Object(
            JsonObject::from([("result".to_owned(), JsonValue::String(SENTINEL.to_owned()))]),
        ));
        assert!(!serde_json::to_string(&graph_run_context(&run))?.contains(SENTINEL));
        assert!(serde_json::to_string(&graph_run_ephemeral_result(&run))?.contains(SENTINEL));
        let checkpoint = GraphCheckpoint {
            graph_name: run.graph.name,
            state: run.state,
            steps: run.steps,
            sync_points: run.sync_points,
            journal: run.journal,
            suspended_step: None,
        };
        let serialized = serde_json::to_value(checkpoint)?;
        let step = serialized
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .and_then(|steps| steps.first())
            .and_then(serde_json::Value::as_object)
            .ok_or("missing serialized step")?;

        assert!(step.contains_key("contract"));
        assert!(!serialized.to_string().contains(SENTINEL));
        assert!(!step.contains_key("ephemeral_contract"));
        assert!(!step.contains_key("output"));
        assert!(!step.contains_key("outputs"));
        Ok(())
    }

    fn test_step(
        graph_name: &str,
        step_id: &str,
        contract: JsonObject,
    ) -> Result<StepRun, Box<dyn std::error::Error>> {
        let output = InvocationOutput::runtime_success(JsonValue::Null, 1, JsonObject::new());
        let receipt = step_receipt(graph_name, step_id, 1, &output, &contract, TEST_CREATED_AT)?;
        Ok(StepRun {
            step_id: step_id.to_owned(),
            attempt: 1,
            skill: step_id.to_owned(),
            runner: None,
            fanout_group: None,
            contract,
            ephemeral_contract: EphemeralValue::default(),
            outcome: output.into(),
            nested_receipts: Vec::new(),
            admission_witness: StepAdmissionWitness::local_runtime(step_id, receipt.id.as_str()),
            receipt,
        })
    }

    fn test_graph_run(
        result_from: Vec<&str>,
        mut steps: Vec<StepRun>,
        status: GraphStatus,
    ) -> Result<GraphRun, Box<dyn std::error::Error>> {
        let name = "test-graph".to_owned();
        let receipt = graph_receipt(&name, &mut steps, Vec::new(), TEST_CREATED_AT)?;
        Ok(GraphRun {
            graph: ExecutionGraph {
                name: name.clone(),
                owner: None,
                result_from: result_from.into_iter().map(str::to_owned).collect(),
                charter_from: None,
                steps: Vec::new(),
                fanout_groups: BTreeMap::new(),
                policy: None,
                raw: RawGraphIr {
                    document: JsonObject::new(),
                },
            },
            state: SequentialGraphState {
                graph_id: name,
                status,
                steps: Vec::new(),
            },
            steps,
            sync_points: Vec::new(),
            receipt,
            journal: ExecutionJournal::default(),
        })
    }
}
