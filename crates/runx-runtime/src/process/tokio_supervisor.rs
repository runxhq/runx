use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;

use thiserror::Error;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};

#[derive(Clone, Debug)]
pub(crate) struct TokioProcessSpec {
    pub(crate) label: &'static str,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: BTreeMap<String, String>,
}

impl TokioProcessSpec {
    pub(crate) fn new(
        label: &'static str,
        command: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            label,
            command: command.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
        }
    }

    pub(crate) fn args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub(crate) fn env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }
}

#[derive(Debug, Error)]
pub(crate) enum TokioProcessSupervisorError {
    #[error("failed to spawn {label} command '{command}' in cwd '{cwd}': {source}")]
    Spawn {
        label: &'static str,
        command: String,
        cwd: String,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) fn spawn_tokio_process(
    spec: TokioProcessSpec,
) -> Result<OwnedTokioProcess, TokioProcessSupervisorError> {
    super::ensure_windows_host_job().map_err(|source| TokioProcessSupervisorError::Spawn {
        label: spec.label,
        command: spec.command.clone(),
        cwd: spec.cwd.display().to_string(),
        source,
    })?;
    let mut command = tokio::process::Command::new(&spec.command);
    let cwd = super::child_process_cwd(&spec.cwd);
    command
        .args(&spec.args)
        .current_dir(cwd.as_ref())
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_group(&mut command);
    OwnedTokioProcess::spawn(command).map_err(|source| TokioProcessSupervisorError::Spawn {
        label: spec.label,
        command: spec.command,
        cwd: spec.cwd.display().to_string(),
        source,
    })
}

#[cfg(unix)]
fn configure_process_group(command: &mut tokio::process::Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut tokio::process::Command) {}

/// Owns one asynchronously supervised subprocess tree.
///
/// Unix gives every child a dedicated process group. Windows creates the child
/// suspended, assigns it to a per-execution Job Object, and only then resumes
/// it. The wrapper remains intact for the complete MCP session so stopping the
/// session retains tree ownership.
pub(crate) struct OwnedTokioProcess {
    #[cfg(not(windows))]
    child: tokio::process::Child,
    #[cfg(windows)]
    child: Box<dyn process_wrap::tokio::ChildWrapper>,
}

impl OwnedTokioProcess {
    fn spawn(command: tokio::process::Command) -> io::Result<Self> {
        #[cfg(not(windows))]
        {
            let mut command = command;
            super::with_spawn_lock(|| command.spawn()).map(|child| Self { child })
        }

        #[cfg(windows)]
        {
            use process_wrap::tokio::{CommandWrap, JobObject};

            let mut wrapped = CommandWrap::from(command);
            wrapped.wrap(JobObject);
            wrapped.spawn().map(|child| Self { child })
        }
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        #[cfg(not(windows))]
        {
            self.child.stdin.take()
        }
        #[cfg(windows)]
        {
            self.child.stdin().take()
        }
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        #[cfg(not(windows))]
        {
            self.child.stdout.take()
        }
        #[cfg(windows)]
        {
            self.child.stdout().take()
        }
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        #[cfg(not(windows))]
        {
            self.child.stderr.take()
        }
        #[cfg(windows)]
        {
            self.child.stderr().take()
        }
    }

    pub(crate) fn start_kill(&mut self) -> io::Result<()> {
        self.child.start_kill()
    }
}
