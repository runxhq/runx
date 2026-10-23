use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use runx_contracts::javascript_worker::{
    InvocationLimits, PROTOCOL_VERSION, WorkerFate, WorkerInvocationRequest, WorkerRequest,
};

use crate::RuntimeError;

mod pool;
mod process;
mod response_reader;
mod session;

use pool::WorkerPool;

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

pub(super) enum WorkerInvocationResult {
    Success(serde_json::Value),
    Failure { message: String, fate: WorkerFate },
}

pub(super) struct WorkerInvocationOutcome {
    pub(super) result: WorkerInvocationResult,
    pub(super) execution_boundary: runx_contracts::ExecutionBoundaryKind,
}

pub(super) struct JavaScriptWorkerSupervisor {
    pool: WorkerPool,
    next_invocation: AtomicU64,
}

impl JavaScriptWorkerSupervisor {
    pub(super) fn new(max_concurrency: usize) -> Self {
        Self {
            pool: WorkerPool::new(max_concurrency),
            next_invocation: AtomicU64::new(1),
        }
    }

    pub(super) fn invoke(
        &self,
        invocation: WorkerInvocation,
    ) -> Result<WorkerInvocationOutcome, RuntimeError> {
        self.invoke_inner(invocation, || {})
    }

    fn invoke_inner(
        &self,
        invocation: WorkerInvocation,
        after_acquire: impl FnOnce(),
    ) -> Result<WorkerInvocationOutcome, RuntimeError> {
        let invocation_id = format!(
            "js-{}",
            self.next_invocation.fetch_add(1, Ordering::Relaxed)
        );
        let timeout = Duration::from_millis(invocation.limits.wall_milliseconds);
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
        let mut lease = self.pool.acquire(worker_path.as_deref())?;
        after_acquire();
        let (execution_boundary, result) = {
            let session = lease.session_mut()?;
            let execution_boundary = session.execution_boundary;
            let result = session.invoke(&invocation_id, &request, timeout);
            (execution_boundary, result)
        };
        match result {
            Ok(response) => {
                let fate = match &response {
                    WorkerInvocationResult::Success(_) => WorkerFate::Reuse,
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
