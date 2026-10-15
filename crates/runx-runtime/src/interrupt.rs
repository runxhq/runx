use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::process::{ProcessSignal, signal_process_group_id};

fn active_process_groups() -> &'static Mutex<BTreeSet<u32>> {
    static ACTIVE: OnceLock<Mutex<BTreeSet<u32>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn interrupt_requested() -> &'static AtomicBool {
    static INTERRUPTED: AtomicBool = AtomicBool::new(false);
    &INTERRUPTED
}

/// A graceful stop of the whole process, asked by `SIGTERM`.
fn stop_asked() -> &'static Arc<AtomicBool> {
    static STOPPING: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    STOPPING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

thread_local! {
    /// The stop of the call this thread works, set when its host cancels it.
    static CALL_STOP: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Ask the process to stop gracefully: a driver admits no further step or
/// call, a command it supervises is terminated and recorded stopped, and an
/// operation already reaching its provider returns and is recorded.
pub fn stop() {
    stop_asked().store(true, Ordering::Release);
}

/// Whether `SIGTERM` asked the process to stop.
#[must_use]
pub fn was_stopped() -> bool {
    stop_asked().load(Ordering::Acquire)
}

/// Do `work` as one call its host may cancel through `stop`: while it runs on
/// this thread, a set `stop` stops it as [`stop`] stops the process, and the
/// thread's next call starts clear.
pub fn while_working<T>(stop: &Arc<AtomicBool>, work: impl FnOnce() -> T) -> T {
    let previous = CALL_STOP.with(|call| call.replace(Some(Arc::clone(stop))));
    let result = work();
    CALL_STOP.with(|call| *call.borrow_mut() = previous);
    result
}

/// Whether the work on this thread should stop: the process was asked to,
/// or its host cancelled the call it serves.
#[must_use]
pub fn stop_requested() -> bool {
    was_stopped()
        || CALL_STOP.with(|call| {
            call.borrow()
                .as_ref()
                .is_some_and(|stop| stop.load(Ordering::Acquire))
        })
}

/// Stop gracefully on `SIGTERM`, for a process that drives a run; a second
/// `SIGTERM` ends it at once. `SIGINT` and the death of the parent keep the
/// forced [`interrupt`].
pub fn stop_on_terminate() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use signal_hook::consts::SIGTERM;
        // Registered first, so it sees the flag the first signal sets only
        // on the second.
        signal_hook::flag::register_conditional_shutdown(SIGTERM, 143, Arc::clone(stop_asked()))?;
        signal_hook::flag::register(SIGTERM, Arc::clone(stop_asked()))?;
    }
    Ok(())
}

fn lock_active_process_groups() -> std::sync::MutexGuard<'static, BTreeSet<u32>> {
    active_process_groups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Tracks one child process group for the lifetime of its supervised execution.
///
/// The terminal interrupt handler uses this registry to terminate every active
/// context before exiting. Registration is runtime-owned so CLI tools,
/// JavaScript workers, and connector MCP servers share one boundary.
pub(crate) struct ActiveProcessGroup {
    process_id: u32,
}

impl ActiveProcessGroup {
    pub(crate) fn register(process_id: u32) -> Self {
        lock_active_process_groups().insert(process_id);
        if interrupt_requested().load(Ordering::Acquire) {
            let _terminated = signal_process_group_id(process_id, ProcessSignal::Force);
        }
        Self { process_id }
    }
}

impl Drop for ActiveProcessGroup {
    fn drop(&mut self) {
        lock_active_process_groups().remove(&self.process_id);
    }
}

/// Force-terminate every currently supervised child process group.
///
/// Returns `true` for the first interrupt request in this process. The CLI uses
/// that distinction to allow one short cleanup window and make a repeated
/// interrupt exit immediately.
pub fn terminate_active_processes() -> bool {
    let first = !interrupt_requested().swap(true, Ordering::AcqRel);
    let process_ids = lock_active_process_groups()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    for process_id in process_ids {
        let _terminated = signal_process_group_id(process_id, ProcessSignal::Force);
    }
    first
}

#[must_use]
pub fn was_interrupted() -> bool {
    interrupt_requested().load(Ordering::Acquire)
}

/// End the process early, the one way it ends early: stop every supervised
/// child process group, then exit after a short cleanup window. A second
/// request exits at once.
pub fn interrupt() {
    if !terminate_active_processes() {
        std::process::exit(130);
    }
    let watchdog = std::thread::Builder::new()
        .name("runx-interrupt-watchdog".to_owned())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            std::process::exit(130);
        });
    if watchdog.is_err() {
        std::process::exit(130);
    }
}

/// End the process through [`interrupt`] once the process that started it is
/// gone, checked every second, so a host that dies leaves no driver behind.
/// A process whose parent is already init has nothing to outlive.
pub fn end_with_parent() {
    #[cfg(unix)]
    {
        let parent = std::os::unix::process::parent_id();
        if parent > 1 {
            let _watch = std::thread::Builder::new()
                .name("runx-parent-watch".to_owned())
                .spawn(move || {
                    while std::os::unix::process::parent_id() == parent {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    interrupt();
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_call_stops_only_while_it_is_worked() {
        let stop = Arc::new(AtomicBool::new(false));
        assert!(!stop_requested());
        while_working(&stop, || {
            assert!(!stop_requested());
            stop.store(true, Ordering::Release);
            assert!(stop_requested());
        });
        assert!(!stop_requested(), "the thread's next call starts clear");
        let other = std::thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                while_working(&Arc::new(AtomicBool::new(false)), || {
                    let _cancelled = &stop;
                    stop_requested()
                })
            }
        });
        assert_eq!(
            other.join().ok(),
            Some(false),
            "another call is not stopped"
        );
    }

    #[test]
    fn active_process_groups_unregister_on_drop() {
        let process_id = u32::MAX;
        {
            let _active = ActiveProcessGroup::register(process_id);
            assert!(lock_active_process_groups().contains(&process_id));
        }
        assert!(!lock_active_process_groups().contains(&process_id));
    }
}
