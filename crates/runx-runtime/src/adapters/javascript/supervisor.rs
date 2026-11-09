use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use runx_contracts::javascript_worker::{
    InvocationLimits, PROTOCOL_VERSION, ScriptCall, ScriptEnding, ScriptPassRequest,
    ScriptSettlement, WorkerFate, WorkerInvocationRequest, WorkerRequest,
};

use crate::RuntimeError;

mod pool;
mod process;
mod response_reader;
mod session;
mod watch;

use pool::WorkerPool;
use watch::{Clock, SystemClock};

#[derive(Debug)]
pub(super) struct WorkerInvocation {
    pub(super) entry_module: String,
    pub(super) export_name: String,
    pub(super) modules: BTreeMap<String, String>,
    pub(super) inputs: serde_json::Value,
    pub(super) environment: BTreeMap<String, String>,
    pub(super) worker_path: Option<String>,
    pub(super) limits: InvocationLimits,
}

/// One pass of a script step, as the supervisor sends it.
#[derive(Debug)]
pub(super) struct ScriptPassInvocation {
    pub(super) entry_module: String,
    pub(super) export_name: String,
    pub(super) modules: BTreeMap<String, String>,
    pub(super) inputs: serde_json::Value,
    pub(super) worker_path: Option<String>,
    pub(super) limits: InvocationLimits,
    pub(super) settled: Vec<ScriptSettlement>,
    pub(super) now: i64,
}

pub(super) enum WorkerInvocationResult {
    Success(serde_json::Value),
    /// A script pass: the calls it made and how it ended.
    Passed {
        calls: Vec<ScriptCall>,
        ending: ScriptEnding,
    },
    Failure {
        message: String,
        fate: WorkerFate,
    },
}

pub(super) struct WorkerInvocationOutcome {
    pub(super) result: WorkerInvocationResult,
    pub(super) execution_boundary: runx_contracts::ExecutionBoundaryKind,
}

pub(super) struct JavaScriptWorkerSupervisor {
    pool: WorkerPool,
    next_invocation: AtomicU64,
    /// What a worker's idleness is measured by.
    clock: Box<dyn Clock>,
}

impl JavaScriptWorkerSupervisor {
    pub(super) fn new(max_concurrency: usize) -> Self {
        Self::with_clock(max_concurrency, Box::new(SystemClock))
    }

    fn with_clock(max_concurrency: usize, clock: Box<dyn Clock>) -> Self {
        Self {
            pool: WorkerPool::new(max_concurrency),
            next_invocation: AtomicU64::new(1),
            clock,
        }
    }

    pub(super) fn invoke(
        &self,
        invocation: WorkerInvocation,
    ) -> Result<WorkerInvocationOutcome, RuntimeError> {
        self.invoke_inner(invocation, || {})
    }

    /// Run one pass of a script step.
    pub(super) fn pass(
        &self,
        pass: ScriptPassInvocation,
    ) -> Result<WorkerInvocationOutcome, RuntimeError> {
        let invocation_id = self.invocation_id();
        let cpu_budget = Duration::from_millis(pass.limits.cpu_milliseconds);
        let request = WorkerRequest::Pass(Box::new(ScriptPassRequest {
            protocol_version: PROTOCOL_VERSION,
            invocation_id: invocation_id.clone(),
            entry_module: pass.entry_module,
            export_name: pass.export_name,
            modules: pass.modules,
            inputs: pass.inputs,
            limits: pass.limits,
            settled: pass.settled,
            now: pass.now,
        }));
        self.exchange(
            &invocation_id,
            &request,
            cpu_budget,
            pass.worker_path.as_deref(),
            || {},
        )
    }

    fn invocation_id(&self) -> String {
        format!(
            "js-{}",
            self.next_invocation.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn invoke_inner(
        &self,
        invocation: WorkerInvocation,
        after_acquire: impl FnOnce(),
    ) -> Result<WorkerInvocationOutcome, RuntimeError> {
        let invocation_id = self.invocation_id();
        let cpu_budget = Duration::from_millis(invocation.limits.cpu_milliseconds);
        let worker_path = invocation.worker_path;
        let request = WorkerRequest::Invoke(Box::new(WorkerInvocationRequest {
            protocol_version: PROTOCOL_VERSION,
            invocation_id: invocation_id.clone(),
            entry_module: invocation.entry_module,
            export_name: invocation.export_name,
            modules: invocation.modules,
            inputs: invocation.inputs,
            environment: invocation.environment,
            limits: invocation.limits,
        }));
        self.exchange(
            &invocation_id,
            &request,
            cpu_budget,
            worker_path.as_deref(),
            after_acquire,
        )
    }

    /// Send `request` on a leased worker and read its answer within
    /// `cpu_budget` of the worker's CPU time; a worker that failed in a way
    /// that leaves it unusable is discarded.
    fn exchange(
        &self,
        invocation_id: &str,
        request: &WorkerRequest,
        cpu_budget: Duration,
        worker_path: Option<&str>,
        after_acquire: impl FnOnce(),
    ) -> Result<WorkerInvocationOutcome, RuntimeError> {
        let mut lease = self.pool.acquire(worker_path)?;
        after_acquire();
        let (execution_boundary, result) = {
            let session = lease.session_mut()?;
            let execution_boundary = session.execution_boundary;
            let result = session.invoke(invocation_id, request, cpu_budget, self.clock.as_ref());
            (execution_boundary, result)
        };
        match result {
            Ok(response) => {
                let fate = match &response {
                    WorkerInvocationResult::Success(_) | WorkerInvocationResult::Passed { .. } => {
                        WorkerFate::Reuse
                    }
                    WorkerInvocationResult::Failure { fate, .. } => *fate,
                };
                if fate == WorkerFate::Discard {
                    lease.poison();
                }
                Ok(WorkerInvocationOutcome {
                    result: response,
                    execution_boundary,
                })
            }
            Err(error) => {
                lease.poison();
                Err(error)
            }
        }
    }

    #[cfg(test)]
    fn invoke_after_acquire(
        &self,
        invocation: WorkerInvocation,
        after_acquire: impl FnOnce(),
    ) -> Result<WorkerInvocationOutcome, RuntimeError> {
        self.invoke_inner(invocation, after_acquire)
    }

    pub(super) fn spawn_count(&self) -> u64 {
        self.pool.spawn_count()
    }

    pub(super) fn peak_in_flight(&self) -> usize {
        self.pool.peak_in_flight()
    }
}

fn lock<'a, T>(
    mutex: &'a Mutex<T>,
    context: &str,
) -> Result<std::sync::MutexGuard<'a, T>, RuntimeError> {
    mutex
        .lock()
        .map_err(|_| worker_error(format!("{context}: mutex poisoned")))
}

fn worker_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::JavaScriptWorker {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
