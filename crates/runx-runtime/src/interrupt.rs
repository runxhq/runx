use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::process::{ProcessSignal, signal_process_group_id};

fn active_process_groups() -> &'static Mutex<BTreeSet<u32>> {
    static ACTIVE: OnceLock<Mutex<BTreeSet<u32>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn interrupt_requested() -> &'static AtomicBool {
    static INTERRUPTED: AtomicBool = AtomicBool::new(false);
    &INTERRUPTED
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
    fn active_process_groups_unregister_on_drop() {
        let process_id = u32::MAX;
        {
            let _active = ActiveProcessGroup::register(process_id);
            assert!(lock_active_process_groups().contains(&process_id));
        }
        assert!(!lock_active_process_groups().contains(&process_id));
    }
}
