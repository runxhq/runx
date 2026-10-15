//! Exact argv admission for the tools that run a process: the project root,
//! the executable, its arguments, the working directory inside the root, and
//! the explicit environment, each bounded. No shell expands any of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{NativeInvocation, invalid_input, resolve_repo_root_for};
use crate::RuntimeError;

const MAX_COMMAND_ARGS: usize = 128;
const MAX_COMMAND_ARG_BYTES: usize = 8 * 1024;
const MAX_ENV: usize = 64;
const MAX_ENV_VALUE_BYTES: usize = 8 * 1024;

/// What a call asks to run.
pub(super) struct ArgvRequest<'a> {
    pub(super) repo_root: &'a str,
    pub(super) command: &'a str,
    pub(super) args: &'a [String],
    pub(super) cwd: &'a str,
    pub(super) env: &'a BTreeMap<String, String>,
}

/// An argv a tool admitted.
pub(super) struct Argv {
    pub(super) repo_root: PathBuf,
    pub(super) command: String,
    pub(super) args: Vec<String>,
    pub(super) cwd: PathBuf,
    /// `cwd` relative to `repo_root`, `.` for the root itself.
    pub(super) cwd_relative: String,
    pub(super) explicit_env: BTreeMap<String, String>,
}

impl Argv {
    /// The executable and its arguments.
    pub(super) fn argv(&self) -> Vec<String> {
        std::iter::once(self.command.clone())
            .chain(self.args.iter().cloned())
            .collect()
    }
}

/// Admit `request` for `tool`, within the roots `invocation` may reach.
pub(super) fn admit<I: ?Sized>(
    tool: &str,
    invocation: &NativeInvocation<'_, I>,
    request: ArgvRequest<'_>,
) -> Result<Argv, RuntimeError> {
    let repo_root = resolve_repo_root_for(
        tool,
        request.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let command = bounded_text(tool, request.command, "command", MAX_COMMAND_ARG_BYTES)?;
    let args = arguments(tool, request.args)?;
    let cwd = working_directory(tool, request.cwd, &repo_root)?;
    let cwd_relative = relative_path(tool, &repo_root, &cwd)?;
    let explicit_env = explicit_env(tool, request.env)?;
    Ok(Argv {
        repo_root,
        command,
        args,
        cwd,
        cwd_relative,
        explicit_env,
    })
}

fn arguments(tool: &str, values: &[String]) -> Result<Vec<String>, RuntimeError> {
    if values.len() > MAX_COMMAND_ARGS {
        return Err(invalid_input(
            tool,
            format!("args must contain no more than {MAX_COMMAND_ARGS} entries"),
        ));
    }
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            bounded_text(
                tool,
                value,
                &format!("args[{index}]"),
                MAX_COMMAND_ARG_BYTES,
            )
        })
        .collect()
}

fn explicit_env(
    tool: &str,
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    if values.len() > MAX_ENV {
        return Err(invalid_input(
            tool,
            format!("env must contain no more than {MAX_ENV} entries"),
        ));
    }
    values
        .iter()
        .map(|(name, value)| {
            if !valid_env_name(name) {
                return Err(invalid_input(tool, format!("invalid env name {name:?}")));
            }
            Ok((
                name.clone(),
                bounded_text(tool, value, &format!("env.{name}"), MAX_ENV_VALUE_BYTES)?,
            ))
        })
        .collect()
}

fn working_directory(tool: &str, requested: &str, root: &Path) -> Result<PathBuf, RuntimeError> {
    let requested_path = Path::new(requested);
    if requested_path.is_absolute() {
        return Err(invalid_input(tool, "cwd must be relative to repo_root"));
    }
    let candidate = std::fs::canonicalize(root.join(requested_path))
        .map_err(|source| RuntimeError::io("resolving command cwd", source))?;
    if candidate != root && !candidate.starts_with(root) {
        return Err(invalid_input(tool, "cwd escapes repo_root"));
    }
    if !candidate.is_dir() {
        return Err(invalid_input(tool, "cwd must resolve to a directory"));
    }
    Ok(candidate)
}

pub(super) fn bounded_text(
    tool: &str,
    value: &str,
    field: &str,
    max_bytes: usize,
) -> Result<String, RuntimeError> {
    if value.is_empty() {
        return Err(invalid_input(tool, format!("{field} must not be empty")));
    }
    if value.len() > max_bytes {
        return Err(invalid_input(
            tool,
            format!("{field} exceeds {max_bytes} bytes"),
        ));
    }
    if value.contains('\0') {
        return Err(invalid_input(tool, format!("{field} contains NUL")));
    }
    Ok(value.to_owned())
}

fn relative_path(tool: &str, root: &Path, path: &Path) -> Result<String, RuntimeError> {
    path.strip_prefix(root)
        .map(|value| {
            let value = value
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            if value.is_empty() {
                ".".to_owned()
            } else {
                value
            }
        })
        .map_err(|_| invalid_input(tool, "cwd escapes repo_root"))
}

fn valid_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z'))
        && chars.all(|character| matches!(character, '_' | 'A'..='Z' | '0'..='9'))
}
