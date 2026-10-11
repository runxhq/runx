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
        project_worker_outcome(started, outcome, prepared.limits)
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
    limits: runx_contracts::javascript_worker::InvocationLimits,
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
                    worker_observations(outcome.execution_boundary, limits, None),
                ),
            )
        }
        supervisor::WorkerInvocationResult::Failure { limit, message, .. } => Ok(
            AdapterProjection::from_duration_ms(elapsed_ms(started)).runtime_output(
                InvocationStatus::Failure,
                JsonValue::Null,
                Some(message),
                worker_observations(outcome.execution_boundary, limits, limit),
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

/// Where a worker invocation ran and the ceilings that bounded it.
fn worker_observations(
    boundary: runx_contracts::ExecutionBoundaryKind,
    limits: runx_contracts::javascript_worker::InvocationLimits,
    hit: Option<runx_contracts::javascript_worker::WorkerLimit>,
) -> Observations {
    Observations {
        boundary: Some(boundary),
        limits: Some(javascript_execution_limits(limits, hit)),
        ..Observations::default()
    }
}

fn javascript_execution_limits(
    limits: runx_contracts::javascript_worker::InvocationLimits,
    hit: Option<runx_contracts::javascript_worker::WorkerLimit>,
) -> runx_contracts::ExecutionLimits {
    use runx_contracts::javascript_worker::{InvocationLimits, WorkerLimit};
    use runx_contracts::{ExecutionLimitHit, ExecutionLimitUnit, ExecutionLimits};

    let ceiling = InvocationLimits::default();
    let configured = std::collections::BTreeMap::from([
        (
            "javascript.source_bytes".to_owned(),
            execution_limit(
                usize_as_u64(limits.source_bytes),
                usize_as_u64(ceiling.source_bytes),
                ExecutionLimitUnit::Bytes,
                None,
            ),
        ),
        (
            "javascript.input_bytes".to_owned(),
            execution_limit(
                usize_as_u64(limits.input_bytes),
                usize_as_u64(ceiling.input_bytes),
                ExecutionLimitUnit::Bytes,
                None,
            ),
        ),
        (
            "javascript.output_bytes".to_owned(),
            execution_limit(
                usize_as_u64(limits.output_bytes),
                usize_as_u64(ceiling.output_bytes),
                ExecutionLimitUnit::Bytes,
                None,
            ),
        ),
        (
            "javascript.heap_bytes".to_owned(),
            execution_limit(
                limits.heap_bytes,
                ceiling.heap_bytes,
                ExecutionLimitUnit::Bytes,
                None,
            ),
        ),
        (
            "javascript.stack_bytes".to_owned(),
            execution_limit(
                usize_as_u64(limits.stack_bytes),
                usize_as_u64(ceiling.stack_bytes),
                ExecutionLimitUnit::Bytes,
                None,
            ),
        ),
        (
            "javascript.wall_milliseconds".to_owned(),
            execution_limit(
                limits.wall_milliseconds,
                runx_contracts::javascript_worker::MAX_WALL_MILLISECONDS,
                ExecutionLimitUnit::Milliseconds,
                Some("tool.timeout_seconds"),
            ),
        ),
        (
            "javascript.queued_jobs".to_owned(),
            execution_limit(
                u64::from(limits.queued_jobs),
                u64::from(ceiling.queued_jobs),
                ExecutionLimitUnit::Jobs,
                None,
            ),
        ),
    ]);
    let hit = hit.map(|limit| {
        let (configured, maximum, unit, manifest_field) = match limit {
            WorkerLimit::SourceBytes => (
                usize_as_u64(limits.source_bytes),
                usize_as_u64(ceiling.source_bytes),
                ExecutionLimitUnit::Bytes,
                None,
            ),
            WorkerLimit::InputBytes => (
                usize_as_u64(limits.input_bytes),
                usize_as_u64(ceiling.input_bytes),
                ExecutionLimitUnit::Bytes,
                None,
            ),
            WorkerLimit::OutputBytes => (
                usize_as_u64(limits.output_bytes),
                usize_as_u64(ceiling.output_bytes),
                ExecutionLimitUnit::Bytes,
                None,
            ),
            WorkerLimit::WallMilliseconds => (
                limits.wall_milliseconds,
                runx_contracts::javascript_worker::MAX_WALL_MILLISECONDS,
                ExecutionLimitUnit::Milliseconds,
                Some("tool.timeout_seconds"),
            ),
            WorkerLimit::QueuedJobs => (
                u64::from(limits.queued_jobs),
                u64::from(ceiling.queued_jobs),
                ExecutionLimitUnit::Jobs,
                None,
            ),
        };
        ExecutionLimitHit {
            id: format!("javascript.{}", limit.as_str()),
            limit: execution_limit(configured, maximum, unit, manifest_field),
        }
    });
    ExecutionLimits { configured, hit }
}

fn execution_limit(
    configured: u64,
    maximum: u64,
    unit: runx_contracts::ExecutionLimitUnit,
    manifest_field: Option<&str>,
) -> runx_contracts::ExecutionLimit {
    runx_contracts::ExecutionLimit {
        configured,
        maximum,
        unit,
        manifest_field: manifest_field.map(str::to_owned),
    }
}

fn usize_as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
