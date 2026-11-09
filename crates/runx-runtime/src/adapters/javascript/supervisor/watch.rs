//! How far one invocation may go: the CPU time its worker spends, read from
//! the worker process's CPU clock outside its sandbox, and how long the worker
//! may spend none while answering nothing. A worker's invocation is pure
//! computation, so CPU time is what its limit protects; the machine's load
//! never counts against it.

use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// A worker that spends no CPU time and answers nothing this long is
/// discarded as unresponsive. It can't race a worker that's computing.
pub(super) const UNRESPONSIVE_AFTER: Duration = Duration::from_secs(60);

/// How often the supervisor reads the CPU clock of a worker that hasn't
/// answered yet.
pub(super) const SAMPLE_EVERY: Duration = Duration::from_millis(20);

/// Where an invocation reads the time it measures a worker's idleness by.
pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// The system's monotonic clock.
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A worker process's CPU clock: the CPU time it has spent since it started.
pub(super) struct CpuClock {
    system: System,
    pid: Pid,
}

impl CpuClock {
    pub(super) fn of(process_id: u32) -> Self {
        Self {
            system: System::new(),
            pid: Pid::from_u32(process_id),
        }
    }

    /// The CPU time the process has spent, or none once it can't be read,
    /// as when it has exited.
    pub(super) fn spent(&mut self) -> Option<Duration> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
        self.system
            .process(self.pid)
            .map(|process| Duration::from_millis(process.accumulated_cpu_time()))
    }
}

/// What one reading of a worker's CPU clock says of its invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Working,
    /// The invocation has spent more CPU time than its limit.
    OverBudget,
    /// The worker has spent no CPU time and answered nothing for
    /// [`UNRESPONSIVE_AFTER`].
    Unresponsive,
}

/// One invocation's watch over its worker: the CPU time it started at, and
/// when the worker last spent any.
pub(super) struct Watch {
    budget: Duration,
    started: Duration,
    last: Duration,
    progressed_at: Instant,
}

impl Watch {
    pub(super) fn start(budget: Duration, cpu: Duration, now: Instant) -> Self {
        Self {
            budget,
            started: cpu,
            last: cpu,
            progressed_at: now,
        }
    }

    /// The verdict on a reading `cpu` of the worker's CPU clock taken at
    /// `now`; an unreadable clock counts as no CPU spent.
    pub(super) fn observe(&mut self, cpu: Option<Duration>, now: Instant) -> Verdict {
        let cpu = cpu.unwrap_or(self.last);
        if cpu.saturating_sub(self.started) > self.budget {
            return Verdict::OverBudget;
        }
        if cpu > self.last {
            self.last = cpu;
            self.progressed_at = now;
            return Verdict::Working;
        }
        if now.saturating_duration_since(self.progressed_at) >= UNRESPONSIVE_AFTER {
            Verdict::Unresponsive
        } else {
            Verdict::Working
        }
    }
}
