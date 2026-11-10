mod bundle;
mod paged;
mod supervisor;

use std::sync::Arc;
use std::time::Instant;

use runx_contracts::javascript_worker::MAX_WORKER_POOL_SIZE;
use runx_contracts::{JsonObject, JsonValue};

use self::bundle::validated_module;
use self::supervisor::JavaScriptWorkerSupervisor;
use crate::RuntimeError;
use crate::adapter::{InvocationOutput, InvocationStatus, SkillAdapter, SkillInvocation};
use crate::adapter_pipeline::AdapterProjection;
use crate::observations::Observations;

const WORKER_PATH_ENV: &str = "RUNX_JS_WORKER_PATH";

/// One explicit deterministic-JavaScript session. Clones share a bounded lazy
/// worker pool so warm sequential work reuses one process while concurrent
/// branches receive independent wall-time kill boundaries. Independent
/// adapters never share failure or lifecycle state.
#[derive(Clone)]
pub struct JavaScriptAdapter {
    supervisor: Arc<JavaScriptWorkerSupervisor>,
    max_concurrency: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JavaScriptSessionStats {
    pub spawned_process_count: u64,
    pub peak_in_flight: usize,
}

pub(crate) struct PreparedJavaScriptInvocation {
    entry_module: String,
    export_name: String,
    modules: std::collections::BTreeMap<String, String>,
    environment: std::collections::BTreeMap<String, String>,
    worker_path: Option<String>,
    limits: runx_contracts::javascript_worker::InvocationLimits,
}

/// A script step's module bundle, prepared once and passed again after each
/// settlement.
pub(crate) struct PreparedScript {
    entry_module: String,
    export_name: String,
    modules: std::collections::BTreeMap<String, String>,
    worker_path: Option<String>,
    limits: runx_contracts::javascript_worker::InvocationLimits,
}

/// What a script pass or settlement came to: the calls made since the last,
/// in position order, and how the script stands.
pub(crate) struct ScriptPassed {
    pub(crate) calls: Vec<runx_contracts::javascript_worker::ScriptCall>,
    pub(crate) ending: runx_contracts::javascript_worker::ScriptEnding,
}

/// A script waiting on its calls, live on its worker.
pub(crate) struct LiveScript<'a> {
    script: supervisor::LiveScript<'a>,
}

impl LiveScript<'_> {
    /// Deliver further settlements, in the order they settled.
    pub(crate) fn settle(
        &mut self,
        settled: Vec<runx_contracts::javascript_worker::ScriptSettlement>,
        now: i64,
    ) -> Result<ScriptPassed, RuntimeError> {
        crate::work_counts::count_many(
            crate::work_counts::Work::ScriptSettlementDelivered,
            settled.len(),
        );
        script_passed(self.script.settle(settled, now)?)
    }
}

fn script_passed(result: supervisor::WorkerInvocationResult) -> Result<ScriptPassed, RuntimeError> {
    match result {
        supervisor::WorkerInvocationResult::Passed { calls, ending } => {
            Ok(ScriptPassed { calls, ending })
        }
        supervisor::WorkerInvocationResult::Failure { message, .. } => {
            Err(RuntimeError::JavaScriptWorker { message })
        }
        supervisor::WorkerInvocationResult::Success(_) => Err(RuntimeError::JavaScriptWorker {
            message: "the worker answered a script with a tool's result".to_owned(),
        }),
    }
}

impl PreparedJavaScriptInvocation {
    fn with_inputs(
        &self,
        inputs: &JsonObject,
    ) -> Result<supervisor::WorkerInvocation, RuntimeError> {
        let inputs = serde_json::to_value(inputs)
            .map_err(|source| RuntimeError::json("serializing JavaScript inputs", source))?;
        Ok(supervisor::WorkerInvocation {
            entry_module: self.entry_module.clone(),
            export_name: self.export_name.clone(),
            modules: self.modules.clone(),
            inputs,
            environment: self.environment.clone(),
            worker_path: self.worker_path.clone(),
            limits: self.limits,
        })
    }
}

impl JavaScriptAdapter {
    #[must_use]
    pub fn new_session() -> Self {
        Self::with_max_concurrency(1)
    }

    #[must_use]
    pub fn with_max_concurrency(max_concurrency: usize) -> Self {
        let max_concurrency = max_concurrency.clamp(1, MAX_WORKER_POOL_SIZE);
        Self {
            supervisor: Arc::new(JavaScriptWorkerSupervisor::new(max_concurrency)),
            max_concurrency,
        }
    }

