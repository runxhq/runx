//! Runtime-owned preparation for trusted host processes and the deterministic
//! JavaScript worker.
//!
//! This module controls exact argv, cwd, delivered environment, runtime input
//! files, cleanup, and observed execution-boundary evidence. It does not claim
//! filesystem, network, or syscall confinement for host processes.

mod environment;
mod paths;
mod template;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{ExecutionBoundaryKind, JsonObject};
use runx_parser::ProcessTool;

use crate::RuntimeError;

use self::environment::child_base_env;
use self::environment::child_env;
use self::paths::{execution_workspace_root, resolve_cwd, resolved_skill_directory, workspace_cwd};
use self::template::resolve_template;

pub(crate) const NATIVE_COMMAND_EXECUTION_BOUNDARY: ExecutionBoundaryKind =
    ExecutionBoundaryKind::TrustedHostProcess;

#[derive(PartialEq)]
pub struct PreparedProcessInvocation {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    /// Where the prepared process will run.
    pub boundary: ExecutionBoundaryKind,
    pub cleanup_paths: Vec<PathBuf>,
}

impl std::fmt::Debug for PreparedProcessInvocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedProcessInvocation")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env_names", &self.env.keys().collect::<Vec<_>>())
            .field("boundary", &self.boundary)
            .field("cleanup_paths", &self.cleanup_paths)
            .finish()
    }
}

pub(crate) struct ProcessExecutionPlan {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) boundary: ExecutionBoundaryKind,
    pub(crate) cleanup_paths: Vec<PathBuf>,
}

pub(crate) struct NativeCommandInvocationRequest<'a> {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: &'a Path,
    pub(crate) workspace_root: &'a Path,
    pub(crate) explicit_env: &'a BTreeMap<String, String>,
    pub(crate) base_env: &'a BTreeMap<String, String>,
}

impl PreparedProcessInvocation {
    pub(crate) fn into_execution_plan(mut self) -> ProcessExecutionPlan {
        ProcessExecutionPlan {
            command: std::mem::take(&mut self.command),
            args: std::mem::take(&mut self.args),
            cwd: std::mem::take(&mut self.cwd),
            env: std::mem::take(&mut self.env),
            boundary: self.boundary,
            cleanup_paths: std::mem::take(&mut self.cleanup_paths),
        }
    }
}

impl Drop for PreparedProcessInvocation {
    fn drop(&mut self) {
        crate::process::cleanup_paths_quietly(&self.cleanup_paths);
    }
}

pub(crate) fn process_base_environment(
    base_env: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    child_base_env(base_env)
}

/// The workspace a host process runs in: the run's workspace, else the
/// runtime's own working directory.
#[cfg(feature = "mcp")]
pub(crate) fn process_workspace(
    base_env: &BTreeMap<String, String>,
) -> Result<std::path::PathBuf, RuntimeError> {
    match workspace_cwd(base_env)? {
        Some(cwd) => Ok(cwd),
        None => std::env::current_dir().map_err(|error| RuntimeError::InvalidRunStep {
            step_id: "mcp".to_owned(),
            reason: error.to_string(),
        }),
    }
}

pub fn prepare_process_invocation(
    tool: &ProcessTool,
    skill_directory: &Path,
    inputs: &JsonObject,
    base_env: &BTreeMap<String, String>,
) -> Result<PreparedProcessInvocation, RuntimeError> {
    let command = tool.command.clone();
    let workspace_cwd = workspace_cwd(base_env)?;
    let skill_directory = resolved_skill_directory(skill_directory, workspace_cwd.as_deref())?;
    let workspace_root = execution_workspace_root(workspace_cwd.as_deref(), &skill_directory);
    let cwd = resolve_cwd(
        tool.cwd.as_deref(),
        &skill_directory,
        workspace_cwd.as_deref(),
    )?;
    let mut invocation_env = base_env.clone();
    invocation_env.insert(
        crate::paths::RUNX_CWD_ENV.to_owned(),
        workspace_root.to_string_lossy().into_owned(),
    );
    let declared_environment =
        crate::execution_environment::resolve_environment(&tool.environment, &invocation_env)?;
    let args = tool
        .args
        .iter()
        .map(|arg| resolve_template(arg, inputs, &declared_environment))
        .collect();
    let mut cleanup_paths = Vec::new();
    let env = match child_env(
        &declared_environment,
        &invocation_env,
        inputs,
        &mut cleanup_paths,
    ) {
        Ok(env) => env,
        Err(error) => {
            crate::process::cleanup_paths_quietly(&cleanup_paths);
            return Err(error);
        }
    };
    prepare_exact_process_invocation(
        command,
        args,
        cwd,
        env,
        cleanup_paths,
        ExecutionBoundaryKind::TrustedHostProcess,
    )
}

