use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use runx_runtime::WorkspaceEnv;

use crate::cli_args::{flag_value, optional_flag_value_or, os_arg, os_flag_value, split_flag};

#[derive(Debug, Eq, PartialEq)]
pub struct McpPlan {
    pub refs: Vec<PathBuf>,
    pub receipt_dir: Option<PathBuf>,
    pub runner: Option<String>,
    /// When set, serve the governed MCP server over streamable HTTP at this
    /// address instead of over stdio.
    pub http_listen: Option<String>,
    pub http_allow_non_loopback: bool,
}

// Function rationale: flag parsing is kept in one linear pass so
// CLI usage errors preserve exact native argument semantics.
pub fn parse_mcp_plan(args: &[OsString]) -> Result<McpPlan, String> {
    let subcommand = os_arg(args, 1, "mcp")?;
    if subcommand != "serve" {
        return Err(format!("unknown mcp subcommand {subcommand}"));
    }
    let mut refs = Vec::new();
    let mut receipt_dir = None;
    let mut runner = None;
    let mut http_listen = None;
    let mut http_allow_non_loopback = false;
    let mut index = 2;
    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            refs.push(PathBuf::from(args[index].clone()));
            index += 1;
            continue;
        };
        if !token.starts_with("--") {
            refs.push(PathBuf::from(token));
            index += 1;
            continue;
        }
        let (flag, inline_value) = split_flag(token);
        match flag {
            "--receipt-dir" => {
                let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
                receipt_dir = Some(PathBuf::from(value));
                index = next_index;
            }
            "--runner" => {
                let (value, next_index) = flag_value(args, index, flag, inline_value, "mcp")?;
                runner = Some(value);
                index = next_index;
            }
            "--http-listen" => {
                let (value, next_index) = optional_flag_value_or(
                    args,
                    index,
                    inline_value,
                    runx_runtime::adapters::mcp::DEFAULT_MCP_HTTP_LISTEN_ADDR,
                    "mcp",
                )?;
                http_listen = Some(value);
                index = next_index;
            }
            "--http-allow-non-loopback" => {
                if inline_value.is_some() {
                    return Err("--http-allow-non-loopback does not take a value".to_owned());
                }
                http_allow_non_loopback = true;
                index += 1;
            }
            _ => return Err(format!("unknown mcp serve flag {flag}")),
        }
    }
    if refs.is_empty() {
        return Err("runx mcp serve requires at least one skill reference.".to_owned());
    }
    Ok(McpPlan {
        refs,
        receipt_dir,
        runner,
        http_listen,
        http_allow_non_loopback,
    })
}

// Function rationale: native MCP startup owns one cohesive
// stdio-vs-HTTP transport selection and error presentation boundary.
pub fn run_native_mcp_with_workspace(plan: McpPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let options =
        match runx_runtime::adapters::mcp::McpServerOptions::from_skill_paths_with_execution(
            &plan.refs,
            "runx-cli",
            env!("CARGO_PKG_VERSION"),
            runx_runtime::adapters::mcp::McpServerExecutionOptions {
                runner: plan.runner,
                receipt_dir: plan.receipt_dir,
                env: workspace.env().clone(),
            },
        ) {
            Ok(options) => options,
            Err(error) => {
                let _ignored = writeln!(std::io::stderr(), "runx: {error}");
                return ExitCode::from(1);
            }
        };
    let result = match &plan.http_listen {
        Some(listen_addr) => {
            let bearer_token = match runx_runtime::adapters::mcp::generate_mcp_http_bearer_token() {
                Ok(token) => token,
                Err(error) => {
                    let _ignored = writeln!(std::io::stderr(), "runx: {error}");
                    return ExitCode::from(1);
                }
            };
            let _ignored = writeln!(
                std::io::stderr(),
                "runx MCP HTTP bearer token: {bearer_token}"
            );
            let _ignored = writeln!(
                std::io::stderr(),
                "runx MCP HTTP requires: Authorization: Bearer {bearer_token}"
            );
            if plan.http_allow_non_loopback {
                let _ignored = writeln!(
                    std::io::stderr(),
                    "runx MCP HTTP non-loopback listen explicitly enabled."
                );
            }
            runx_runtime::adapters::mcp::serve_mcp_http_server_blocking(
                listen_addr,
                options,
                runx_runtime::adapters::mcp::McpHttpServerSecurity {
                    bearer_token,
                    allow_non_loopback: plan.http_allow_non_loopback,
                },
            )
        }
        None => runx_runtime::adapters::mcp::serve_mcp_json_rpc(
            std::io::stdin(),
            std::io::stdout(),
            options,
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ignored = writeln!(std::io::stderr(), "runx: {error}");
            ExitCode::from(1)
        }
    }
}
