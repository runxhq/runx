#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
#[cfg(unix)]
use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use runx_contracts::javascript_worker::InvocationLimits;
use runx_contracts::javascript_worker::MAX_STDERR_BYTES;

use super::process::BoundedStderr;
#[cfg(unix)]
use super::process::{worker_binary_name, worker_candidates};
use super::{JavaScriptWorkerSupervisor, WorkerInvocation, WorkerInvocationResult};

#[cfg(unix)]
#[test]
fn worker_candidates_include_the_real_binary_directory_for_a_dev_symlink() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("temporary worker layout");
    let target_dir = temp.path().join("target/debug");
    let link_dir = temp.path().join("bin");
    fs::create_dir_all(&target_dir).expect("target directory");
    fs::create_dir_all(&link_dir).expect("link directory");

    let runx = target_dir.join("runx");
    let worker = target_dir.join(worker_binary_name());
    fs::write(&runx, b"runx").expect("runx binary fixture");
    fs::write(&worker, b"worker").expect("worker binary fixture");
    let link = link_dir.join("runx");
    symlink(&runx, &link).expect("runx dev symlink");

    let canonical_worker = fs::canonicalize(worker).expect("canonical worker fixture");
    assert!(worker_candidates(&link, worker_binary_name()).contains(&canonical_worker));
}

#[test]
fn bounded_stderr_never_retains_more_than_the_protocol_limit() {
    let mut capture = BoundedStderr::default();
    capture.push(&vec![b'x'; MAX_STDERR_BYTES + 10]);
    assert_eq!(capture.bytes.len(), MAX_STDERR_BYTES);
    assert!(capture.truncated);
}

#[test]
fn supervisors_own_independent_session_state() {
    let first = JavaScriptWorkerSupervisor::new(1);
    let second = JavaScriptWorkerSupervisor::new(1);
    assert_eq!(first.spawn_count(), 0);
    assert_eq!(second.spawn_count(), 0);
}

#[test]
fn concurrently_started_workers_close_while_their_siblings_stay_alive()
-> Result<(), Box<dyn std::error::Error>> {
    use super::session::{WorkerLaunchPlan, WorkerSession};

    let plan = WorkerLaunchPlan::prepare(None)?;
    for _ in 0..32 {
        let barrier = Barrier::new(4);
        let mut sessions = thread::scope(|scope| {
            let starts = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        WorkerSession::start(&plan)
                    })
                })
                .collect::<Vec<_>>();
            starts
                .into_iter()
                .map(|start| start.join().expect("worker startup thread"))
                .collect::<Result<Vec<_>, _>>()
        })?;
        // Each session closes while its siblings still run: its reader sees
        // its own worker's pipes close, which no sibling holds open. A
        // session whose close waited on a live sibling would never be joined.
        while let Some(session) = sessions.pop() {
            thread::spawn(move || drop(session))
                .join()
                .expect("worker shutdown thread");
        }
    }
    Ok(())
}

#[test]
fn pooled_worker_never_reuses_a_session_for_a_different_runtime_path()
-> Result<(), Box<dyn std::error::Error>> {
    let supervisor = JavaScriptWorkerSupervisor::new(1);
    supervisor.invoke(invocation(
        "export default () => ({ ready: true });",
        InvocationLimits::default().cpu_milliseconds,
    ))?;
    let missing = tempfile::tempdir()?.path().join("missing-worker");
    let mut changed = invocation(
        "export default () => ({ should_not_run: true });",
        InvocationLimits::default().cpu_milliseconds,
    );
    changed.worker_path = Some(missing.to_string_lossy().into_owned());

    let error = supervisor
        .invoke(changed)
        .err()
        .ok_or("a different worker path unexpectedly reused the pooled process")?;
    assert!(
        error
            .to_string()
            .contains("cannot mix runtime worker paths")
    );
    Ok(())
}

