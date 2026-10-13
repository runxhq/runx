use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use runx_runtime::{TurnRequest, WorkspaceEnv};

use crate::cli_args::{flag_value, optional_flag_value_or, os_flag_value, split_flag};

/// `runx mcp`: the one governed MCP server. It serves skills, and the turns
/// their own calls open, or the open turns of one run (`--run`), for a host
/// such as Fieldwork that hands its agent that run alone.
#[derive(Debug, Eq, PartialEq)]
pub struct McpPlan {
    pub refs: Vec<PathBuf>,
    pub run: Option<String>,
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
    let mut refs = Vec::new();
    let mut run = None;
    let mut receipt_dir = None;
    let mut runner = None;
    let mut http_listen = None;
    let mut http_allow_non_loopback = false;
    let mut index = 1;
    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            refs.push(PathBuf::from(args[index].clone()));
            index += 1;
            continue;
        };
        if !token.starts_with('-') {
            refs.push(PathBuf::from(token));
            index += 1;
            continue;
        }
        let (flag, inline_value) = split_flag(token);
        match flag {
            "--run" => {
                let (value, next_index) = flag_value(args, index, flag, inline_value, "mcp")?;
                run = Some(value);
                index = next_index;
            }
            "--receipt-dir" | "-R" => {
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
            _ => return Err(format!("unknown mcp flag {flag}")),
        }
    }
    match (&run, refs.is_empty()) {
        (None, true) => {
            return Err(
                "runx mcp serves skill references, or one run's turns with --run <run-id>"
                    .to_owned(),
            );
        }
        (Some(_), false) => {
            return Err(
                "runx mcp serves skill references or one run's turns, not both: a skill server already works the turns its own calls open".to_owned(),
            );
        }
        (Some(_), true) if runner.is_some() => {
            return Err(
                "runx mcp --runner selects a skill's runner; --run serves a run".to_owned(),
            );
        }
        _ => {}
    }
    Ok(McpPlan {
        refs,
        run,
        receipt_dir,
        runner,
        http_listen,
        http_allow_non_loopback,
    })
}

// Function rationale: native MCP startup owns one cohesive
// stdio-vs-HTTP transport selection and error presentation boundary.
pub fn run_native_mcp_with_workspace(plan: McpPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let options = match server_options(&plan, workspace) {
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

/// The tools the server offers: one run's open turns, or the skills and the
/// turns their own calls open.
fn server_options(
    plan: &McpPlan,
    workspace: &WorkspaceEnv,
) -> Result<runx_runtime::adapters::mcp::McpServerOptions, String> {
    if let Some(run_id) = &plan.run {
        let session = crate::runtime::local_orchestrator(workspace.env())
            .map_err(|error| error.to_string())?
            .open_turns(&TurnRequest {
                run_id: run_id.clone(),
                receipt_dir: plan.receipt_dir.clone(),
                env: workspace.env().clone(),
                cwd: workspace.cwd().to_path_buf(),
            })
            .map_err(|error| error.to_string())?;
        return Ok(runx_runtime::adapters::mcp::McpServerOptions::for_turns(
            session,
            "runx-cli",
            env!("CARGO_PKG_VERSION"),
        ));
    }
    let effects = crate::runtime::runtime_effect_registry().map_err(|error| error.to_string())?;
    // Each ref resolves as `runx skill` resolves it, so the server offers the
    // same skill a run would.
    let skill_paths = plan
        .refs
        .iter()
        .map(|skill_ref| {
            crate::skill::resolve_skill_package(skill_ref, workspace.env(), workspace.cwd())
        })
        .collect::<Result<Vec<_>, _>>()?;
    runx_runtime::adapters::mcp::McpServerOptions::from_skill_paths_with_execution(
        &skill_paths,
        "runx-cli",
        env!("CARGO_PKG_VERSION"),
        runx_runtime::adapters::mcp::McpServerExecutionOptions {
            runner: plan.runner.clone(),
            receipt_dir: plan.receipt_dir.clone(),
            env: workspace.env().clone(),
            effects,
        },
    )
    .map_err(|error| error.to_string())
}
