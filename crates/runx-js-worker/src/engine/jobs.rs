use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use boa_engine::job::{Job, JobExecutor};
use boa_engine::{Context, JsNativeError};

use super::EngineError;

pub(super) struct BoundedJobExecutor {
    promise_jobs: RefCell<VecDeque<boa_engine::job::PromiseJob>>,
    async_jobs: RefCell<VecDeque<boa_engine::job::NativeAsyncJob>>,
    generic_jobs: RefCell<VecDeque<boa_engine::job::GenericJob>>,
    count: Cell<u32>,
    maximum: u32,
    overflowed: Cell<bool>,
    /// A timer was asked for; the deterministic worker has no clock to run one.
    timed: Cell<bool>,
}

impl BoundedJobExecutor {
    pub(super) fn new(maximum: u32) -> Self {
        Self {
            promise_jobs: RefCell::new(VecDeque::new()),
            async_jobs: RefCell::new(VecDeque::new()),
            generic_jobs: RefCell::new(VecDeque::new()),
            count: Cell::new(0),
            maximum,
            overflowed: Cell::new(false),
            timed: Cell::new(false),
        }
    }

    /// The executor's own refusal, reported before the engine's error for it.
    pub(super) fn check(&self) -> Result<(), EngineError> {
        if self.overflowed.get() {
            return Err(EngineError::limit(format!(
                "JavaScript queued more than {} jobs",
                self.maximum
            )));
        }
        if self.timed.get() {
            return Err(EngineError::new(
                crate::protocol::WorkerFailureCode::ExecutionFailed,
                "JavaScript timers do not run in the deterministic worker",
            ));
        }
        Ok(())
    }

    fn admit(&self) -> bool {
        let next = self.count.get().saturating_add(1);
        self.count.set(next);
        if next > self.maximum {
            self.overflowed.set(true);
            return false;
        }
        true
    }
}

impl JobExecutor for BoundedJobExecutor {
    fn enqueue_job(self: Rc<Self>, job: Job, _context: &mut Context) {
        if !self.admit() {
            return;
        }
        match job {
            Job::PromiseJob(job) => self.promise_jobs.borrow_mut().push_back(job),
            Job::AsyncJob(job) => self.async_jobs.borrow_mut().push_back(job),
            Job::GenericJob(job) => self.generic_jobs.borrow_mut().push_back(job),
            Job::TimeoutJob(_) => self.timed.set(true),
            _ => self.overflowed.set(true),
        }
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> boa_engine::JsResult<()> {
        loop {
            let asynchronous = self.async_jobs.borrow_mut().pop_front();
            if let Some(job) = asynchronous {
                let context_cell = RefCell::new(&mut *context);
                futures_lite::future::block_on(job.call(&context_cell))?;
                continue;
            }
            let promise = self.promise_jobs.borrow_mut().pop_front();
            if let Some(job) = promise {
                job.call(context)?;
                continue;
            }
            let generic = self.generic_jobs.borrow_mut().pop_front();
            if let Some(job) = generic {
                job.call(context)?;
                continue;
            }
            break;
        }
        if self.overflowed.get() || self.timed.get() {
            return Err(JsNativeError::range()
                .with_message("the deterministic JavaScript worker refused a job")
                .into());
        }
        Ok(())
    }
}