#[test]
fn a_worker_past_its_cpu_time_does_not_fail_a_healthy_sibling()
-> Result<(), Box<dyn std::error::Error>> {
    let supervisor = Arc::new(JavaScriptWorkerSupervisor::new(2));
    let barrier = Arc::new(Barrier::new(3));
    let slow = invoke_at(
        &supervisor,
        &barrier,
        invocation(
            "export default () => { let value = 0; for (let index = 0; index < 9000000; index += 1) value = (value + index) % 1000003; return { value }; };",
            1,
        ),
    );
    let healthy = invoke_at(
        &supervisor,
        &barrier,
        invocation(
            "export default () => { let value = 0; for (let index = 0; index < 100000; index += 1) value = (value + index) % 1000003; return { healthy: true }; };",
            InvocationLimits::default().cpu_milliseconds,
        ),
    );
    barrier.wait();

    let slow = slow
        .join()
        .map_err(|_| "slow JavaScript invocation thread panicked")??;
    assert!(
        matches!(
            slow.result,
            WorkerInvocationResult::Failure {
                ref message,
                fate: runx_contracts::javascript_worker::WorkerFate::Discard,
            } if message.contains("exceeded its 1 ms CPU time limit")
        ),
        "an invocation past its CPU time is a typed CPU-limit failure"
    );
    let healthy = healthy
        .join()
        .map_err(|_| "healthy JavaScript invocation thread panicked")??;
    assert!(matches!(
        healthy.result,
        WorkerInvocationResult::Success(ref output)
            if output == &serde_json::json!({"healthy": true})
    ));
    assert_eq!(supervisor.spawn_count(), 2);
    assert_eq!(supervisor.peak_in_flight(), 2);

    let barrier = Arc::new(Barrier::new(3));
    let recovered = (0..2)
        .map(|_| {
            invoke_at(
                &supervisor,
                &barrier,
                invocation(
                    "export default () => { let value = 0; for (let index = 0; index < 100000; index += 1) value = (value + index) % 1000003; return { recovered: true }; };",
                    InvocationLimits::default().cpu_milliseconds,
                ),
            )
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for invocation in recovered {
        let outcome = invocation
            .join()
            .map_err(|_| "recovery JavaScript invocation thread panicked")??;
        assert!(matches!(
            outcome.result,
            WorkerInvocationResult::Success(ref output)
                if output == &serde_json::json!({"recovered": true})
        ));
    }
    assert_eq!(supervisor.spawn_count(), 3);
    Ok(())
}

/// Run `invocation` on its own thread, arriving at `barrier` once it holds a
/// worker, or at once when it couldn't get one. So a worker that can't start
/// fails the test with its own error, rather than leaving the barrier waiting
/// for a thread that will never arrive.
fn invoke_at(
    supervisor: &Arc<JavaScriptWorkerSupervisor>,
    barrier: &Arc<Barrier>,
    invocation: WorkerInvocation,
) -> thread::JoinHandle<Result<super::WorkerInvocationOutcome, crate::RuntimeError>> {
    let supervisor = Arc::clone(supervisor);
    let barrier = Arc::clone(barrier);
    thread::spawn(move || {
        let mut arrived = false;
        let outcome = supervisor.invoke_after_acquire(invocation, || {
            arrived = true;
            barrier.wait();
        });
        if !arrived {
            barrier.wait();
        }
        outcome
    })
}

fn invocation(source: &str, cpu_milliseconds: u64) -> WorkerInvocation {
    WorkerInvocation {
        entry_module: "main.mjs".to_owned(),
        export_name: "default".to_owned(),
        modules: BTreeMap::from([("main.mjs".to_owned(), source.to_owned())]),
        inputs: serde_json::json!({}),
        environment: BTreeMap::new(),
        worker_path: None,
        limits: InvocationLimits {
            cpu_milliseconds,
            ..InvocationLimits::default()
        },
    }
}

/// A clock that moves on 30 seconds each time it is read, so a worker that
/// spends nothing is idle past the unresponsive window within a few readings.
struct Advancing {
    started: std::time::Instant,
    readings: std::sync::atomic::AtomicU32,
}

impl super::watch::Clock for Advancing {
    fn now(&self) -> std::time::Instant {
        let readings = self
            .readings
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.started + std::time::Duration::from_secs(30 * u64::from(readings))
    }
}

#[test]
fn the_watch_stops_an_invocation_past_its_cpu_time_or_idle_past_the_unresponsive_window() {
    use std::time::{Duration, Instant};

    use super::watch::{UNRESPONSIVE_AFTER, Verdict, Watch};

    let start = Instant::now();
    let second = Duration::from_secs(1);
    // Spending within its budget, a worker is working however long it takes.
    let mut spinning = Watch::start(2 * second, 5 * second, start);
    assert_eq!(
        spinning.observe(Some(6 * second), start + 10 * UNRESPONSIVE_AFTER),
        Verdict::Working
    );
    // One reading past its budget, counted from the invocation's start, stops it.
    assert_eq!(
        spinning.observe(Some(7 * second + Duration::from_millis(1)), start),
        Verdict::OverBudget
    );
    // A worker that spends nothing is unresponsive only once the window has
    // passed since it last spent any.
    let mut blocked = Watch::start(2 * second, second, start);
    assert_eq!(
        blocked.observe(Some(second), start + UNRESPONSIVE_AFTER - second),
        Verdict::Working
    );
    assert_eq!(
        blocked.observe(None, start + UNRESPONSIVE_AFTER),
        Verdict::Unresponsive
    );
    // Spending any CPU time restarts the window.
    let mut waking = Watch::start(2 * second, second, start);
    let later = start + UNRESPONSIVE_AFTER - second;
    assert_eq!(
        waking.observe(Some(second + Duration::from_millis(5)), later),
        Verdict::Working
    );
    assert_eq!(
        waking.observe(None, start + UNRESPONSIVE_AFTER),
        Verdict::Working
    );
    assert_eq!(
        waking.observe(None, later + UNRESPONSIVE_AFTER),
        Verdict::Unresponsive
    );
}

/// A worker that completes its handshake and then blocks reading its pipe
/// spends no CPU time and answers nothing: the supervisor discards it as
/// unresponsive once its clock says the window has passed.
#[cfg(unix)]
#[test]
fn a_worker_blocked_on_its_pipe_is_discarded_as_unresponsive()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    use runx_contracts::javascript_worker::{PROTOCOL_VERSION, WorkerFate};

    let temp = tempfile::tempdir()?;
    let ready = format!(r#"{{"type":"ready","protocol_version":{PROTOCOL_VERSION}}}"#);
    let length = u32::try_from(ready.len())?.to_be_bytes();
    let prefix = length
        .iter()
        .map(|byte| format!("\\{byte:03o}"))
        .collect::<String>();
    let worker = temp.path().join("blocked-worker");
    fs::write(
        &worker,
        format!("#!/bin/sh\nprintf '{prefix}%s' '{ready}'\n/bin/cat >/dev/null\n"),
    )?;
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o755))?;

    let supervisor = JavaScriptWorkerSupervisor::with_clock(
        1,
        Box::new(Advancing {
            started: std::time::Instant::now(),
            readings: std::sync::atomic::AtomicU32::new(0),
        }),
    );
    let mut blocked = invocation(
        "export default () => ({ never: true });",
        InvocationLimits::default().cpu_milliseconds,
    );
    blocked.worker_path = Some(worker.to_string_lossy().into_owned());
    let outcome = supervisor.invoke(blocked)?;
    assert!(
        matches!(
            outcome.result,
            WorkerInvocationResult::Failure {
                ref message,
                fate: WorkerFate::Discard,
            } if message.contains("spent no CPU time and answered nothing for 60 s")
        ),
        "a blocked worker is discarded as unresponsive"
    );
    Ok(())
}
