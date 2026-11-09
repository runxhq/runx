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
    /// What the work this thread does runs under.
    static WORKING: RefCell<Working> = RefCell::new(Working::default());
}

/// What a thread's work runs under, outermost first: the stops any one of
/// which stops it, the call its host serves and within it a call its host
/// may cancel alone, and the calls whose child runs it is part of.
#[derive(Clone, Debug, Default)]
pub(crate) struct Working {
    stops: Vec<Arc<AtomicBool>>,
    callers: Vec<Arc<Caller>>,
}

/// A call whose work is a child run: whether its host cancelled it, which
/// the child heeds at every depth, and whether anything in the child began
/// an effect.
#[derive(Debug)]
pub(crate) struct Caller {
    cancelled: Arc<AtomicBool>,
    effected: AtomicBool,
}

impl Caller {
    /// A call its host cancels through `cancelled`.
    pub(crate) fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            effected: AtomicBool::new(false),
        }
    }

    /// Whether anything its child run did began an effect.
    pub(crate) fn effected(&self) -> bool {
        self.effected.load(Ordering::Acquire)
    }
}

/// Whether `SIGTERM` asked the process to stop.
#[must_use]
pub fn was_stopped() -> bool {
    stop_asked().load(Ordering::Acquire)
}

/// Do `work` as one call its host may cancel through `stop`: while it runs on
/// this thread, a set `stop` stops it as `SIGTERM` stops the process, as
/// does any stop the work it's part of runs under, and the thread's next
/// call starts clear.
pub fn while_working<T>(stop: &Arc<AtomicBool>, work: impl FnOnce() -> T) -> T {
    let mut working = working();
    working.stops.push(Arc::clone(stop));
    while_working_under(&working, work)
}

/// What the work on this thread runs under, for a part of it handed to
/// another thread.
#[must_use]
pub(crate) fn working() -> Working {
    WORKING.with(|working| working.borrow().clone())
}

/// Do `work`, a part of other work handed to this thread, under `working`,
/// what that work runs under.
pub(crate) fn while_working_under<T>(working: &Working, work: impl FnOnce() -> T) -> T {
    /// Puts the thread's work back as it was, however the work ends.
    struct Restore(Working);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = std::mem::take(&mut self.0);
            WORKING.with(|current| *current.borrow_mut() = previous);
        }
    }
    let _restore = Restore(WORKING.with(|current| current.replace(working.clone())));
    work()
}

/// Do `work` as the child run of `caller`, a call its host may cancel.
pub(crate) fn while_called<T>(caller: &Arc<Caller>, work: impl FnOnce() -> T) -> T {
    let mut working = working();
    working.callers.push(Arc::clone(caller));
    while_working_under(&working, work)
}

/// Do `work`, a read, so that a cancel of it through `stop`, or of any call
/// whose child run it is part of, stops it.
pub(crate) fn while_reading<T>(stop: Option<&Arc<AtomicBool>>, work: impl FnOnce() -> T) -> T {
    let mut working = working();
    let callers = working
        .callers
        .iter()
        .map(|caller| Arc::clone(&caller.cancelled))
        .collect::<Vec<_>>();
    working
        .stops
        .extend(stop.cloned().into_iter().chain(callers));
    while_working_under(&working, work)
}

/// Whether the host of a call whose child run this work is part of
/// cancelled it, at any depth: the run starts no further step.
#[must_use]
pub(crate) fn caller_cancelled() -> bool {
    WORKING.with(|working| {
        working
            .borrow()
            .callers
            .iter()
            .any(|caller| caller.cancelled.load(Ordering::Acquire))
    })
}

/// Say this work is beginning an effect: every call whose child run it is
/// part of has committed one.
pub(crate) fn note_effect() {
    WORKING.with(|working| {
        for caller in &working.borrow().callers {
            caller.effected.store(true, Ordering::Release);
        }
    });
}

/// Whether the work on this thread should stop: the process was asked to,
/// or its host cancelled the call it serves, or the call within it.
#[must_use]
pub fn stop_requested() -> bool {
    was_stopped()
        || WORKING.with(|working| {
            working
                .borrow()
                .stops
                .iter()
                .any(|stop| stop.load(Ordering::Acquire))
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
    fn work_handed_to_another_thread_keeps_its_stops() {
        let served = Arc::new(AtomicBool::new(false));
        let handed = while_working(&served, working);
        let part = std::thread::spawn({
            let served = Arc::clone(&served);
            move || {
                while_working_under(&handed, || {
                    let before = stop_requested();
                    served.store(true, Ordering::Release);
                    (before, stop_requested())
                })
            }
        });
        assert_eq!(
            part.join().ok(),
            Some((false, true)),
            "a cancel of the call it's part of stops it"
        );
        let own = Arc::new(AtomicBool::new(true));
        served.store(false, Ordering::Release);
        while_working(&served, || {
            assert!(!stop_requested());
            while_working(&own, || {
                assert!(stop_requested(), "its own cancel stops it")
            });
            assert!(!stop_requested(), "and only it");
        });
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
