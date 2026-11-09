use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use runx_runtime::adapters::mcp::{
    McpServerExecutionOptions, McpServerOptions, McpServerToolBehavior,
};
use runx_runtime::{TurnRequest, WorkspaceEnv};

use crate::cli_args::{flag_value, optional_flag_value_or, os_flag_value, split_flag};
use crate::skill::MethodRef;

/// `runx mcp`: the one governed MCP server. It serves skills, and the turns
/// their own calls open, or the open turns of one run (`--run`), for a host
/// such as Fieldwork that hands its agent that run alone.
#[derive(Debug, Eq, PartialEq)]
pub struct McpPlan {
    /// The skills served, each `<ref>` or `<ref>#<method>`.
    pub refs: Vec<MethodRef>,
    pub run: Option<String>,
    /// The saved authority every run the server's skill calls start holds
    /// (`--authority`); with none, no run holds one.
    pub authority: Option<String>,
    pub receipt_dir: Option<PathBuf>,
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
    let mut authority = None;
    let mut receipt_dir = None;
    let mut http_listen = None;
    let mut http_allow_non_loopback = false;
    let mut index = 1;
    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            refs.push(MethodRef::parse(&args[index], "mcp")?);
            index += 1;
            continue;
        };
        if !token.starts_with('-') {
            refs.push(MethodRef::parse(OsStr::new(token), "mcp")?);
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
            "--receipt-dir" => {
                let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
                receipt_dir = Some(PathBuf::from(value));
                index = next_index;
            }
            "--authority" => {
                let (value, next_index) = flag_value(args, index, flag, inline_value, "mcp")?;
                if authority.replace(value).is_some() {
                    return Err("runx mcp takes --authority once".to_owned());
                }
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
        _ => {}
    }
    if run.is_some() && authority.is_some() {
        return Err(
            "runx mcp --run serves a run's turns under the authority the run started with, so it takes no --authority".to_owned(),
        );
    }
    Ok(McpPlan {
        refs,
        run,
        authority,
        receipt_dir,
        http_listen,
        http_allow_non_loopback,
    })
}

// Function rationale: native MCP startup owns one cohesive
// stdio-vs-HTTP transport selection and error presentation boundary.
pub fn run_native_mcp_with_workspace(mut plan: McpPlan, workspace: &WorkspaceEnv) -> ExitCode {
    // The skills served, and where their receipts go, are named from where
    // the operator typed them.
    for skill_ref in &mut plan.refs {
        if let Some(invoked) = workspace.invoked_path(&skill_ref.skill_path) {
            skill_ref.skill_path = invoked;
        }
    }
    plan.receipt_dir = plan
        .receipt_dir
        .take()
        .map(|dir| workspace.typed_path(&dir));
    if plan.run.is_some() {
        crate::runtime::end_with_host();
    }
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
    // A saved authority applies only where the server names it, and only in
    // a workspace it lists; it's never assumed.
    let run_authority = plan
        .authority
        .as_deref()
        .map(|name| crate::authority::named_authority(name, workspace))
        .transpose()?;
    let effects = crate::runtime::runtime_effect_registry().map_err(|error| error.to_string())?;
    // Each ref resolves as `runx skill` resolves it, so the server offers the
    // same skill a run would. A ref with `#method` offers that method under
    // the skill's own name; a ref without one offers its default method and
    // each other method only a connection can work.
    let mut served: Option<McpServerOptions> = None;
    let mut skill_tools = Vec::new();
    for skill_ref in &plan.refs {
        let skill_path = crate::skill::resolve_skill_package(
            &skill_ref.skill_path,
            workspace.env(),
            workspace.cwd(),
        )?;
        let mut options = McpServerOptions::from_skill_paths_with_execution(
            &[skill_path],
            "runx-cli",
            env!("CARGO_PKG_VERSION"),
            McpServerExecutionOptions {
                method: skill_ref.method.clone(),
                receipt_dir: plan.receipt_dir.clone(),
                env: workspace.env().clone(),
                effects: effects.clone(),
                run_authority: run_authority.clone(),
            },
        )
        .map_err(|error| error.to_string())?;
        let (skills, turns): (Vec<_>, Vec<_>) = std::mem::take(&mut options.tools)
            .into_iter()
            .partition(|tool| matches!(tool.result, McpServerToolBehavior::Skill(_)));
        skill_tools.extend(skills);
        // The turn tools work any run this server's calls opened, whichever
        // skill opened it, so the server offers them once.
        if served.is_none() {
            options.tools = turns;
            served = Some(options);
        }
    }
    let mut served = served.ok_or("runx mcp serves skill references")?;
    skill_tools.append(&mut served.tools);
    served.tools = skill_tools;
    Ok(served)
}
