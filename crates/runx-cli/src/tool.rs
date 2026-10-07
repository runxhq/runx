// Module rationale: command wiring keeps tool build/search/inspect output parity together.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_runtime::{
    ToolBuildOptions, ToolCatalogError, ToolInspectOptions, ToolSearchOptions, WorkspaceEnv,
    build_tool_catalogs, inspect_tool_with_effects, search_tools_with_effects,
};

use crate::router::{ToolAction, ToolPlan};

pub fn run_native_tool(plan: ToolPlan, workspace: &WorkspaceEnv) -> ExitCode {
    match run_tool(plan, workspace) {
        Ok(output) => crate::cli_io::write_stdout_code(&output.stdout, output.exit_code),
        Err(error) => {
            let _ignored = crate::cli_io::write_stderr_code(&render_cli_error(&error.to_string()));
            ExitCode::from(error.exit_code())
        }
    }
}

struct ToolCliOutput {
    stdout: String,
    exit_code: u8,
}

fn run_tool(plan: ToolPlan, workspace: &WorkspaceEnv) -> Result<ToolCliOutput, ToolCliError> {
    match plan.action {
        ToolAction::Build => run_build(plan, workspace.env(), workspace.cwd()),
        ToolAction::Search => run_search(plan),
        ToolAction::Inspect => run_inspect(plan, workspace.env(), workspace.cwd()),
    }
}