    /// The digest of the sources the worker this session runs `env`'s
    /// JavaScript on reports it was built from: an idle worker's, or one
    /// started now, which stays in the session for the next call to it.
    pub(crate) fn worker_digest(
        &self,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<String, RuntimeError> {
        self.supervisor
            .worker_digest(env.get(WORKER_PATH_ENV).map(String::as_str))
    }

    #[must_use]
    pub fn spawned_process_count(&self) -> u64 {
        self.supervisor.spawn_count()
    }

    #[must_use]
    pub fn session_stats(&self) -> JavaScriptSessionStats {
        JavaScriptSessionStats {
            spawned_process_count: self.supervisor.spawn_count(),
            peak_in_flight: self.supervisor.peak_in_flight(),
        }
    }

    /// Prepare `request` to run from `package`, the validated package its
    /// run admitted: its modules are that package's, never reread.
    pub(crate) fn prepare_invocation(
        &self,
        request: &SkillInvocation,
        package: &crate::LoadedSkillPackage,
    ) -> Result<PreparedJavaScriptInvocation, RuntimeError> {
        validate_pure_javascript_boundary(request)?;
        validated_module(request, package)
    }

    pub(crate) fn invoke_prepared(
        &self,
        prepared: &PreparedJavaScriptInvocation,
        inputs: &JsonObject,
    ) -> Result<InvocationOutput, RuntimeError> {
        let started = Instant::now();
        let outcome = self.supervisor.invoke(prepared.with_inputs(inputs)?)?;
        project_worker_outcome(started, outcome)
    }

    /// Prepare script step `script` to run from `package`, the validated
    /// package its run admitted.
    pub(crate) fn prepare_script(
        &self,
        script: &runx_parser::ScriptStep,
        package: &crate::LoadedSkillPackage,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<PreparedScript, RuntimeError> {
        bundle::validated_script(script, package, env.get(WORKER_PATH_ENV).cloned())
    }

    /// Run `prepared` from the beginning against what its calls `settled`,
    /// in the order they settled; a `ctx.now()` past the record reads `now`.
    /// The script comes back live, to take each further settlement where it
    /// stands rather than from the beginning.
    pub(crate) fn start_script(
        &self,
        prepared: &PreparedScript,
        inputs: &JsonObject,
        settled: Vec<runx_contracts::javascript_worker::ScriptSettlement>,
        now: i64,
    ) -> Result<(LiveScript<'_>, ScriptPassed), RuntimeError> {
        let inputs = serde_json::to_value(inputs)
            .map_err(|source| RuntimeError::json("serializing a script's inputs", source))?;
        crate::work_counts::count(crate::work_counts::Work::ScriptPass);
        crate::work_counts::count_many(
            crate::work_counts::Work::ScriptSettlementDelivered,
            settled.len(),
        );
        let (script, result) = self
            .supervisor
            .start_script(supervisor::ScriptPassInvocation {
                entry_module: prepared.entry_module.clone(),
                export_name: prepared.export_name.clone(),
                modules: prepared.modules.clone(),
                inputs,
                worker_path: prepared.worker_path.clone(),
                limits: prepared.limits,
                settled,
                now,
            })?;
        Ok((LiveScript { script }, script_passed(result)?))
    }

    /// Run `request` from `package`, the validated package its run admitted.
    pub(crate) fn invoke_with_artifacts(
        &self,
        request: SkillInvocation,
        package: &crate::LoadedSkillPackage,
        local_artifacts: &crate::services::LocalArtifactService,
    ) -> Result<InvocationOutput, RuntimeError> {
        if worker(&request)?.paged_input.is_some() {
            return paged::invoke(self, request, package, local_artifacts);
        }
        let prepared = self.prepare_invocation(&request, package)?;
        self.invoke_prepared(&prepared, &request.inputs)
    }
}

impl Default for JavaScriptAdapter {
    fn default() -> Self {
        Self::new_session()
    }
}

impl std::fmt::Debug for JavaScriptAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JavaScriptAdapter")
            .field("max_concurrency", &self.max_concurrency)
            .field("spawned_process_count", &self.spawned_process_count())
            .finish_non_exhaustive()
    }
}

/// The adapter alone, outside any run, runs the package as it is now.
impl SkillAdapter for JavaScriptAdapter {
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        validate_pure_javascript_boundary(&request)?;
        let package = crate::load_validated_skill_package(&request.skill_directory)?;
        self.invoke_with_artifacts(
            request,
            &package,
            &crate::services::LocalArtifactService::default(),
        )
    }
}

fn project_worker_outcome(
    started: Instant,
    outcome: supervisor::WorkerInvocationOutcome,
) -> Result<InvocationOutput, RuntimeError> {
    match outcome.result {
        supervisor::WorkerInvocationResult::Success(output) => {
            let value = serde_json::from_value(output).map_err(|source| {
                RuntimeError::json("converting JavaScript worker output", source)
            })?;
            Ok(
                AdapterProjection::from_duration_ms(elapsed_ms(started)).runtime_output(
                    InvocationStatus::Success,
                    value,
                    None,
                    worker_observations(outcome.execution_boundary),
                ),
            )
        }
        supervisor::WorkerInvocationResult::Failure { message, .. } => Ok(
            AdapterProjection::from_duration_ms(elapsed_ms(started)).runtime_output(
                InvocationStatus::Failure,
                JsonValue::Null,
                Some(message),
                worker_observations(outcome.execution_boundary),
            ),
        ),
        supervisor::WorkerInvocationResult::Passed { .. } => Err(RuntimeError::JavaScriptWorker {
            message: "the worker answered a tool with a script pass".to_owned(),
        }),
    }
}

/// The worker tool an invocation runs.
pub(super) fn worker(request: &SkillInvocation) -> Result<&runx_parser::WorkerTool, RuntimeError> {
    request
        .executable
        .worker()
        .ok_or_else(|| RuntimeError::UnsupportedExecutable {
            kind: request.executable.kind().to_owned(),
        })
}

fn validate_pure_javascript_boundary(request: &SkillInvocation) -> Result<(), RuntimeError> {
    worker(request)?;
    if !request.credential_delivery.secret_env().is_empty() {
        return Err(RuntimeError::InvalidProcessInvocation {
            message: "worker tools cannot receive credentials; route provider access through a connector operation"
                .to_owned(),
        });
    }
    Ok(())
}

/// Where a worker invocation ran.
fn worker_observations(boundary: runx_contracts::ExecutionBoundaryKind) -> Observations {
    Observations {
        boundary: Some(boundary),
        ..Observations::default()
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
