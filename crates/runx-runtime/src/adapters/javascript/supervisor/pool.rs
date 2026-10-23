use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::RuntimeError;

use super::session::{WorkerLaunchPlan, WorkerSession};
use super::{WorkerFate, lock, worker_error};

pub(super) struct WorkerPool {
    state: Mutex<PoolState>,
    available: Condvar,
    launch_plan: Mutex<Option<Arc<WorkerLaunchPlan>>>,
    maximum: usize,
    spawn_count: AtomicU64,
    peak_in_flight: AtomicUsize,
}

#[derive(Default)]
struct PoolState {
    idle: Vec<WorkerSession>,
    total: usize,
    starting: usize,
    in_flight: usize,
}

impl WorkerPool {
    pub(super) fn new(maximum: usize) -> Self {
        Self {
            state: Mutex::new(PoolState::default()),
            available: Condvar::new(),
            launch_plan: Mutex::new(None),
            maximum,
            spawn_count: AtomicU64::new(0),
            peak_in_flight: AtomicUsize::new(0),
        }
    }

    pub(super) fn acquire(
        &self,
        worker_path_override: Option<&str>,
    ) -> Result<WorkerLease<'_>, RuntimeError> {
        let launch_plan = self.launch_plan(worker_path_override)?;
        loop {
            let mut state = lock(&self.state, "locking JavaScript worker pool")?;
            if let Some(session) = state.idle.pop() {
                self.record_in_flight(&mut state);
                return Ok(WorkerLease::new(self, session));
            }
            if state.total.saturating_add(state.starting) < self.maximum {
                state.starting = state.starting.saturating_add(1);
                drop(state);
                let started = WorkerSession::start(&launch_plan);
                let mut state = lock(&self.state, "recording JavaScript worker startup")?;
                state.starting = state.starting.saturating_sub(1);
                match started {
                    Ok(session) => {
                        state.total = state.total.saturating_add(1);
                        self.spawn_count.fetch_add(1, Ordering::Relaxed);
                        self.record_in_flight(&mut state);
                        self.available.notify_all();
                        return Ok(WorkerLease::new(self, session));
                    }
                    Err(error) => {
                        self.available.notify_all();
                        return Err(error);
                    }
                }
            }
            state = self.available.wait(state).map_err(|_| {
                worker_error("waiting for JavaScript worker capacity: mutex poisoned")
            })?;
            drop(state);
        }
    }

    fn launch_plan(
        &self,
        worker_path_override: Option<&str>,
    ) -> Result<Arc<WorkerLaunchPlan>, RuntimeError> {
        let mut cached = lock(&self.launch_plan, "locking JavaScript worker launch plan")?;
        if let Some(plan) = cached.as_ref() {
            if plan.worker_path_override.as_deref() != worker_path_override {
                return Err(worker_error(
                    "one JavaScript worker pool cannot mix runtime worker paths",
                ));
            }
            return Ok(plan.clone());
        }
        let plan = Arc::new(WorkerLaunchPlan::prepare(worker_path_override)?);
        *cached = Some(plan.clone());
        Ok(plan)
    }

    fn record_in_flight(&self, state: &mut PoolState) {
        state.in_flight = state.in_flight.saturating_add(1);
        self.peak_in_flight
            .fetch_max(state.in_flight, Ordering::Relaxed);
    }

    fn release(&self, mut session: WorkerSession, fate: WorkerFate) {
        if fate == WorkerFate::Discard {
            session.terminate();
        }
        if let Ok(mut state) = self.state.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
            match fate {
                WorkerFate::Reuse => state.idle.push(session),
                WorkerFate::Discard => {
                    state.total = state.total.saturating_sub(1);
                    drop(session);
                }
            }
            self.available.notify_one();
        }
    }

    pub(super) fn spawn_count(&self) -> u64 {
        self.spawn_count.load(Ordering::Relaxed)
    }

    pub(super) fn peak_in_flight(&self) -> usize {
        self.peak_in_flight.load(Ordering::Relaxed)
    }
}

pub(super) struct WorkerLease<'a> {
    pool: &'a WorkerPool,
    session: Option<WorkerSession>,
    fate: WorkerFate,
}

impl<'a> WorkerLease<'a> {
    fn new(pool: &'a WorkerPool, session: WorkerSession) -> Self {
        Self {
            pool,
            session: Some(session),
            fate: WorkerFate::Reuse,
        }
    }

    pub(super) fn session_mut(&mut self) -> Result<&mut WorkerSession, RuntimeError> {
        self.session
            .as_mut()
            .ok_or_else(|| worker_error("JavaScript worker lease no longer owns its session"))
    }

    pub(super) fn poison(&mut self) {
        self.fate = WorkerFate::Discard;
    }
}

impl Drop for WorkerLease<'_> {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            self.pool.release(session, self.fate);
        }
    }
}
