//! `runx turn <run-id>`: the agent turn a paused run waits on, served as MCP
//! tools over stdio. Each tool call is made through the run: admitted against
//! the turn's allowance and the run's authority, executed, and recorded.
//! `turn_complete` ends the turn and the run continues.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use runx_runtime::WorkspaceEnv;

use crate::cli_args::{os_arg, os_flag_value, split_flag};

#[derive(Debug, Eq, PartialEq)]
pub struct TurnPlan {
    pub run_id: String,
    pub receipt_dir: Option<PathBuf>,
}

pub fn parse_turn_plan(args: &[OsString]) -> Result<TurnPlan, String> {
    if args.len() < 2 {
        return Err("runx turn requires <run-id>".to_owned());
    }
    let run_id = os_arg(args, 1, "turn")?.to_owned();
    let mut receipt_dir = None;
    let mut index = 2;
    while index < args.len() {
        let token = args[index]
            .to_str()
            .ok_or_else(|| "runx turn arguments must be UTF-8".to_owned())?;
        let (flag, inline_value) = split_flag(token);
        match flag {
            "--receipt-dir" => {
                let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
                receipt_dir = Some(PathBuf::from(value));
                index = next_index;
            }
            _ => return Err(format!("unknown turn argument {token}")),
        }
    }
    Ok(TurnPlan {
        run_id,
        receipt_dir,
    })
}

pub fn run_native_turn_with_workspace(plan: TurnPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let options = crate::runtime::runtime_effect_registry()
        .map_err(|error| error.to_string())
        .and_then(|effects| {
            runx_runtime::adapters::mcp::McpServerOptions::for_turn(
                &plan.run_id,
                workspace.cwd().to_path_buf(),
                runx_runtime::adapters::mcp::McpServerExecutionOptions {
                    runner: None,
                    receipt_dir: plan.receipt_dir,
                    env: workspace.env().clone(),
                    effects,
                },
                "runx-cli",
                env!("CARGO_PKG_VERSION"),
            )
            .map_err(|error| error.to_string())
        });
    let options = match options {
        Ok(options) => options,
        Err(error) => {
            let _ignored = writeln!(std::io::stderr(), "runx: {error}");
            return ExitCode::from(1);
        }
    };
    match runx_runtime::adapters::mcp::serve_mcp_json_rpc(
        std::io::stdin(),
        std::io::stdout(),
        options,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ignored = writeln!(std::io::stderr(), "runx: {error}");
            ExitCode::from(1)
        }
    }
}
