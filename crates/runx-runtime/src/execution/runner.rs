// Module rationale: RuntimeOptions, checkpoint resume, and
// the public graph runner surface are audited as one unit.
//! The act engine for runx: the single admit -> execute -> seal path every run
//! takes. A standalone skill is a one-act plan and a graph a multi-act plan;
//! both run through this one engine.
//!
//! The public surface lives here: [`Runtime`], [`RuntimeOptions`], [`StepRun`]
//! and [`GraphRun`]. Every run records into its own stream
//! ([`super::run_record`]) and continues from it. The internal state machine
//! and the per-step execution helpers live in private submodules.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{ClosureDisposition, JsonObject, JsonValue, Receipt};
use runx_core::state_machine::{GraphStatus, SequentialGraphState, StepAdmissionWitness};
use runx_parser::ExecutionGraph;
use serde::{Deserialize, Serialize};

use super::graph::load_graph;
use super::run_record::{RunLog, RunRecord};
use crate::RuntimeError;
use crate::adapter::{
    EphemeralValue, InvocationDiagnostics, InvocationOutput, InvocationStatus, SkillAdapter,
};
use crate::effects::RuntimeEffectRegistry;
use crate::host::{Host, NoopHost};
use crate::lifecycle::LifecycleEvent;
use crate::receipts::signing::strip_receipt_signing_env;
use crate::receipts::{
    GraphClosure, RunScope, RuntimeReceiptSignatureConfig, RuntimeReceiptSignaturePolicy,
    graph_receipt_with_closure,
};
use crate::services::ReceiptServices;

mod admission;
mod dispatch;
mod graph_engine;
mod step_handlers;
pub mod turn;

use graph_engine::GraphExecution;

pub const RUNX_RUN_ID_ENV: &str = "RUNX_RUN_ID";

/// Why the run's delegation refuses a child run one level below this run.
pub(crate) fn child_depth_refusal(options: &RuntimeOptions) -> Option<String> {
    let limit = options.run_authority.as_ref()?.child_limits().depth?;
    (options.run.depth() + 1 > limit)
        .then(|| format!("the run's delegation lets child runs nest at most {limit} deep"))
}

#[derive(Clone, Debug)]
pub struct RuntimeOptions {
    pub current_context: Vec<runx_contracts::ContextEntry>,
    pub historical_context: Vec<runx_contracts::ContextEntry>,
    pub run_authority: Option<crate::RunAuthority>,
    pub created_at: String,
    pub env: BTreeMap<String, String>,
    pub receipt_signature: RuntimeReceiptSignatureConfig,
    pub effects: RuntimeEffectRegistry,
    /// The run this runtime executes, and the stream it records into.
    pub run: RunLog,
}

impl RuntimeOptions {
    #[must_use]
    pub fn local_development(env: BTreeMap<String, String>, run: RunLog) -> Self {
        Self::from_env_and_signature(env, RuntimeReceiptSignatureConfig::local_development(), run)
    }

    pub fn from_env(env: BTreeMap<String, String>, run: RunLog) -> Result<Self, RuntimeError> {
        let receipt_services =
            ReceiptServices::from_env(&env).map_err(|error| RuntimeError::ReceiptInvalid {
                message: error.to_string(),
            })?;
        Ok(Self::from_env_and_signature(
            env,
            receipt_services.signature_config().clone(),
            run,
        ))
    }

    pub fn from_env_or_local_development(
        env: BTreeMap<String, String>,
        run: RunLog,
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
            run,
        ))
    }

    fn from_env_and_signature(
        mut env: BTreeMap<String, String>,
        receipt_signature: RuntimeReceiptSignatureConfig,
        run: RunLog,
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
            run,
        }
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

