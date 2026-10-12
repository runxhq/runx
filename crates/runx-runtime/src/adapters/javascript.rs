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

    #[must_use]
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
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

    pub(crate) fn prepare_invocation(
        &self,
        request: &SkillInvocation,
    ) -> Result<PreparedJavaScriptInvocation, RuntimeError> {
        validate_pure_javascript_boundary(request)?;
        validated_module(request)
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

    pub(crate) fn invoke_with_artifacts(
        &self,
        request: SkillInvocation,
        local_artifacts: &crate::services::LocalArtifactService,
    ) -> Result<InvocationOutput, RuntimeError> {
        if worker(&request)?.pages.is_some() {
            return paged::invoke(self, request, local_artifacts);
        }
        self.invoke_once(request)
    }

    fn invoke_once(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        let prepared = self.prepare_invocation(&request)?;
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

impl SkillAdapter for JavaScriptAdapter {
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        self.invoke_with_artifacts(request, &crate::services::LocalArtifactService::default())
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
