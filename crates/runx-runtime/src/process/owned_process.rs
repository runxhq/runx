use std::io;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};
use std::time::Duration;

#[cfg(not(windows))]
use std::process::Child;
#[cfg(not(windows))]
use wait_timeout::ChildExt;

use super::signals::ProcessSignal;
#[cfg(not(windows))]
use super::signals::{configure_process_group, signal_process_group_id};

/// Owns one subprocess tree, not only its root process.
///
/// Unix uses a dedicated process group. Windows uses a race-free Job Object
/// created before the suspended child is resumed. The Windows host process is
/// also placed in an outer kill-on-close job so an abrupt Runx exit cannot
/// orphan any nested execution jobs.
pub(super) struct OwnedProcess {
    #[cfg(not(windows))]
    child: Child,
    #[cfg(windows)]
    child: Box<dyn process_wrap::std::ChildWrapper>,
}

impl OwnedProcess {
    pub(super) fn spawn(command: Command) -> io::Result<Self> {
        #[cfg(not(windows))]
        {
            let mut command = command;
            configure_process_group(&mut command);
            super::with_spawn_lock(|| command.spawn()).map(|child| Self { child })
        }

        #[cfg(windows)]
        {
            use process_wrap::std::{CommandWrap, JobObject};

            super::ensure_windows_host_job()?;
            let mut wrapped = CommandWrap::from(command);
            wrapped.wrap(JobObject);
            crate::work_counts::counted_spawn(wrapped.spawn()).map(|child| Self { child })
        }
    }

    pub(super) fn id(&self) -> u32 {
        #[cfg(not(windows))]
        {
            self.child.id()
        }
        #[cfg(windows)]
        {
            self.child.id()
        }
    }

    pub(super) fn take_stdin(&mut self) -> Option<ChildStdin> {
        #[cfg(not(windows))]
        {
            self.child.stdin.take()
        }
        #[cfg(windows)]
        {
            self.child.stdin().take()
        }
    }

    pub(super) fn take_stdout(&mut self) -> Option<ChildStdout> {
        #[cfg(not(windows))]
        {
            self.child.stdout.take()
        }
        #[cfg(windows)]
        {
            self.child.stdout().take()
        }
    }

    pub(super) fn take_stderr(&mut self) -> Option<ChildStderr> {
        #[cfg(not(windows))]
        {
            self.child.stderr.take()
        }
        #[cfg(windows)]
        {
            self.child.stderr().take()
        }
    }

    pub(super) fn wait_timeout(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        #[cfg(not(windows))]
        {
            self.child.wait_timeout(timeout)
        }
        #[cfg(windows)]
        {
            if let Some(status) = self.child.try_wait()? {
                return Ok(Some(status));
            }
            std::thread::sleep(timeout);
            self.child.try_wait()
        }
    }

    pub(super) fn reap_with_timeout(&mut self, timeout: Duration) -> io::Result<ExitStatus> {
        #[cfg(not(windows))]
        {
            self.child.wait_timeout(timeout)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out reaping terminated process",
                )
            })
        }
        #[cfg(windows)]
        {
            // JobObjectChild::wait waits for completion-port state in addition
            // to reaping the root and can block after TerminateJobObject has
            // already drained that state. Poll the root instead; the Job
            // Object owns termination of the complete tree.
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if let Some(status) = self.child.try_wait()? {
                    return Ok(status);
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out reaping terminated process",
                    ));
                }
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
            }
        }
    }

    pub(super) fn signal(&mut self, signal: ProcessSignal) -> io::Result<()> {
        #[cfg(not(windows))]
        {
            if signal_process_group_id(self.child.id(), signal) {
                return Ok(());
            }
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            self.child.kill()
        }

        #[cfg(windows)]
        {
            let _ = signal;
            self.child.start_kill()
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let _ = self.signal(ProcessSignal::Force);
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use std::process::Command;
    use std::time::Duration;

    use super::{OwnedProcess, ProcessSignal};

    #[test]
    fn unix_reap_with_timeout_honors_its_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 5"]);
        let mut process = OwnedProcess::spawn(command)?;

        let error = match process.reap_with_timeout(Duration::ZERO) {
            Ok(status) => return Err(format!("running process was reaped as {status}").into()),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);

        process.signal(ProcessSignal::Force)?;
        let _status = process.reap_with_timeout(Duration::from_secs(5))?;
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    const FIXTURE_ENV: &str = "RUNX_WINDOWS_HOST_JOB_FIXTURE";
    const NODE_ENV: &str = "RUNX_WINDOWS_TEST_NODE";
    const PID_PREFIX: &str = "RUNX_WINDOWS_DESCENDANT=";

    #[test]
    fn windows_host_job_fixture() -> Result<(), String> {
        if std::env::var_os(FIXTURE_ENV).is_none() {
            return Ok(());
        }
        super::super::ensure_windows_host_job().map_err(|error| error.to_string())?;
        let node = std::env::var(NODE_ENV).map_err(|error| error.to_string())?;
        let child = Command::new(node)
            .args(["-e", "setInterval(()=>{},1000)"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("spawning host-job descendant: {error}"))?;
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        writeln!(stdout, "{PID_PREFIX}{}", child.id()).map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn windows_host_job_reaps_descendants_after_abrupt_owner_exit() -> Result<(), String> {
        let node = node_binary()?;
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut fixture = Command::new(executable)
            .args([
                "--exact",
                "process::owned_process::tests::windows_host_job_fixture",
                "--nocapture",
            ])
            .env(FIXTURE_ENV, "1")
            .env(NODE_ENV, &node)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("spawning host-job fixture: {error}"))?;
        let stdout = fixture
            .stdout
            .take()
            .ok_or_else(|| "host-job fixture stdout was not captured".to_owned())?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(value) = line.strip_prefix(PID_PREFIX) {
                    let _ = sender.send(value.to_owned());
                    return;
                }
            }
        });
        let descendant = match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(value) => value.parse::<u32>().map_err(|error| error.to_string())?,
            Err(error) => {
                let _ = fixture.kill();
                let _ = fixture.wait();
                let _ = reader.join();
                return Err(format!(
                    "host-job fixture did not report a descendant: {error}"
                ));
            }
        };

        fixture
            .kill()
            .map_err(|error| format!("killing host-job owner: {error}"))?;
        fixture
            .wait()
            .map_err(|error| format!("waiting for host-job owner: {error}"))?;
        reader
            .join()
            .map_err(|_| "host-job fixture output reader failed".to_owned())?;
        assert_process_exits(&node, descendant)
    }

    fn node_binary() -> Result<String, String> {
        let output = Command::new("where.exe")
            .arg("node.exe")
            .output()
            .map_err(|error| format!("locating node.exe: {error}"))?;
        if !output.status.success() {
            return Err("node.exe is required for Windows process-tree lifecycle tests".to_owned());
        }
        String::from_utf8(output.stdout)
            .map_err(|error| error.to_string())?
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| "where.exe returned no node.exe path".to_owned())
    }

    fn assert_process_exits(node: &str, process_id: u32) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let probe = [
            "try{process.kill(Number(process.argv[1]),0);process.exit(1)}",
            "catch(error){process.exit(error.code==='ESRCH'?0:2)}",
        ]
        .join("");
        let process_id = process_id.to_string();
        while Instant::now() < deadline {
            let status = Command::new(node)
                .args(["-e", probe.as_str(), process_id.as_str()])
                .status()
                .map_err(|error| format!("probing descendant {process_id}: {error}"))?;
            if status.success() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err(format!(
            "Windows host Job Object left descendant {process_id} alive"
        ))
    }
}