#[derive(Clone, Debug)]
pub struct StepRun {
    pub step_id: String,
    pub attempt: u32,
    pub skill: String,
    pub runner: Option<String>,
    /// The one semantic, addressable output surface retained by graph state.
    pub contract: JsonObject,
    /// Immediate caller-only overlay. It exists only in the live process and is
    /// deliberately absent from the run record, receipts, and graph context.
    #[doc(hidden)]
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
    pub receipt: Receipt,
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
    /// A nested graph's provenance from its parent step, by step id.
    inherited_provenance: BTreeMap<String, Vec<runx_contracts::ProvenanceEntry>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GraphTerminalOutcome {
    Propagate,
    SealReceipt,
}

/// A step closure that stops the graph as an outcome rather than a fault: an
/// agent act that declined, deferred, blocked, or was superseded.
pub(crate) const fn is_governed_step_closure(disposition: &ClosureDisposition) -> bool {
    matches!(
        disposition,
        ClosureDisposition::Declined
            | ClosureDisposition::Deferred
            | ClosureDisposition::Superseded
            | ClosureDisposition::Blocked
    )
}

/// A graph's public result. A graph that a step's own governed closure
/// stopped returns that step's declared outputs; any other graph returns its
/// result producers' outputs.
pub(crate) fn graph_terminal_result(run: &GraphRun) -> Result<JsonValue, RuntimeError> {
    let disposition = &run.receipt.seal.disposition;
    let closing_step = run.steps.last().filter(|step| {
        is_governed_step_closure(disposition)
            && !step.outcome.succeeded()
            && step.receipt.seal.disposition == *disposition
    });
    match closing_step {
        Some(step) => Ok(JsonValue::Object(step.contract.clone())),
        None => graph_run_result(run),
    }
}

/// The graph step a sealable fault belongs to: its own step id, or the step
/// a failed step run names.
pub(crate) fn failed_step_id(graph: &ExecutionGraph, error: &RuntimeError) -> Option<String> {
    error.graph_step_id().map(str::to_owned).or_else(|| {
        let RuntimeError::SkillFailed { skill_name, .. } = error else {
            return None;
        };
        graph
            .steps
            .iter()
            .any(|step| step.id == *skill_name)
            .then(|| skill_name.clone())
    })
}

/// The closure a governed stop seals: a policy block, an authority denial, a
/// pending resolution, or a step's own non-`closed` closure. Any other error
/// is a fault, not a stop.
pub(crate) fn graph_stop_closure(
    graph_name: &str,
    error: &RuntimeError,
) -> Option<(String, crate::receipts::GraphClosure)> {
    let (step_id, disposition, reason_code, summary) = match error {
        RuntimeError::GraphBlocked { step_id, reason } => (
            step_id,
            ClosureDisposition::Blocked,
            "graph_blocked".to_owned(),
            format!("graph {graph_name} blocked at {step_id}: {reason}"),
        ),
        RuntimeError::AuthorityDenied {
            verb,
            step_id,
            reason,
        } => (
            step_id,
            ClosureDisposition::Blocked,
            "authority_denied".to_owned(),
            format!("graph {graph_name} denied {verb:?} at {step_id}: {reason}"),
        ),
        RuntimeError::ResolutionPending {
            step_id, reason, ..
        } => (
            step_id,
            ClosureDisposition::Deferred,
            "resolution_pending".to_owned(),
            format!("graph {graph_name} deferred at {step_id}: {reason}"),
        ),
        RuntimeError::StepClosed {
            step_id,
            disposition,
            reason_code,
            summary,
            ..
        } => (
            step_id,
            disposition.clone(),
            reason_code.clone(),
            format!(
                "graph {graph_name} {} at {step_id}: {summary}",
                disposition.label()
            ),
        ),
        _ => return None,
    };
    Some((
        step_id.clone(),
        crate::receipts::GraphClosure {
            disposition,
            reason_code,
            summary,
        },
    ))
}

impl<A> Runtime<A>
where
    A: SkillAdapter,
{
    pub fn new(adapter: A, options: RuntimeOptions) -> Self {
        Self {
            configured_adapter: adapter,
            javascript: crate::adapters::javascript::JavaScriptAdapter::new_session(),
            local_artifacts: crate::services::LocalArtifactService::default(),
            options: Arc::new(options),
            inherited_provenance: BTreeMap::new(),
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
            inherited_provenance: BTreeMap::new(),
        }
    }

    /// Carry the parent step's provenance into this nested runtime's steps.
    pub(crate) fn with_inherited_provenance(
        mut self,
        inherited: BTreeMap<String, Vec<runx_contracts::ProvenanceEntry>>,
    ) -> Self {
        self.inherited_provenance = inherited;
        self
    }

    pub(crate) fn inherited_provenance(&self, step_id: &str) -> &[runx_contracts::ProvenanceEntry] {
        self.inherited_provenance
            .get(step_id)
            .map_or(&[], Vec::as_slice)
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

    /// Run, or continue, this runtime's run of the graph at `graph_path`.
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

    /// Run, or continue, this runtime's run of `graph`. A run with recorded
    /// steps continues from its record: completed steps are not run again, and
    /// a suspended step resumes its attempt.
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

    fn resolved_graph_path(&self, graph_path: &Path) -> Result<PathBuf, RuntimeError> {
        if graph_path.is_absolute() {
            return Ok(crate::path_util::lexical_normalize(graph_path));
        }
        let workspace = self
            .options
            .env
            .get(crate::paths::RUNX_CWD_ENV)
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
        let mut execution = self.open_execution(&graph)?;
        match execution.run(self, graph_dir, &graph, host) {
            Ok(()) => {
                let receipt = graph_receipt_with_closure(
                    self.scope(&graph.name),
                    &execution.runs,
                    &self.options.created_at,
                    GraphClosure::completed(&graph.name),
                    self.options.signature_policy(),
                )?;
                execution.record_lifecycle(
                    host,
                    LifecycleEvent::graph_completed(&graph.name, &receipt),
                )?;
                Ok(execution.finish(graph, receipt))
            }
            Err(error) if terminal_outcome == GraphTerminalOutcome::SealReceipt => {
                let Some((step_id, closure)) = graph_stop_closure(&graph.name, &error) else {
                    return self.seal_failed_graph_in_place(execution, graph, error, host);
                };
                let receipt = graph_receipt_with_closure(
                    self.scope(&graph.name),
                    &execution.runs,
                    &self.options.created_at,
                    closure,
                    self.options.signature_policy(),
                )?;
                execution.record_lifecycle(
                    host,
                    LifecycleEvent::graph_stopped(&graph.name, &step_id, &receipt),
                )?;
                Ok(execution.finish(graph, receipt))
            }
            Err(error) => Err(error),
        }
    }

    /// The run's execution state, replayed from its record. Its owner starts
    /// the run before the engine runs it.
    fn open_execution(&self, graph: &ExecutionGraph) -> Result<GraphExecution, RuntimeError> {
        match self.options.run.load()? {
            None => Err(RuntimeError::EngineInvariant {
                context: "running a graph",
                message: format!("run {} was not started", self.options.run.run_id()),
            }),
            Some(record) => self.replay_execution(graph, &record),
        }
    }

    fn replay_execution(
        &self,
        graph: &ExecutionGraph,
        record: &RunRecord,
    ) -> Result<GraphExecution, RuntimeError> {
        if record.sealed.is_some() {
            return Err(RuntimeError::EngineInvariant {
                context: "continuing a run",
                message: format!("run {} is already sealed", self.options.run.run_id()),
            });
        }
        GraphExecution::from_record(graph, record)
    }

    /// Seal a graph whose step faulted, in place: the same failed closure
    /// a stopped run seals from its record. A fault with no step to own it is
    /// returned unsealed.
    fn seal_failed_graph_in_place(
        &self,
        mut execution: GraphExecution,
        graph: ExecutionGraph,
        error: RuntimeError,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        let Some(step_id) = (!error.is_fatal_step_fault())
            .then(|| failed_step_id(&graph, &error))
            .flatten()
        else {
            return Err(error);
        };
        let summary = format!("graph {} failed at {step_id}: {error}", graph.name);
        let fault_run = if execution.has_run(&step_id) {
            None
        } else {
            let step = graph
                .steps
                .iter()
                .find(|step| step.id == step_id)
                .ok_or_else(|| RuntimeError::StepMissing {
                    step_id: step_id.clone(),
                })?;
            let attempt = execution.attempt(&step_id);
            Some(step_handlers::runtime_error_step_run(
                self,
                &graph.name,
                step,
                attempt,
                error,
            )?)
        };
        execution.fail_graph_at(self, host, &step_id, fault_run)?;
        let receipt = graph_receipt_with_closure(
            self.scope(&graph.name),
            &execution.runs,
            &self.options.created_at,
            crate::receipts::GraphClosure {
                disposition: ClosureDisposition::Failed,
                reason_code: "graph_step_failed".to_owned(),
                summary,
            },
            self.options.signature_policy(),
        )?;
        execution.record_lifecycle(
            host,
            LifecycleEvent::graph_stopped(&graph.name, &step_id, &receipt),
        )?;
        Ok(execution.finish(graph, receipt))
    }

    /// Seal a recorded run that stopped at `step_id` short of completion:
    /// blocked, deferred, or closed by a step's governed closure.
    pub(crate) fn seal_stopped_with_host(
        &self,
        graph: ExecutionGraph,
        step_id: &str,
        closure: crate::receipts::GraphClosure,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        let mut execution = self.open_execution(&graph)?;
        let receipt = graph_receipt_with_closure(
            self.scope(&graph.name),
            &execution.runs,
            &self.options.created_at,
            closure,
            self.options.signature_policy(),
        )?;
        execution.record_lifecycle(
            host,
            LifecycleEvent::graph_stopped(&graph.name, step_id, &receipt),
        )?;
        Ok(execution.finish(graph, receipt))
    }

    /// Seal a recorded run whose step at `step_id` failed: the failure is
    /// recorded as that step's run, then the run is sealed failed.
    pub(crate) fn seal_failed_with_host(
        &self,
        graph: ExecutionGraph,
        step_id: &str,
        error: RuntimeError,
        closure: crate::receipts::GraphClosure,
        host: &mut dyn Host,
    ) -> Result<GraphRun, RuntimeError> {
        if closure.disposition != ClosureDisposition::Failed {
            return Err(RuntimeError::ReceiptInvalid {
                message: "a failed run requires a failed closure".to_owned(),
            });
        }
        let mut execution = self.open_execution(&graph)?;
        let step = graph
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .ok_or_else(|| RuntimeError::StepMissing {
                step_id: step_id.to_owned(),
            })?;
        let attempt = execution.failure_attempt(step_id);
        let failed_run =
            step_handlers::runtime_error_step_run(self, &graph.name, step, attempt, error)?;
        execution.record_terminal_step_failure(self, host, step_id, failed_run)?;
        let receipt = graph_receipt_with_closure(
            self.scope(&graph.name),
            &execution.runs,
            &self.options.created_at,
            closure,
            self.options.signature_policy(),
        )?;
        execution.record_lifecycle(
            host,
            LifecycleEvent::graph_stopped(&graph.name, step_id, &receipt),
        )?;
        Ok(execution.finish(graph, receipt))
    }
}

impl<A: Clone> Runtime<A> {
    /// A runtime for another run that shares this one's adapter and native
    /// services, as a long-lived host shares a warm worker session across
    /// the runs it drives.
    #[must_use]
    pub fn for_run(&self, run: RunLog) -> Self {
        let mut options = self.options.as_ref().clone();
        options.run = run;
        Self {
            configured_adapter: self.configured_adapter.clone(),
            javascript: self.javascript.clone(),
            local_artifacts: self.local_artifacts.clone(),
            options: Arc::new(options),
            inherited_provenance: BTreeMap::new(),
        }
    }
}

impl<A> Runtime<A> {
    /// The run this runtime's receipts name, for the graph it runs.
    pub(crate) fn scope<'a>(&'a self, graph_name: &'a str) -> RunScope<'a> {
        RunScope {
            run_id: self.options.run.run_id(),
            graph_name,
        }
    }
}

/// The run a sealed record holds: its recorded steps and its graph receipt.
pub(crate) fn sealed_graph_run(
    graph: ExecutionGraph,
    record: RunRecord,
) -> Result<GraphRun, RuntimeError> {
    let sealed = record
        .sealed
        .clone()
        .ok_or_else(|| RuntimeError::EngineInvariant {
            context: "restoring a sealed run",
            message: "the run has no seal".to_owned(),
        })?;
    let mut execution = GraphExecution::from_record(&graph, &record)?;
    execution.seal_state(&sealed.disposition);
    Ok(execution.finish(graph, sealed.receipt))
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
        InvocationOutput::runtime_failure(result.clone(), graph_failure(run), 0, JsonObject::new())
    };
    output.set_ephemeral(graph_run_ephemeral_result(run));
    Ok(output)
}

/// Why a graph did not succeed: the first step that failed, and its error.
pub(crate) fn graph_failure(run: &GraphRun) -> String {
    let failed = run.state.steps.iter().find_map(|step| {
        (step.status == runx_core::state_machine::GraphStepStatus::Failed)
            .then_some((step.step_id.as_str(), step.error.as_deref()))
    });
    match failed {
        Some((step_id, Some(error))) => {
            format!(
                "graph {} did not succeed: step {step_id}: {error}",
                run.graph.name
            )
        }
        Some((step_id, None)) => {
            format!(
                "graph {} did not succeed: step {step_id} failed",
                run.graph.name
            )
        }
        None => format!("graph {} did not succeed", run.graph.name),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GraphRun, RuntimeOptions, StepRun, graph_run_context, graph_run_ephemeral_result,
        graph_run_result,
    };
    use crate::adapter::{EphemeralValue, InvocationOutput};
    use crate::execution::run_record::scratch_log;
    use crate::receipts::{
        RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
        RUNX_RECEIPT_SIGN_KID_ENV, RunScope, graph_receipt, step_receipt,
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

        let (_root, run) = scratch_log("run_options");
        let error = RuntimeOptions::from_env(env, run)
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
        let (_root, run) = scratch_log("run_options");
        let error = RuntimeOptions::from_env(BTreeMap::new(), run)
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

        let (_root, run) = scratch_log("run_options");
        let error = RuntimeOptions::from_env(env, run)
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

        let (_root, run) = scratch_log("run_options");
        let options = RuntimeOptions::from_env(env, run).map_err(|error| error.to_string())?;

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
    fn the_run_record_holds_each_step_contract_and_never_its_ephemeral_overlay()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut run = test_graph_run(
            vec!["result"],
            vec![test_step(
                "record-shape",
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

        let (root, log) = scratch_log("run_record_shape");
        log.record_step(&run.steps[0], &[])?;
        let record = log.load()?.ok_or("the run has a record")?;
        let crate::execution::run_record::Replayed::Step(step) = &record.facts[0] else {
            return Err("the first fact is the step".into());
        };
        assert_eq!(step.contract, run.steps[0].contract);
        for file in std::fs::read_dir(root.path())? {
            let bytes = std::fs::read(file?.path())?;
            assert!(
                !bytes
                    .windows(SENTINEL.len())
                    .any(|window| window == SENTINEL.as_bytes()),
                "the ephemeral overlay reached the kernel store"
            );
        }
        Ok(())
    }

    #[test]
    fn a_recorded_run_continues_without_running_its_recorded_steps_again()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::adapters::process_tool::ProcessToolAdapter;
        let graph_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/hello-graph/graph.yaml")
            .canonicalize()?;
        let (_first_root, first_log) = scratch_log("run_hello_first");
        let first = super::Runtime::new(
            ProcessToolAdapter,
            RuntimeOptions::local_development(std::env::vars().collect(), first_log),
        )
        .run_graph_file(&graph_path)?;
        assert_eq!(first.state.status, GraphStatus::Succeeded);

        // A second run whose record already holds the first step, as a
        // process that stopped after it would have left it.
        let (_root, log) = scratch_log("run_hello_continued");
        log.record_step(&first.steps[0], &[])?;
        let continued = super::Runtime::new(
            ProcessToolAdapter,
            RuntimeOptions::local_development(std::env::vars().collect(), log),
        )
        .run_graph_file(&graph_path)?;
        assert_eq!(continued.state.status, GraphStatus::Succeeded);
        assert_eq!(
            continued
                .steps
                .iter()
                .map(|step| step.step_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(continued.steps[0].receipt.id, first.steps[0].receipt.id);
        Ok(())
    }

    fn test_step(
        graph_name: &str,
        step_id: &str,
        contract: JsonObject,
    ) -> Result<StepRun, Box<dyn std::error::Error>> {
        let output = InvocationOutput::runtime_success(JsonValue::Null, 1, JsonObject::new());
        let receipt = step_receipt(
            RunScope {
                run_id: "run_test",
                graph_name,
            },
            step_id,
            1,
            &output,
            &contract,
            TEST_CREATED_AT,
        )?;
        Ok(StepRun {
            step_id: step_id.to_owned(),
            attempt: 1,
            skill: step_id.to_owned(),
            runner: None,
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
        steps: Vec<StepRun>,
        status: GraphStatus,
    ) -> Result<GraphRun, Box<dyn std::error::Error>> {
        let name = "test-graph".to_owned();
        let receipt = graph_receipt(
            RunScope {
                run_id: "run_test",
                graph_name: &name,
            },
            &steps,
            TEST_CREATED_AT,
        )?;
        Ok(GraphRun {
            graph: ExecutionGraph {
                name: name.clone(),
                owner: None,
                result_from: result_from.into_iter().map(str::to_owned).collect(),
                policy: None,
                steps: Vec::new(),
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
            receipt,
        })
    }
}
