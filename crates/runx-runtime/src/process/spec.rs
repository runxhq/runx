use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;

use thiserror::Error;

use super::capture::CapturedOutput;

#[derive(Clone, Debug)]
pub(crate) struct ProcessSpec {
    pub(super) label: &'static str,
    pub(super) command: String,
    pub(super) args: Vec<String>,
    pub(super) cwd: Option<PathBuf>,
    pub(super) env: BTreeMap<String, String>,
    pub(super) stdin: Option<ProcessStdin>,
    pub(super) timeout: Option<Duration>,
    pub(super) output_limit_bytes: usize,
    pub(super) cleanup_paths: Vec<PathBuf>,
}

impl ProcessSpec {
    pub(crate) fn new(
        label: &'static str,
        command: impl Into<String>,
        output_limit_bytes: usize,
    ) -> Self {
        Self {
            label,
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
            timeout: None,
            output_limit_bytes,
            cleanup_paths: Vec::new(),
        }
    }

    pub(crate) fn args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub(crate) fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub(crate) fn env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    pub(crate) fn stdin(mut self, stdin: Option<ProcessStdin>) -> Self {
        self.stdin = stdin;
        self
    }

    pub(crate) fn timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    #[cfg(feature = "process-tool")]
    pub(crate) fn cleanup_paths(mut self, cleanup_paths: Vec<PathBuf>) -> Self {
        self.cleanup_paths = cleanup_paths;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProcessStdin {
    pub(super) bytes: Vec<u8>,
    pub(super) write_context: &'static str,
}

impl ProcessStdin {
    pub(crate) fn new(bytes: Vec<u8>, write_context: &'static str) -> Self {
        Self {
            bytes,
            write_context,
        }
    }
}

#[derive(Debug)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) struct ProcessOutcome {
    pub(crate) status: ExitStatus,
    pub(crate) timed_out: bool,
    pub(crate) stdout: CapturedOutput,
    pub(crate) stderr: CapturedOutput,
    pub(crate) duration_ms: u64,
    pub(crate) cleanup_errors: Vec<String>,
}

#[derive(Debug, Error)]
pub(crate) enum ProcessSupervisorError {
    #[error("process I/O failed while {context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl ProcessSupervisorError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