pub(crate) fn prepare_native_command_invocation(
    request: NativeCommandInvocationRequest<'_>,
) -> Result<PreparedProcessInvocation, RuntimeError> {
    let mut invocation_env = request.base_env.clone();
    invocation_env.insert(
        crate::paths::RUNX_CWD_ENV.to_owned(),
        request.workspace_root.to_string_lossy().into_owned(),
    );
    let mut env = child_base_env(&invocation_env)?;
    env.extend(request.explicit_env.clone());
    prepare_exact_process_invocation(
        request.command,
        request.args,
        request.cwd.to_path_buf(),
        env,
        Vec::new(),
        NATIVE_COMMAND_EXECUTION_BOUNDARY,
    )
}

pub(crate) fn prepare_javascript_worker_invocation(
    worker_path: &Path,
) -> Result<PreparedProcessInvocation, RuntimeError> {
    let cwd = std::fs::canonicalize(std::env::temp_dir())
        .map_err(|source| RuntimeError::io("resolving deterministic worker cwd", source))?;
    prepare_exact_process_invocation(
        worker_path.to_string_lossy().into_owned(),
        Vec::new(),
        cwd,
        BTreeMap::new(),
        Vec::new(),
        ExecutionBoundaryKind::DeterministicWorker,
    )
}

pub(crate) fn prepare_exact_process_invocation(
    command: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    cleanup_paths: Vec<PathBuf>,
    boundary: ExecutionBoundaryKind,
) -> Result<PreparedProcessInvocation, RuntimeError> {
    if command.trim().is_empty() {
        return Err(RuntimeError::InvalidProcessInvocation {
            message: "process command must not be empty".to_owned(),
        });
    }
    if !cwd.is_absolute() {
        return Err(RuntimeError::InvalidProcessInvocation {
            message: format!("process cwd must be absolute, got '{}'", cwd.display()),
        });
    }
    Ok(PreparedProcessInvocation {
        command,
        args,
        cwd,
        env,
        boundary,
        cleanup_paths,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use runx_contracts::{EnvironmentRequirements, ExecutionBoundaryKind, JsonObject, JsonValue};

    use super::prepare_process_invocation;

    #[test]
    fn trusted_host_plan_preserves_declared_environment_and_exact_argv()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let tool = runx_parser::ProcessTool {
            command: "/usr/bin/printf".to_owned(),
            args: vec!["{{message}}".to_owned()],
            cwd: None,
            timeout_seconds: Some(5),
            input_mode: None,
            environment: EnvironmentRequirements {
                required: vec!["REGION".to_owned()],
                optional: Vec::new(),
            },
        };
        let inputs =
            JsonObject::from([("message".to_owned(), JsonValue::String("hello".to_owned()))]);
        let base_env = BTreeMap::from([
            (
                crate::paths::RUNX_CWD_ENV.to_owned(),
                workspace.path().to_string_lossy().into_owned(),
            ),
            ("REGION".to_owned(), "ap-southeast-2".to_owned()),
            ("UNDECLARED".to_owned(), "blocked".to_owned()),
        ]);

        let plan = prepare_process_invocation(&tool, workspace.path(), &inputs, &base_env)?;

        assert_eq!(plan.command, "/usr/bin/printf");
        assert_eq!(plan.args, ["hello"]);
        assert_eq!(
            plan.env.get("REGION").map(String::as_str),
            Some("ap-southeast-2")
        );
        assert!(!plan.env.contains_key("UNDECLARED"));
        let debug = format!("{plan:?}");
        assert!(debug.contains("REGION"));
        assert!(!debug.contains("ap-southeast-2"));
        assert!(!debug.contains("blocked"));
        assert_eq!(plan.boundary, ExecutionBoundaryKind::TrustedHostProcess);
        Ok(())
    }
}
