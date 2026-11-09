// Module rationale: router argument handling is centralized for CLI routing tests.
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::cli_args::{flag_value, os_arg, os_flag_value, split_flag};
use crate::config::ConfigPlan;
use crate::connector::ConnectorPlan;
use crate::export::ExportPlan;
use crate::login::LoginPlan;
use crate::mcp::McpPlan;
use crate::notarize::NotarizePlan;
use crate::registry::{RegistryAction, RegistryPlan};
use crate::resume::ResumePlan;
use crate::secret::SecretPlan;
use crate::skill::SkillPlan;
use runx_contracts::validate_registry_version;
use runx_runtime::WorkspaceEnv;
use runx_runtime::registry::parse_registry_ref;

#[derive(Debug, PartialEq)]
pub enum RouterAction {
    Error(String),
    JsonError(JsonErrorPlan),
    RunDoctor(DoctorPlan),
    RunExport(ExportPlan),
    RunList(ListPlan),
    RunLogin(LoginPlan),
    RunMcp(McpPlan),
    RunHistory(HistoryPlan),
    RunProcess(ProcessPlan),
    RunVerify(VerifyPlan),
    RunTest(TestPlan),
    RunConfig(ConfigPlan),
    RunConnector(ConnectorPlan),
    RunNotarize(NotarizePlan),
    RunRegistry(RegistryPlan),
    RunResume(ResumePlan),
    RunSecret(SecretPlan),
    RunSkill(Box<SkillPlan>),
    RunTool(ToolPlan),
    PrintHelp,
    PrintHelpJson,
    PrintCommandHelp(&'static str),
    PrintCommandUsageError(&'static str),
    PrintVersion,
}

#[derive(Debug, Eq, PartialEq)]
pub struct JsonErrorPlan {
    pub message: String,
    pub code: String,
    pub exit_code: u8,
}

#[derive(Debug, Eq, PartialEq)]
pub struct TestPlan {
    /// What to test: a package (its directory or `SKILL.md`), whose
    /// `tests/*.yaml` run, or test files.
    pub fixture_paths: Vec<OsString>,
    /// Where receipts the cases seal are written (`--receipt-dir`).
    pub receipt_dir: Option<OsString>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct HistoryPlan {
    pub args: Vec<OsString>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProcessPlan {
    pub args: Vec<OsString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyPlan {
    pub args: Vec<OsString>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DoctorPlan {
    pub mode: DoctorMode,
    pub path: Option<PathBuf>,
    pub json: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub enum DoctorMode {
    Workspace,
    Authority,
    Registry,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ListPlan {
    pub kind: ListKind,
    pub filter: FilterMode,
    pub json: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FilterMode {
    #[default]
    All,
    Valid,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListKind {
    All,
    Tools,
    Skills,
    Packets,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ToolPlan {
    pub action: ToolAction,
    pub ref_or_query: String,
    pub source: Option<String>,
    pub json: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ToolAction {
    Search,
    Inspect,
}

// Function rationale: router routing is the cutover gate:
// every native command branch and fail-closed decision is reviewed here.
pub fn route_args(args: Vec<OsString>) -> RouterAction {
    route_args_with_optional_workspace(args, None)
}

pub fn route_args_with_workspace(args: Vec<OsString>, workspace: &WorkspaceEnv) -> RouterAction {
    route_args_with_optional_workspace(args, Some(workspace))
}

// Function rationale: router routing is the cutover gate:
// every native command branch and fail-closed decision is reviewed here.
fn route_args_with_optional_workspace(
    args: Vec<OsString>,
    workspace: Option<&WorkspaceEnv>,
) -> RouterAction {
    if root_help_json_requested(&args) {
        return RouterAction::PrintHelpJson;
    }

    if args.is_empty() || single_arg_is(&args, "--help") || single_arg_is(&args, "-h") {
        return RouterAction::PrintHelp;
    }

    if single_arg_is(&args, "--version") || single_arg_is(&args, "-V") {
        return RouterAction::PrintVersion;
    }

    if let Some(spec) = documented_command(&args)
        && nested_help_requested(&args)
    {
        return RouterAction::PrintCommandHelp(spec.name);
    }

    if first_arg_is(&args, "test") {
        return normalize_json_error(&args, native_test_plan(&args));
    }

    if first_arg_is(&args, "config") {
        return route_parse(
            &args,
            crate::config::parse_config_plan(&args),
            RouterAction::RunConfig,
        );
    }

    if first_arg_is(&args, "connector") {
        return route_parse(
            &args,
            crate::connector::parse_connector_plan(&args),
            RouterAction::RunConnector,
        );
    }

    if first_arg_is(&args, "secret") {
        return route_parse(
            &args,
            crate::secret::parse_secret_plan(&args),
            RouterAction::RunSecret,
        );
    }

    if first_arg_is(&args, "login") {
        return route_parse(
            &args,
            crate::login::parse_login_plan(&args),
            RouterAction::RunLogin,
        );
    }

    if first_arg_is(&args, "notarize") {
        return route_parse(
            &args,
            crate::notarize::parse_notarize_plan(&args),
            RouterAction::RunNotarize,
        );
    }

    if first_arg_is(&args, "doctor") {
        return route_parse(&args, parse_doctor_plan(&args), RouterAction::RunDoctor);
    }

    if first_arg_is(&args, "export") {
        return route_parse(
            &args,
            crate::export::parse_export_plan(&args),
            RouterAction::RunExport,
        );
    }

    if first_arg_is(&args, "list") {
        return route_parse(&args, parse_list_plan(&args), RouterAction::RunList);
    }

    if first_arg_is(&args, "history") {
        return RouterAction::RunHistory(HistoryPlan { args });
    }

    if first_arg_is(&args, "process") {
        return RouterAction::RunProcess(ProcessPlan { args });
    }

    if first_arg_is(&args, "resume") {
        return crate::resume::parse_resume_plan(&args).map_or_else(
            |message| json_or_human_error(&args, message),
            RouterAction::RunResume,
        );
    }

    if first_arg_is(&args, "verify") {
        return RouterAction::RunVerify(VerifyPlan { args });
    }

    if first_arg_is(&args, "mcp") {
        return crate::mcp::parse_mcp_plan(&args)
            .map_or_else(RouterAction::Error, RouterAction::RunMcp);
    }

    if first_arg_is(&args, "tool") {
        return route_parse(&args, parse_tool_plan(&args), RouterAction::RunTool);
    }

    if first_arg_is(&args, "registry") {
        if args.len() == 1 {
            return RouterAction::PrintCommandUsageError("registry");
        }
        return parse_registry_plan(&args).map_or_else(
            |message| json_or_human_error(&args, message),
            RouterAction::RunRegistry,
        );
    }

    if first_arg_is(&args, "skill") {
        let parsed = workspace.map_or_else(
            || crate::skill::parse_skill_plan(&args),
            |workspace| crate::skill::parse_skill_plan_with_workspace(&args, workspace),
        );
        return parsed.map_or_else(
            |message| json_or_human_error(&args, message),
            |plan| RouterAction::RunSkill(Box::new(plan)),
        );
    }

    json_or_human_error(
        &args,
        format!(
            "unknown command {}",
            args.first()
                .and_then(|arg| arg.to_str())
                .unwrap_or("<non-utf8>")
        ),
    )
}

fn root_help_json_requested(args: &[OsString]) -> bool {
    args.len() == 2
        && json_requested(args)
        && args
            .iter()
            .any(|arg| matches!(arg.to_str(), Some("--help" | "-h")))
}

pub fn help_text() -> String {
    crate::command_spec::help_text()
}

pub fn command_help_text(command: &str) -> Option<String> {
    crate::command_spec::command_help_text(command)
}

pub fn json_failure_output(message: &str, code: &str) -> String {
    crate::cli_error::json_failure_output(message, code)
}

pub fn json_requested(args: &[OsString]) -> bool {
    args.iter().any(|arg| {
        arg.to_str()
            .is_some_and(|token| token == "--json" || token == "-j" || token.starts_with("--json="))
    })
}

fn route_parse<T>(
    args: &[OsString],
    result: Result<T, String>,
    success: fn(T) -> RouterAction,
) -> RouterAction {
    result.map_or_else(|message| json_or_human_error(args, message), success)
}

fn normalize_json_error(args: &[OsString], action: RouterAction) -> RouterAction {
    match action {
        RouterAction::Error(message) => json_or_human_error(args, message),
        action => action,
    }
}

fn single_arg_is(args: &[OsString], expected: &str) -> bool {
    args.len() == 1 && first_arg_is(args, expected)
}

fn json_or_human_error(args: &[OsString], message: String) -> RouterAction {
    if json_requested(args) {
        RouterAction::JsonError(JsonErrorPlan {
            message,
            code: "invalid_args".to_owned(),
            exit_code: 64,
        })
    } else {
        RouterAction::Error(message)
    }
}

fn nested_help_requested(args: &[OsString]) -> bool {
    args.iter()
        .skip(1)
        .any(|arg| matches!(arg.to_str(), Some("--help" | "-h")))
}

fn documented_command(args: &[OsString]) -> Option<&'static crate::command_spec::CommandSpec> {
    args.first()
        .and_then(|arg| arg.to_str())
        .and_then(crate::command_spec::command_spec)
}

fn first_arg_is(args: &[OsString], expected: &str) -> bool {
    args.first().is_some_and(|arg| arg == OsStr::new(expected))
}

// Function rationale: test flag parsing stays local to the router boundary
// so native dispatch does not grow a second parser surface.
fn native_test_plan(args: &[OsString]) -> RouterAction {
    let mut fixture_paths = Vec::new();
    let mut receipt_dir = None;
    let mut index = 1;

    while index < args.len() {
        let Some(token) = args.get(index).and_then(|arg| arg.to_str()) else {
            return RouterAction::Error("test arguments must be UTF-8".to_owned());
        };

        if !token.starts_with('-') {
            fixture_paths.push(args[index].clone());
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" | "-j" => {
                if inline_value.is_some() {
                    return RouterAction::Error("--json does not take a value".to_owned());
                }
                index += 1;
            }
            "--receipt-dir" => match inline_value {
                Some(value) => {
                    receipt_dir = Some(OsString::from(value));
                    index += 1;
                }
                None => {
                    let Some(value) = args.get(index + 1) else {
                        return RouterAction::Error(
                            "--receipt-dir requires a directory".to_owned(),
                        );
                    };
                    receipt_dir = Some(value.clone());
                    index += 2;
                }
            },
            _ => return RouterAction::Error(format!("unknown test flag {flag}")),
        }
    }

    if fixture_paths.is_empty() {
        return RouterAction::Error("runx test requires a package or a test file".to_owned());
    }

    RouterAction::RunTest(TestPlan {
        fixture_paths,
        receipt_dir,
    })
}

// Function rationale: doctor parsing keeps mode selection and native path handling in one fail-closed pass.
fn parse_doctor_plan(args: &[OsString]) -> Result<DoctorPlan, String> {
    let mut mode = DoctorMode::Workspace;
    let mut path = None;
    let mut json = false;
    let mut index = 1;

    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            if mode != DoctorMode::Workspace {
                return Err(format!(
                    "runx doctor {} does not accept a path",
                    doctor_mode_name(&mode)
                ));
            }
            if path.is_some() {
                return Err("runx doctor accepts at most one path".to_owned());
            }
            path = Some(PathBuf::from(args[index].clone()));
            index += 1;
            continue;
        };
        if !token.starts_with('-') {
            if matches!(token, "authority" | "registry")
                && path.is_none()
                && mode == DoctorMode::Workspace
            {
                mode = if token == "authority" {
                    DoctorMode::Authority
                } else {
                    DoctorMode::Registry
                };
                index += 1;
                continue;
            }
            if mode != DoctorMode::Workspace {
                return Err(format!(
                    "runx doctor {} does not accept a path",
                    doctor_mode_name(&mode)
                ));
            }
            if path.is_some() {
                return Err("runx doctor accepts at most one path".to_owned());
            }
            path = Some(PathBuf::from(token));
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" | "-j" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            _ => return Err(format!("unknown doctor flag {flag}")),
        }
    }

    Ok(DoctorPlan { mode, path, json })
}

fn doctor_mode_name(mode: &DoctorMode) -> &'static str {
    match mode {
        DoctorMode::Workspace => "workspace",
        DoctorMode::Authority => "authority",
        DoctorMode::Registry => "registry",
    }
}

fn parse_list_plan(args: &[OsString]) -> Result<ListPlan, String> {
    let mut kind = ListKind::All;
    let mut filter = FilterMode::All;
    let mut json = false;
    let mut saw_kind = false;
    let mut index = 1;

    while index < args.len() {
        let token = os_arg(args, index, "list")?;
        if !token.starts_with('-') {
            if saw_kind {
                return Err("runx list accepts at most one kind".to_owned());
            }
            kind = parse_list_kind(token)
                .ok_or_else(|| "runx list kind must be tools, skills, or packets".to_owned())?;
            saw_kind = true;
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        if flag == "--only" {
            let value = match inline_value {
                Some(value) => value,
                None => {
                    index += 1;
                    os_arg(args, index, "list")?
                }
            };
            let requested = match value {
                "valid" => FilterMode::Valid,
                "invalid" => FilterMode::Invalid,
                other => {
                    return Err(format!(
                        "runx list --only takes valid or invalid, not {other}"
                    ));
                }
            };
            if filter != FilterMode::All && filter != requested {
                return Err("runx list takes one --only, valid or invalid".to_owned());
            }
            filter = requested;
            index += 1;
            continue;
        }
        if inline_value.is_some() {
            return Err(format!("{flag} does not take a value"));
        }
        match flag {
            "--json" | "-j" => json = true,
            _ => return Err(format!("unknown list flag {flag}")),
        }
        index += 1;
    }

    Ok(ListPlan { kind, filter, json })
}

fn parse_list_kind(value: &str) -> Option<ListKind> {
    match value {
        "tools" => Some(ListKind::Tools),
        "skills" => Some(ListKind::Skills),
        "packets" => Some(ListKind::Packets),
        _ => None,
    }
}

fn parse_tool_plan(args: &[OsString]) -> Result<ToolPlan, String> {
    let subcommand = os_arg(args, 1, "tool")?;
    let action = match subcommand {
        "search" => ToolAction::Search,
        "inspect" => ToolAction::Inspect,
        _ => return Err(format!("unknown tool subcommand {subcommand}")),
    };
    let mut json = false;
    let mut source = None;
    let mut positionals = Vec::new();
    let mut index = 2;

    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            return Err("runx tool search and inspect arguments must be UTF-8".to_owned());
        };
        if !crate::cli_args::is_flag(token) {
            positionals.push(token.to_owned());
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" | "-j" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            "--source" => {
                let (value, next_index) = flag_value(args, index, flag, inline_value, "tool")?;
                source = Some(value);
                index = next_index;
            }
            _ => return Err(format!("unknown tool flag {flag}")),
        }
    }

    let ref_or_query = positionals.join(" ");
    if ref_or_query.is_empty() {
        return Err(match action {
            ToolAction::Search => "runx tool search requires a query".to_owned(),
            ToolAction::Inspect => "runx tool inspect requires a tool name".to_owned(),
        });
    }
    Ok(ToolPlan {
        action,
        ref_or_query,
        source,
        json,
    })
}

fn parse_registry_plan(args: &[OsString]) -> Result<RegistryPlan, String> {
    let subcommand = os_arg(args, 1, "registry")?;
    let action = parse_registry_action(subcommand)?;
    let mut state = RegistryParseState::default();
    parse_registry_args(args, 2, &mut state)?;
    validate_registry_action_flags(action, subcommand, &state)?;
    let mut subject = if state.records.is_none() {
        registry_subject(action, subcommand, &mut state.positionals)?
    } else if state.positionals.is_empty() {
        String::new()
    } else {
        return Err(
            "runx registry install --records takes no ref: each record names the version it installs"
                .to_owned(),
        );
    };
    let mut version = None;
    if action == RegistryAction::Install && state.records.is_none() {
        // `<publisher>/<name>@<version>` is the one way to name a version.
        let parsed = runx_runtime::registry::parse_registry_ref(&subject);
        subject = parsed.skill_id;
        version = parsed.version;
    }

    Ok(RegistryPlan {
        action,
        subject,
        registry: state.registry,
        version,
        expected_digest: state.expected_digest,
        publisher: state.publisher,
        limit: state.limit,
        records: state.records,
        json: state.json,
    })
}

/// The flags each registry command takes; any other is refused. A publish
/// records the version its package's `run.yaml` states, and the registry
/// grants its tier.
fn validate_registry_action_flags(
    action: RegistryAction,
    subcommand: &str,
    state: &RegistryParseState,
) -> Result<(), String> {
    let (accepted, allowed) = match action {
        RegistryAction::Search => (
            "--registry, --limit and --json",
            state.expected_digest.is_none() && state.publisher.is_none() && state.records.is_none(),
        ),
        RegistryAction::Install if state.records.is_some() => (
            "--records, --registry and --json when it installs records",
            state.publisher.is_none() && state.limit.is_none() && state.expected_digest.is_none(),
        ),
        RegistryAction::Install => (
            "--registry, --digest, --records and --json",
            state.publisher.is_none() && state.limit.is_none(),
        ),
        RegistryAction::Package => (
            "--json",
            state.registry.is_none()
                && state.expected_digest.is_none()
                && state.publisher.is_none()
                && state.limit.is_none()
                && state.records.is_none(),
        ),
        RegistryAction::Publish => (
            "--registry, --publisher and --json",
            state.expected_digest.is_none() && state.limit.is_none() && state.records.is_none(),
        ),
    };
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "runx registry {subcommand} accepts only {accepted}"
        ))
    }
}

#[derive(Default)]
struct RegistryParseState {
    json: bool,
    registry: Option<String>,
    expected_digest: Option<String>,
    publisher: Option<String>,
    limit: Option<usize>,
    records: Option<crate::document_input::DocumentInputSource>,
    positionals: Vec<String>,
}

fn parse_registry_action(subcommand: &str) -> Result<RegistryAction, String> {
    match subcommand {
        "search" => Ok(RegistryAction::Search),
        "install" => Ok(RegistryAction::Install),
        "package" => Ok(RegistryAction::Package),
        "publish" => Ok(RegistryAction::Publish),
        _ => Err(format!("unknown registry subcommand {subcommand}")),
    }
}

fn parse_registry_args(
    args: &[OsString],
    first_argument: usize,
    state: &mut RegistryParseState,
) -> Result<(), String> {
    let mut index = first_argument;
    while index < args.len() {
        let token = os_arg(args, index, "registry")?;
        if !crate::cli_args::is_flag(token) {
            state.positionals.push(token.to_owned());
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        index = parse_registry_flag(args, index, flag, inline_value, state)?;
    }
    Ok(())
}

fn parse_registry_flag(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
    state: &mut RegistryParseState,
) -> Result<usize, String> {
    match flag {
        "--json" | "-j" => set_registry_bool_flag(flag, inline_value, &mut state.json, index),
        "--registry" => {
            set_registry_string_flag(args, index, flag, inline_value, &mut state.registry)
        }
        "--digest" => {
            set_registry_string_flag(args, index, flag, inline_value, &mut state.expected_digest)
        }
        "--publisher" => {
            set_registry_string_flag(args, index, flag, inline_value, &mut state.publisher)
        }
        "--limit" => set_registry_limit_flag(args, index, flag, inline_value, state),
        "--records" => {
            if state.records.is_some() {
                return Err(
                    "runx registry install accepts exactly one --records document".to_owned(),
                );
            }
            let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
            state.records = Some(if value == "-" {
                crate::document_input::DocumentInputSource::Stdin
            } else {
                crate::document_input::DocumentInputSource::Path(PathBuf::from(value))
            });
            Ok(next_index)
        }
        _ => Err(format!("unknown registry flag {flag}")),
    }
}

fn set_registry_bool_flag(
    flag: &str,
    inline_value: Option<&str>,
    target: &mut bool,
    index: usize,
) -> Result<usize, String> {
    if inline_value.is_some() {
        return Err(format!("{flag} does not take a value"));
    }
    *target = true;
    Ok(index + 1)
}

fn set_registry_string_flag(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
    target: &mut Option<String>,
) -> Result<usize, String> {
    let (value, next_index) = flag_value(args, index, flag, inline_value, "registry")?;
    *target = Some(value);
    Ok(next_index)
}

fn set_registry_limit_flag(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
    state: &mut RegistryParseState,
) -> Result<usize, String> {
    let (value, next_index) = flag_value(args, index, flag, inline_value, "registry")?;
    let limit = value
        .parse::<usize>()
        .map_err(|_| "--limit must be a positive integer".to_owned())?;
    if limit == 0 {
        return Err("--limit must be greater than zero".to_owned());
    }
    state.limit = Some(limit);
    Ok(next_index)
}

fn registry_subject(
    action: RegistryAction,
    subcommand: &str,
    positionals: &mut Vec<String>,
) -> Result<String, String> {
    match action {
        RegistryAction::Search => {
            if positionals.is_empty() {
                return Err("runx registry search requires a query".to_owned());
            }
            Ok(positionals.join(" "))
        }
        RegistryAction::Install => {
            if positionals.len() != 1 {
                return Err(
                    "runx registry install requires exactly one <publisher>/<name>[@version]"
                        .to_owned(),
                );
            }
            let subject = positionals.remove(0);
            validate_registry_ref_version(&subject)?;
            Ok(subject)
        }
        RegistryAction::Package | RegistryAction::Publish => {
            if positionals.len() != 1 {
                return Err(format!(
                    "runx registry {subcommand} requires exactly one package: a skill (SKILL.md or its directory) or a connector (connector.yaml or its directory)"
                ));
            }
            Ok(positionals.remove(0))
        }
    }
}

fn validate_registry_ref_version(value: &str) -> Result<(), String> {
    if let Some(version) = parse_registry_ref(value).version {
        validate_registry_version(&version)?;
    }
    Ok(())
}