fn run_build(
    plan: ToolPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ToolCliOutput, ToolCliError> {
    let root = runx_runtime::resolve_runx_workspace_base(env, cwd);
    let tool_path = plan
        .path
        .as_deref()
        .map(|path| resolve_user_path(path, env, cwd));
    let report = build_tool_catalogs(&ToolBuildOptions {
        root,
        tool_path,
        all: plan.all,
    })?;
    let stdout = if plan.json {
        json_line(&report)?
    } else {
        render_build_report(report.built.len(), &report.errors)
    };
    let exit_code = if report.status == runx_contracts::tools::ToolBuildStatus::Success {
        0
    } else {
        1
    };
    Ok(ToolCliOutput { stdout, exit_code })
}

fn run_search(plan: ToolPlan) -> Result<ToolCliOutput, ToolCliError> {
    let query = plan
        .ref_or_query
        .ok_or_else(|| ToolCliError::Usage("runx tool search requires a query".to_owned()))?;
    let effects = crate::runtime::runtime_effect_registry().map_err(|error| {
        ToolCliError::Internal(format!("failed to initialize runtime effects: {error}"))
    })?;
    let report = search_tools_with_effects(
        &ToolSearchOptions {
            query,
            source: plan.source,
            limit: 20,
        },
        &effects,
    );
    let stdout = if plan.json {
        json_line(&report)?
    } else {
        render_search_results(&report.results)
    };
    Ok(ToolCliOutput {
        stdout,
        exit_code: 0,
    })
}

fn run_inspect(
    plan: ToolPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ToolCliOutput, ToolCliError> {
    let tool_ref = plan.ref_or_query.ok_or_else(|| {
        ToolCliError::Usage("runx tool inspect requires a tool reference".to_owned())
    })?;
    let root = runx_runtime::resolve_runx_workspace_base(env, cwd);
    let search_from_directory = resolve_user_path(Path::new("."), env, cwd);
    let tool_roots = env_value(env, "RUNX_TOOL_ROOTS")
        .map(|value| split_env_paths(&value))
        .unwrap_or_default();
    let effects = crate::runtime::runtime_effect_registry().map_err(|error| {
        ToolCliError::Internal(format!("failed to initialize runtime effects: {error}"))
    })?;
    let report = inspect_tool_with_effects(
        &ToolInspectOptions {
            root,
            tool_ref,
            source: plan.source,
            search_from_directory,
            tool_roots,
            allow_explicit_manifest_path: true,
        },
        &effects,
    )?;
    let stdout = if plan.json {
        json_line(&report)?
    } else {
        render_inspect_result(&report.tool)
    };
    Ok(ToolCliOutput {
        stdout,
        exit_code: 0,
    })
}

fn env_value(env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    env.get(key).cloned()
}

fn resolve_user_path(user_path: &Path, env: &BTreeMap<String, String>, cwd: &Path) -> PathBuf {
    runx_runtime::resolve_path_from_user_input(&user_path.to_string_lossy(), env, cwd, true)
}

fn split_env_paths(value: &str) -> Vec<PathBuf> {
    std::env::split_paths(value).collect()
}

fn json_line<T: serde::Serialize>(value: &T) -> Result<String, ToolCliError> {
    serde_json::to_string_pretty(value)
        .map(|json| format!("{json}\n"))
        .map_err(|error| ToolCliError::Internal(error.to_string()))
}

fn render_build_report(count: usize, errors: &[String]) -> String {
    let mut lines = vec!["".to_owned(), format!("  tool build  {count} tool(s)")];
    for error in errors {
        lines.push(format!("  {error}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn render_search_results(results: &[runx_contracts::tools::ToolCatalogSearchResult]) -> String {
    if results.is_empty() {
        return "\n  No tools found.\n\n".to_owned();
    }
    let mut lines = vec!["".to_owned(), "  Tools".to_owned()];
    for result in results {
        lines.push(format!("  {}  {}", result.name, result.source_label));
        lines.push(format!("  type      {}", result.source_type));
        lines.push(format!("  namespace {}", result.namespace));
        lines.push(format!("  external  {}", result.external_name));
        lines.push(format!("  catalog   {}", result.catalog_ref));
        if !result.required_scopes.is_empty() {
            lines.push(format!("  scopes    {}", result.required_scopes.join(", ")));
        }
        if let Some(summary) = &result.summary {
            lines.push(format!("  summary   {summary}"));
        }
        lines.push(String::new());
    }
    format!("{}\n", lines.join("\n"))
}

fn render_inspect_result(result: &runx_contracts::tools::ToolInspectResult) -> String {
    let mut lines = inspect_header_lines(result);
    if !result.scopes.is_empty() {
        lines.push(format!("  scopes    {}", result.scopes.join(", ")));
    }
    if let Some(description) = &result.description {
        lines.push(format!("  summary   {description}"));
    }
    if !result.inputs.is_empty() {
        lines.push("  inputs".to_owned());
        lines.extend(input_lines(result));
    }
    lines.push(String::new());
    format!("{}\n", lines.join("\n"))
}

fn inspect_header_lines(result: &runx_contracts::tools::ToolInspectResult) -> Vec<String> {
    let origin = match result.provenance.origin {
        runx_contracts::tools::ToolInspectOrigin::Local => "local",
        runx_contracts::tools::ToolInspectOrigin::Native => "native",
    };
    vec![
        String::new(),
        format!("  {}  {origin}", result.name),
        format!("  exec      {}", result.execution_source_type),
        format!("  path      {}", result.reference_path),
        format!("  root      {}", result.skill_directory),
    ]
}

fn input_lines(result: &runx_contracts::tools::ToolInspectResult) -> Vec<String> {
    result
        .inputs
        .iter()
        .map(|(name, input)| {
            let required = if input.required {
                "required"
            } else {
                "optional"
            };
            let description = input
                .description
                .as_ref()
                .map(|value| format!(" · {value}"))
                .unwrap_or_default();
            format!("    {name}: {} · {required}{description}", input.input_type)
        })
        .collect()
}

fn render_cli_error(message: &str) -> String {
    format!("\n  ✗  {message}\n")
}

#[derive(Debug)]
enum ToolCliError {
    Usage(String),
    Runtime(ToolCatalogError),
    Internal(String),
}

impl ToolCliError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => 2,
            Self::Runtime(_) | Self::Internal(_) => 1,
        }
    }
}

impl std::fmt::Display for ToolCliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) | Self::Internal(message) => formatter.write_str(message),
            Self::Runtime(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<ToolCatalogError> for ToolCliError {
    fn from(value: ToolCatalogError) -> Self {
        Self::Runtime(value)
    }
}
