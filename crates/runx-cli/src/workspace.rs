//! `runx workspace`: the ids the runx home minted for the workspaces a person
//! runs runx in, keyed by canonical path (memory.md). A step reads its run's
//! as `$run.workspace_id`. runx can't tell a move from a copy, so a moved
//! workspace is minted a new id, and `rebind` gives it back the one it had.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use runx_runtime::WorkspaceEnv;
use runx_runtime::workspace_ids::{self, Rebound, WorkspaceIdEntry};
use serde::Serialize;

use crate::cli_args::{os_arg, split_flag};

const USAGE: &str = "runx workspace list | rebind <old path>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspacePlan {
    pub action: WorkspaceAction,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceAction {
    List,
    /// Give the workspace runx runs in the id the workspace at `old` had.
    Rebind {
        old: PathBuf,
    },
}

pub fn parse_workspace_plan(args: &[OsString]) -> Result<WorkspacePlan, String> {
    let subcommand = args
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("usage: {USAGE}"))?;
    let mut positionals = Vec::new();
    let mut json = false;
    for index in 2..args.len() {
        let token = os_arg(args, index, "workspace")?;
        let (flag, inline) = split_flag(token);
        match flag {
            "--json" | "-j" if inline.is_none() => json = true,
            _ if token.starts_with('-') => {
                return Err(format!("unknown workspace argument {token}"));
            }
            _ => positionals.push(token.to_owned()),
        }
    }
    let action = match (subcommand, positionals.as_slice()) {
        ("list", []) => WorkspaceAction::List,
        ("rebind", [old]) => WorkspaceAction::Rebind {
            old: PathBuf::from(old),
        },
        _ => return Err(format!("usage: {USAGE}")),
    };
    Ok(WorkspacePlan { action, json })
}

pub fn run_native_workspace(plan: WorkspacePlan, workspace: &WorkspaceEnv) -> ExitCode {
    match execute(&plan, workspace) {
        Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
        Err(error) if plan.json => crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(&error, "workspace_failed"),
            1,
        ),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx workspace: {error}\n"), 1),
    }
}

fn execute(plan: &WorkspacePlan, workspace: &WorkspaceEnv) -> Result<String, String> {
    let paths = workspace.paths();
    match &plan.action {
        WorkspaceAction::List => {
            let listed = workspace_ids::list(paths).map_err(|error| error.to_string())?;
            if plan.json {
                return render_json(&Output::List {
                    workspaces: &listed,
                });
            }
            if listed.is_empty() {
                return Ok(
                    "no workspace ids yet; runx mints one the first time a step reads $run.workspace_id\n"
                        .to_owned(),
                );
            }
            Ok(listed
                .iter()
                .map(|entry| format!("{}  {}\n", entry.id, entry.path))
                .collect())
        }
        WorkspaceAction::Rebind { old } => {
            let rebound = workspace_ids::rebind(paths, old).map_err(|error| error.to_string())?;
            if plan.json {
                return render_json(&Output::Rebind {
                    rebound: &RebindOutput::from(&rebound),
                });
            }
            Ok(match &rebound.replaced {
                Some(replaced) => format!(
                    "{} is {} again; the id minted for it here, {replaced}, is gone, and what was kept under it stays under it\n",
                    rebound.path, rebound.id
                ),
                None => format!("{} is {} again\n", rebound.path, rebound.id),
            })
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Output<'a> {
    List {
        workspaces: &'a [WorkspaceIdEntry],
    },
    Rebind {
        #[serde(flatten)]
        rebound: &'a RebindOutput,
    },
}

/// A rebind as JSON: the workspace, the id it has again, and the id it
/// replaced, if one was minted for it where it is now.
#[derive(Serialize)]
struct RebindOutput {
    path: String,
    id: String,
    replaced: Option<String>,
}

impl From<&Rebound> for RebindOutput {
    fn from(rebound: &Rebound) -> Self {
        Self {
            path: rebound.path.clone(),
            id: rebound.id.clone(),
            replaced: rebound.replaced.clone(),
        }
    }
}

fn render_json(output: &Output<'_>) -> Result<String, String> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        status: &'static str,
        #[serde(flatten)]
        output: &'a Output<'a>,
    }
    serde_json::to_string_pretty(&Envelope {
        status: "success",
        output,
    })
    .map(|value| format!("{value}\n"))
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(args: &[&str]) -> Result<WorkspacePlan, String> {
        parse_workspace_plan(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn each_command_parses_and_anything_else_is_refused() -> Result<(), String> {
        assert_eq!(plan(&["workspace", "list"])?.action, WorkspaceAction::List);
        assert!(plan(&["workspace", "list", "--json"])?.json);
        assert_eq!(
            plan(&["workspace", "rebind", "../old"])?.action,
            WorkspaceAction::Rebind {
                old: PathBuf::from("../old")
            }
        );
        for refused in [
            &["workspace"][..],
            &["workspace", "rebind"],
            &["workspace", "list", "extra"],
            &["workspace", "rebind", "a", "b"],
            &["workspace", "forget"],
            &["workspace", "list", "--force"],
        ] {
            assert!(plan(refused).is_err(), "{refused:?}");
        }
        Ok(())
    }
}
