// Module rationale: router argument handling is centralized for CLI routing tests.
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::cli_args::{flag_value, os_arg, os_flag_value, split_flag};
use crate::config::ConfigPlan;
use crate::connect::ConnectPlan;
use crate::connector::ConnectorPlan;
use crate::credential::CredentialPlan;
use crate::document_input::DocumentInputSource;
use crate::export::ExportPlan;
use crate::kernel::KernelPlan;
use crate::login::LoginPlan;
use crate::managed_agent::{managed_agent_policy, parse_boolean_flag, parse_managed_agent_rounds};
use crate::mcp::McpPlan;
use crate::parser::ParserPlan;
use crate::policy::{PolicyAction, PolicyPlan};
use crate::publish::PublishPlan;
use crate::registry::{ConnectorAction, RegistryAction, RegistryPlan, parse_connector_subject};
use crate::resume::ResumePlan;
use crate::skill::SkillPlan;
use runx_contracts::validate_registry_version;
use runx_runtime::registry::parse_registry_ref;
use runx_runtime::{ManagedAgentPolicy, WorkspaceEnv};

#[derive(Debug, PartialEq)]
pub enum RouterAction {
    Error(String),
    JsonError(JsonErrorPlan),
    RunDev(DevPlan),
    RunDoctor(DoctorPlan),
    RunExport(ExportPlan),
    RunInit(InitPlan),
    RunList(ListPlan),
    RunLogin(LoginPlan),
    RunMcp(McpPlan),
    RunTurn(crate::turn::TurnPlan),
    RunParser(ParserPlan),
    RunNew(NewPlan),
    RunHistory(HistoryPlan),
    RunVerify(VerifyPlan),
    RunHarness(HarnessPlan),
    RunKernel(KernelPlan),
    RunConfig(ConfigPlan),
    RunConnect(ConnectPlan),
    RunConnector(ConnectorPlan),
    RunCredential(CredentialPlan),
    RunPolicy(PolicyPlan),
    RunPublish(PublishPlan),
    RunRegistry(RegistryPlan),
    RunResume(ResumePlan),
    RunSkill(SkillPlan),
    RunTool(ToolPlan),
    RunAddUrl(AddUrlPlan),
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

/// Arguments for indexing a repository via `runx add <repository-url>`.
#[derive(Debug, Eq, PartialEq)]
pub struct AddUrlPlan {
    pub repo: String,
    pub repo_ref: Option<String>,
    pub api_base_url: Option<String>,
    pub json: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DevPlan {
    pub root: Option<PathBuf>,
    pub lane: Option<String>,
    pub json: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct HarnessPlan {
    /// Replay targets: standalone fixture `.yaml` files, or a skill package
    /// (directory / `SKILL.md`) whose declared inline `harness.cases` are run.
    pub fixture_paths: Vec<OsString>,
    /// Where receipts the cases seal are written (`--receipt-dir`).
    pub receipt_dir: Option<OsString>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct HistoryPlan {
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
    OkOnly,
    InvalidOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListKind {
    All,
    Tools,
    Skills,
    Graphs,
    Packets,
    Overlays,
}

#[derive(Debug, Eq, PartialEq)]
pub struct NewPlan {
    pub name: String,
    pub objective: String,
    pub project_context: Option<String>,
    pub directory: Option<PathBuf>,
    pub receipt_dir: Option<PathBuf>,
    pub json: bool,
    pub managed_agent: ManagedAgentPolicy,
}

#[derive(Debug, Eq, PartialEq)]
pub struct InitPlan {
    pub global: bool,
    pub json: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ToolPlan {
    pub action: ToolAction,
    pub path: Option<PathBuf>,
    pub ref_or_query: Option<String>,
    pub all: bool,
    pub source: Option<String>,
    pub json: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ToolAction {
    Build,
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

    if first_arg_is(&args, "harness") {
        return normalize_json_error(&args, native_harness_plan(&args));
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

    if first_arg_is(&args, "credential") {
        return route_parse(
            &args,
            crate::credential::parse_credential_plan(&args),
            RouterAction::RunCredential,
        );
    }

    if first_arg_is(&args, "login") {
        return route_parse(
            &args,
            crate::login::parse_login_plan(&args),
            RouterAction::RunLogin,
        );
    }

    if first_arg_is(&args, "connect") {
        return route_parse(
            &args,
            crate::connect::parse_connect_plan(&args),
            RouterAction::RunConnect,
        );
    }

    if first_arg_is(&args, "policy") {
        return route_parse(&args, parse_policy_plan(&args), RouterAction::RunPolicy);
    }

    if first_arg_is(&args, "publish") {
        return route_parse(
            &args,
            crate::publish::parse_publish_plan(&args),
            RouterAction::RunPublish,
        );
    }

    if first_arg_is(&args, "kernel") {
        return route_parse(&args, parse_kernel_plan(&args), RouterAction::RunKernel);
    }

    if first_arg_is(&args, "parser") {
        return route_parse(&args, parse_parser_plan(&args), RouterAction::RunParser);
    }

    if first_arg_is(&args, "doctor") {
        return route_parse(&args, parse_doctor_plan(&args), RouterAction::RunDoctor);
    }

    if first_arg_is(&args, "dev") {
        return route_parse(&args, parse_dev_plan(&args), RouterAction::RunDev);
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

    if first_arg_is(&args, "new") {
        return route_parse(&args, parse_new_plan(&args), RouterAction::RunNew);
    }

    if first_arg_is(&args, "init") {
        return route_parse(&args, parse_init_plan(&args), RouterAction::RunInit);
    }

    if first_arg_is(&args, "history") {
        return RouterAction::RunHistory(HistoryPlan { args });
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

    if first_arg_is(&args, "turn") {
        return crate::turn::parse_turn_plan(&args)
            .map_or_else(RouterAction::Error, RouterAction::RunTurn);
    }

    if first_arg_is(&args, "mcp") {
        if mcp_runner_before_serve(&args) {
            return RouterAction::Error(
                "runx mcp --runner must follow the serve subcommand".to_owned(),
            );
        }
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

    if first_arg_is(&args, "add") {
        return parse_add_plan(&args).unwrap_or_else(|message| json_or_human_error(&args, message));
    }

    if first_arg_is(&args, "skill") {
        let parsed = workspace.map_or_else(
            || crate::skill::parse_skill_plan(&args),
            |workspace| crate::skill::parse_skill_plan_with_workspace(&args, workspace),
        );
        return parsed.map_or_else(
            |message| json_or_human_error(&args, message),
            RouterAction::RunSkill,
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

pub fn history_help_text() -> String {
    command_help("history")
}

pub fn harness_help_text() -> String {
    command_help("harness")
}

pub fn resume_help_text() -> String {
    command_help("resume")
}

pub fn list_help_text() -> String {
    command_help("list")
}

pub fn login_help_text() -> String {
    command_help("login")
}

pub fn connect_help_text() -> String {
    command_help("connect")
}

pub fn publish_help_text() -> String {
    command_help("publish")
}

pub fn add_help_text() -> String {
    command_help("add")
}

pub fn registry_help_text() -> String {
    command_help("registry")
}

pub fn verify_help_text() -> String {
    command_help("verify")
}

pub fn skill_help_text() -> String {
    command_help("skill")
}

fn command_help(command: &str) -> String {
    command_help_text(command).unwrap_or_default()
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

fn mcp_runner_before_serve(args: &[OsString]) -> bool {
    args.iter()
        .skip(1)
        .take_while(|arg| arg.as_os_str() != OsStr::new("serve"))
        .any(|arg| {
            arg.to_str()
                .is_some_and(|token| token == "--runner" || token.starts_with("--runner="))
        })
}

// Function rationale: harness flag parsing stays local to the
// router boundary so native dispatch does not grow a second parser surface.
fn native_harness_plan(args: &[OsString]) -> RouterAction {
    let mut fixture_paths = Vec::new();
    let mut receipt_dir = None;
    let mut index = 1;

    while index < args.len() {
        let Some(token) = args.get(index).and_then(|arg| arg.to_str()) else {
            return RouterAction::Error("harness arguments must be UTF-8".to_owned());
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
            "-R" => {
                if inline_value.is_some() {
                    return RouterAction::Error(
                        "-R requires a separate directory value".to_owned(),
                    );
                }
                let Some(value) = args.get(index + 1) else {
                    return RouterAction::Error("-R requires a directory".to_owned());
                };
                receipt_dir = Some(value.clone());
                index += 2;
            }
            _ => return RouterAction::Error(format!("unknown harness flag {flag}")),
        }
    }

    if fixture_paths.is_empty() {
        return RouterAction::Error(
            "runx harness requires a fixture path or skill package".to_owned(),
        );
    }

    RouterAction::RunHarness(HarnessPlan {
        fixture_paths,
        receipt_dir,
    })
}

fn parse_new_plan(args: &[OsString]) -> Result<NewPlan, String> {
    let parsed = parse_new_args(args)?;
    Ok(NewPlan {
        name: parsed
            .name
            .ok_or_else(|| "runx new requires a package name".to_owned())?,
        objective: parsed
            .objective
            .ok_or_else(|| "runx new requires --objective".to_owned())?,
        project_context: parsed.project_context,
        directory: parsed.directory,
        receipt_dir: parsed.receipt_dir,
        json: parsed.json,
        managed_agent: managed_agent_policy(
            "new",
            parsed.managed_agent,
            parsed.managed_agent_rounds,
        )?,
    })
}

#[derive(Default)]
struct NewParseState {
    name: Option<String>,
    objective: Option<String>,
    project_context: Option<String>,
    directory: Option<PathBuf>,
    receipt_dir: Option<PathBuf>,
    json: bool,
    managed_agent: bool,
    managed_agent_rounds: Option<u32>,
}

fn parse_new_args(args: &[OsString]) -> Result<NewParseState, String> {
    let mut parsed = NewParseState::default();
    let mut index = 1;
    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            return Err("runx new package name and option values must be UTF-8".to_owned());
        };
        if !token.starts_with('-') {
            if parsed.name.is_none() {
                parsed.name = Some(token.to_owned());
            } else {
                return Err(format!("unexpected runx new argument {token}"));
            }
            index += 1;
            continue;
        }
        index = parse_new_flag(args, index, token, &mut parsed)?;
    }
    Ok(parsed)
}

fn parse_new_flag(
    args: &[OsString],
    index: usize,
    token: &str,
    parsed: &mut NewParseState,
) -> Result<usize, String> {
    let (flag, inline_value) = split_flag(token);
    match flag {
        "--json" | "-j" => {
            if inline_value.is_some() {
                return Err(format!("{flag} does not take a value"));
            }
            parsed.json = true;
            Ok(index + 1)
        }
        "--objective" => {
            let (value, next_index) = flag_value(args, index, flag, inline_value, "runx new")?;
            parsed.objective = Some(non_empty_new_value(flag, value)?);
            Ok(next_index)
        }
        "--project-context" => {
            let (value, next_index) = flag_value(args, index, flag, inline_value, "runx new")?;
            parsed.project_context = Some(non_empty_new_value(flag, value)?);
            Ok(next_index)
        }
        "--directory" => {
            let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
            parsed.directory = Some(PathBuf::from(value));
            Ok(next_index)
        }
        "--receipt-dir" | "-R" => {
            let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
            parsed.receipt_dir = Some(PathBuf::from(value));
            Ok(next_index)
        }
        "--managed-agent" => {
            parsed.managed_agent = match inline_value {
                Some(value) => parse_boolean_flag("new", flag, value)?,
                None => true,
            };
            Ok(index + 1)
        }
        "--managed-agent-rounds" => {
            let (value, next_index) = flag_value(args, index, flag, inline_value, "runx new")?;
            parsed.managed_agent_rounds = Some(parse_managed_agent_rounds("new", &value)?);
            Ok(next_index)
        }
        _ => Err(format!("unknown new flag {flag}")),
    }
}

fn non_empty_new_value(flag: &str, value: String) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("runx new {flag} requires a non-empty value"));
    }
    Ok(value.to_owned())
}

fn parse_add_plan(args: &[OsString]) -> Result<RouterAction, String> {
    let parsed = parse_add_args(args)?;
    if is_repository_url(parsed.subject.as_deref().unwrap_or_default()) {
        return add_url_plan(parsed).map(RouterAction::RunAddUrl);
    }
    add_registry_plan(parsed).map(RouterAction::RunRegistry)
}

#[derive(Default)]
struct AddParseState {
    subject: Option<String>,
    registry: Option<String>,
    version: Option<String>,
    repo_ref: Option<String>,
    expected_digest: Option<String>,
    destination: Option<PathBuf>,
    api_base_url: Option<String>,
    json: bool,
}

fn parse_add_args(args: &[OsString]) -> Result<AddParseState, String> {
    let mut parsed = AddParseState::default();
    let mut index = 1;
    while index < args.len() {
        let token = os_arg(args, index, "add")?;
        if !token.starts_with("--") {
            if parsed.subject.is_some() {
                return Err("runx add accepts exactly one ref or repository URL".to_owned());
            }
            parsed.subject = Some(token.to_owned());
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        index = parse_add_flag(&mut parsed, args, index, flag, inline_value)?;
    }
    if parsed.subject.is_none() {
        return Err("runx add requires a skill ref or repository URL".to_owned());
    }
    Ok(parsed)
}

fn parse_add_flag(
    parsed: &mut AddParseState,
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
) -> Result<usize, String> {
    match flag {
        "--json" => {
            if inline_value.is_some() {
                return Err("--json does not take a value".to_owned());
            }
            parsed.json = true;
            Ok(index + 1)
        }
        "--registry" => set_add_string(args, index, flag, inline_value, &mut parsed.registry),
        "--version" => set_add_string(args, index, flag, inline_value, &mut parsed.version),
        "--ref" => set_add_string(args, index, flag, inline_value, &mut parsed.repo_ref),
        "--digest" => set_add_string(args, index, flag, inline_value, &mut parsed.expected_digest),
        "--to" => {
            let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
            parsed.destination = Some(PathBuf::from(value));
            Ok(next_index)
        }
        "--api-base-url" => {
            set_add_string(args, index, flag, inline_value, &mut parsed.api_base_url)
        }
        _ => Err(format!("unknown add flag {flag}")),
    }
}

fn set_add_string(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
    slot: &mut Option<String>,
) -> Result<usize, String> {
    let (value, next_index) = flag_value(args, index, flag, inline_value, "add")?;
    *slot = Some(value);
    Ok(next_index)
}

fn add_url_plan(parsed: AddParseState) -> Result<AddUrlPlan, String> {
    if parsed.registry.is_some() {
        return Err(
            "runx add <repository-url> uses --api-base-url for the hosted index API, not --registry"
                .to_owned(),
        );
    }
    if parsed.version.is_some() {
        return Err("runx add <repository-url> uses --ref for git refs, not --version".to_owned());
    }
    if parsed.expected_digest.is_some() || parsed.destination.is_some() {
        return Err(
            "runx add <repository-url> indexes the repository and does not support --to or --digest"
                .to_owned(),
        );
    }
    Ok(AddUrlPlan {
        repo: parsed.subject.unwrap_or_default(),
        repo_ref: parsed.repo_ref,
        api_base_url: parsed.api_base_url,
        json: parsed.json,
    })
}

fn add_registry_plan(parsed: AddParseState) -> Result<RegistryPlan, String> {
    if parsed.repo_ref.is_some() {
        return Err(
            "runx add <skill-ref> uses --version for registry versions, not --ref".to_owned(),
        );
    }
    if parsed.api_base_url.is_some() {
        return Err("runx add <skill-ref> does not accept --api-base-url".to_owned());
    }
    Ok(RegistryPlan {
        action: RegistryAction::Install,
        subject: parsed.subject.unwrap_or_default(),
        registry: parsed.registry,
        registry_dir: None,
        version: parsed.version,
        expected_digest: parsed.expected_digest,
        destination: parsed.destination,
        owner: None,
        profile: None,
        trust_tier: None,
        limit: None,
        upsert: false,
        json: parsed.json,
    })
}

/// A repository URL rather than a skill ref: an `http(s)` URL, or a host name
/// (it has a dot) followed by at least an owner and a name. The hosted index
/// decides which hosts it supports.
fn is_repository_url(value: &str) -> bool {
    let value = value.trim();
    let path = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    parts.next().is_some_and(|host| {
        host.contains('.')
            && !host.starts_with('.')
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
    }) && parts.next().is_some()
        && parts.next().is_some()
}

fn parse_init_plan(args: &[OsString]) -> Result<InitPlan, String> {
    let mut global = false;
    let mut json = false;
    let mut index = 1;

    while index < args.len() {
        let token = os_arg(args, index, "init")?;
        if token == "-g" {
            global = true;
            index += 1;
            continue;
        }
        if !token.starts_with("--") {
            return Err(format!("unexpected init argument {token}"));
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            "--global" => {
                if inline_value.is_some() {
                    return Err("--global does not take a value".to_owned());
                }
                global = true;
                index += 1;
            }
            _ => return Err(format!("unknown init flag {flag}")),
        }
    }

    Ok(InitPlan { global, json })
}

fn parse_dev_plan(args: &[OsString]) -> Result<DevPlan, String> {
    let mut root = None;
    let mut lane = None;
    let mut json = false;
    let mut index = 1;

    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            if root.is_some() {
                return Err("runx dev accepts at most one root path".to_owned());
            }
            root = Some(PathBuf::from(args[index].clone()));
            index += 1;
            continue;
        };
        if !token.starts_with("--") {
            if root.is_some() {
                return Err("runx dev accepts at most one root path".to_owned());
            }
            root = Some(PathBuf::from(token));
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            "--lane" => {
                let (value, next_index) = flag_value(args, index, flag, inline_value, "dev")?;
                if value.is_empty() {
                    return Err("--lane must not be empty".to_owned());
                }
                lane = Some(value);
                index = next_index;
            }
            _ => return Err(format!("unknown dev flag {flag}")),
        }
    }

    Ok(DevPlan { root, lane, json })
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
            kind = parse_list_kind(token).ok_or_else(|| {
                "runx list kind must be tools, skills, graphs, packets, or overlays".to_owned()
            })?;
            saw_kind = true;
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        if inline_value.is_some() {
            return Err(format!("{flag} does not take a value"));
        }
        let requested = match flag {
            "--json" | "-j" => {
                json = true;
                index += 1;
                continue;
            }
            "--ok-only" => FilterMode::OkOnly,
            "--invalid-only" => FilterMode::InvalidOnly,
            _ => return Err(format!("unknown list flag {flag}")),
        };
        if filter != FilterMode::All && filter != requested {
            return Err("runx list accepts either --ok-only or --invalid-only".to_owned());
        }
        filter = requested;
        index += 1;
    }

    Ok(ListPlan { kind, filter, json })
}

fn parse_list_kind(value: &str) -> Option<ListKind> {
    match value {
        "tools" => Some(ListKind::Tools),
        "skills" => Some(ListKind::Skills),
        "graphs" => Some(ListKind::Graphs),
        "packets" => Some(ListKind::Packets),
        "overlays" => Some(ListKind::Overlays),
        _ => None,
    }
}

fn parse_kernel_plan(args: &[OsString]) -> Result<KernelPlan, String> {
    let subcommand = os_arg(args, 1, "kernel")?;
    if subcommand != "eval" {
        return Err(format!("unknown kernel subcommand {subcommand}"));
    }
    let parsed = parse_json_eval_input(
        args,
        2,
        JsonEvalCommand {
            command: "kernel",
            subject: "kernel eval",
            duplicate_input: "runx kernel eval accepts exactly one --input",
            requires_json: "runx kernel eval requires --json",
            requires_input: "runx kernel eval requires --input <file|->",
        },
    )?;
    Ok(KernelPlan {
        input: parsed.input,
        json: true,
    })
}

fn parse_parser_plan(args: &[OsString]) -> Result<ParserPlan, String> {
    let subcommand = os_arg(args, 1, "parser")?;
    if subcommand != "eval" {
        return Err(format!("unknown parser subcommand {subcommand}"));
    }
    let parsed = parse_json_eval_input(
        args,
        2,
        JsonEvalCommand {
            command: "parser",
            subject: "parser eval",
            duplicate_input: "runx parser eval accepts exactly one --input",
            requires_json: "runx parser eval requires --json",
            requires_input: "runx parser eval requires --input <file|->",
        },
    )?;
    Ok(ParserPlan {
        input: parsed.input,
        json: true,
    })
}

struct JsonEvalCommand {
    command: &'static str,
    subject: &'static str,
    duplicate_input: &'static str,
    requires_json: &'static str,
    requires_input: &'static str,
}

struct JsonEvalPlan {
    input: DocumentInputSource,
}

fn parse_json_eval_input(
    args: &[OsString],
    mut index: usize,
    command: JsonEvalCommand,
) -> Result<JsonEvalPlan, String> {
    let mut input = None;
    let mut json = false;
    while index < args.len() {
        let token = os_arg(args, index, command.command)?;
        if !token.starts_with("--") {
            return Err(format!("unexpected {} argument {token}", command.subject));
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            "--input" => {
                if input.is_some() {
                    return Err(command.duplicate_input.to_owned());
                }
                let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
                input = Some(if value == OsStr::new("-") {
                    DocumentInputSource::Stdin
                } else {
                    DocumentInputSource::Path(PathBuf::from(value))
                });
                index = next_index;
            }
            _ => return Err(format!("unknown {} flag {flag}", command.subject)),
        }
    }
    if !json {
        return Err(command.requires_json.to_owned());
    }
    Ok(JsonEvalPlan {
        input: input.ok_or_else(|| command.requires_input.to_owned())?,
    })
}

fn parse_policy_plan(args: &[OsString]) -> Result<PolicyPlan, String> {
    let subcommand = os_arg(args, 1, "policy")?;
    let action = match subcommand {
        "inspect" => PolicyAction::Inspect,
        "lint" => PolicyAction::Lint,
        _ => return Err(format!("unknown policy subcommand {subcommand}")),
    };
    let mut json = false;
    let mut positionals = Vec::new();
    let mut index = 2;

    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            positionals.push(PathBuf::from(args[index].clone()));
            index += 1;
            continue;
        };
        if !token.starts_with("--") {
            positionals.push(PathBuf::from(token));
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            _ => return Err(format!("unknown policy flag {flag}")),
        }
    }

    let [path] = positionals.as_slice() else {
        return Err("runx policy inspect|lint requires exactly one policy path".to_owned());
    };
    Ok(PolicyPlan {
        action,
        path: path.clone(),
        json,
    })
}

fn parse_tool_plan(args: &[OsString]) -> Result<ToolPlan, String> {
    let subcommand = os_arg(args, 1, "tool")?;
    let action = match subcommand {
        "build" => ToolAction::Build,
        "search" => ToolAction::Search,
        "inspect" => ToolAction::Inspect,
        _ => return Err(format!("unknown tool subcommand {subcommand}")),
    };
    let mut json = false;
    let mut all = false;
    let mut source = None;
    let mut positionals = Vec::<OsString>::new();
    let mut index = 2;

    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            if action != ToolAction::Build {
                return Err("runx tool search and inspect arguments must be UTF-8".to_owned());
            }
            positionals.push(args[index].clone());
            index += 1;
            continue;
        };
        if !token.starts_with("--") {
            positionals.push(OsString::from(token));
            index += 1;
            continue;
        }

        let (flag, inline_value) = split_flag(token);
        match flag {
            "--json" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            "--all" => {
                if inline_value.is_some() {
                    return Err("--all does not take a value".to_owned());
                }
                all = true;
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

    match action {
        ToolAction::Build => build_tool_plan(positionals, all, source, json),
        ToolAction::Search => search_tool_plan(positionals, all, source, json),
        ToolAction::Inspect => inspect_tool_plan(positionals, all, source, json),
    }
}

fn build_tool_plan(
    positionals: Vec<OsString>,
    all: bool,
    source: Option<String>,
    json: bool,
) -> Result<ToolPlan, String> {
    if source.is_some() {
        return Err("runx tool build does not accept --source".to_owned());
    }
    if all && !positionals.is_empty() {
        return Err("runx tool build accepts either --all or one tool directory".to_owned());
    }
    if !all && positionals.len() != 1 {
        return Err("runx tool build requires a tool directory or --all".to_owned());
    }

    Ok(ToolPlan {
        action: ToolAction::Build,
        path: positionals.first().cloned().map(PathBuf::from),
        ref_or_query: None,
        all,
        source: None,
        json,
    })
}

fn search_tool_plan(
    positionals: Vec<OsString>,
    all: bool,
    source: Option<String>,
    json: bool,
) -> Result<ToolPlan, String> {
    if all {
        return Err("runx tool search does not accept --all".to_owned());
    }
    let query = utf8_tool_positionals(positionals)?.join(" ");
    if query.is_empty() {
        return Err("runx tool search requires a query".to_owned());
    }

    Ok(ToolPlan {
        action: ToolAction::Search,
        path: None,
        ref_or_query: Some(query),
        all: false,
        source,
        json,
    })
}

fn inspect_tool_plan(
    positionals: Vec<OsString>,
    all: bool,
    source: Option<String>,
    json: bool,
) -> Result<ToolPlan, String> {
    if all {
        return Err("runx tool inspect does not accept --all".to_owned());
    }
    let tool_ref = utf8_tool_positionals(positionals)?.join(" ");
    if tool_ref.is_empty() {
        return Err("runx tool inspect requires a tool reference".to_owned());
    }

    Ok(ToolPlan {
        action: ToolAction::Inspect,
        path: None,
        ref_or_query: Some(tool_ref),
        all: false,
        source,
        json,
    })
}

fn utf8_tool_positionals(positionals: Vec<OsString>) -> Result<Vec<String>, String> {
    positionals
        .into_iter()
        .map(|value| {
            value
                .into_string()
                .map_err(|_| "runx tool search and inspect arguments must be UTF-8".to_owned())
        })
        .collect()
}

fn parse_registry_plan(args: &[OsString]) -> Result<RegistryPlan, String> {
    let subcommand = os_arg(args, 1, "registry")?;
    let (action, subcommand, first_argument) = if subcommand == "connector" {
        let connector = os_arg(args, 2, "registry connector")?;
        (
            RegistryAction::Connector(ConnectorAction::parse(connector)?),
            format!("connector {connector}"),
            3,
        )
    } else if subcommand == "mark" {
        let mark = os_arg(args, 2, "registry mark")?;
        if mark != "check" {
            return Err(format!("unknown registry mark subcommand {mark}"));
        }
        (RegistryAction::MarkCheck, "mark check".to_owned(), 3)
    } else {
        (parse_registry_action(subcommand)?, subcommand.to_owned(), 2)
    };
    let mut state = RegistryParseState::default();
    parse_registry_args(args, first_argument, &mut state)?;
    validate_registry_action_flags(&action, &subcommand, &state)?;
    let subject = registry_subject(&action, &subcommand, &mut state.positionals)?;

    Ok(RegistryPlan {
        action,
        subject,
        registry: state.registry,
        registry_dir: state.registry_dir,
        version: state.version,
        expected_digest: state.expected_digest,
        destination: state.destination,
        owner: state.owner,
        profile: state.profile,
        trust_tier: state.trust_tier,
        limit: state.limit,
        upsert: state.upsert,
        json: state.json,
    })
}

fn validate_registry_action_flags(
    action: &RegistryAction,
    subcommand: &str,
    state: &RegistryParseState,
) -> Result<(), String> {
    if let RegistryAction::Connector(connector) = action {
        // Connectors have no local registry, owner override, profile or trust flag.
        let (accepted, allowed) = match connector {
            ConnectorAction::Package => ("--json", !registry_target_flags(state)),
            ConnectorAction::Publish | ConnectorAction::Read => (
                "--registry, --version and --json",
                state.expected_digest.is_none(),
            ),
            ConnectorAction::Install => ("--registry, --version, --digest and --json", true),
        };
        let shared = state.profile.is_none()
            && state.registry_dir.is_none()
            && state.destination.is_none()
            && state.owner.is_none()
            && state.trust_tier.is_none()
            && state.limit.is_none()
            && !state.upsert;
        return if allowed && shared {
            Ok(())
        } else {
            Err(format!(
                "runx registry {subcommand} accepts only {accepted}"
            ))
        };
    }
    if *action == RegistryAction::MarkCheck {
        let only_json = !registry_target_flags(state) && state.profile.is_none();
        return if only_json {
            Ok(())
        } else {
            Err("runx registry mark check accepts only --json".to_owned())
        };
    }
    if *action != RegistryAction::Package {
        return Ok(());
    }
    if registry_target_flags(state) {
        return Err("runx registry package accepts only --profile and --json".to_owned());
    }
    Ok(())
}

/// A flag naming a registry, a version or an install, which packaging never takes.
fn registry_target_flags(state: &RegistryParseState) -> bool {
    state.registry.is_some()
        || state.registry_dir.is_some()
        || state.version.is_some()
        || state.expected_digest.is_some()
        || state.destination.is_some()
        || state.owner.is_some()
        || state.trust_tier.is_some()
        || state.limit.is_some()
        || state.upsert
}

#[derive(Default)]
struct RegistryParseState {
    json: bool,
    upsert: bool,
    registry: Option<String>,
    registry_dir: Option<PathBuf>,
    version: Option<String>,
    expected_digest: Option<String>,
    destination: Option<PathBuf>,
    owner: Option<String>,
    profile: Option<PathBuf>,
    trust_tier: Option<runx_runtime::registry::TrustTier>,
    limit: Option<usize>,
    positionals: Vec<String>,
}

fn parse_registry_action(subcommand: &str) -> Result<RegistryAction, String> {
    match subcommand {
        "search" => Ok(RegistryAction::Search),
        "read" => Ok(RegistryAction::Read),
        "resolve" => Ok(RegistryAction::Resolve),
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
        if !token.starts_with("--") {
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
        "--json" => set_registry_bool_flag(flag, inline_value, &mut state.json, index),
        "--upsert" => set_registry_bool_flag(flag, inline_value, &mut state.upsert, index),
        "--registry" => {
            set_registry_string_flag(args, index, flag, inline_value, &mut state.registry)
        }
        "--registry-dir" => {
            set_registry_path_flag(args, index, flag, inline_value, &mut state.registry_dir)
        }
        "--version" => set_registry_version_flag(args, index, flag, inline_value, state),
        "--digest" => {
            set_registry_string_flag(args, index, flag, inline_value, &mut state.expected_digest)
        }
        "--to" => set_registry_path_flag(args, index, flag, inline_value, &mut state.destination),
        "--owner" => set_registry_string_flag(args, index, flag, inline_value, &mut state.owner),
        "--profile" => set_registry_path_flag(args, index, flag, inline_value, &mut state.profile),
        "--trust-tier" => set_registry_trust_tier_flag(args, index, flag, inline_value, state),
        "--limit" => set_registry_limit_flag(args, index, flag, inline_value, state),
        _ => Err(format!("unknown registry flag {flag}")),
    }
}

fn set_registry_trust_tier_flag(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
    state: &mut RegistryParseState,
) -> Result<usize, String> {
    if state.trust_tier.is_some() {
        return Err(format!("{flag} was provided more than once"));
    }
    let (value, next_index) = flag_value(args, index, flag, inline_value, "registry")?;
    state.trust_tier = Some(parse_registry_trust_tier(&value)?);
    Ok(next_index)
}

fn parse_registry_trust_tier(value: &str) -> Result<runx_runtime::registry::TrustTier, String> {
    match value {
        "first_party" | "first-party" => Ok(runx_runtime::registry::TrustTier::FirstParty),
        "verified" => Ok(runx_runtime::registry::TrustTier::Verified),
        "community" => Ok(runx_runtime::registry::TrustTier::Community),
        _ => Err(format!(
            "invalid registry trust tier {value}; expected first_party, verified, or community"
        )),
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

fn set_registry_path_flag(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
    target: &mut Option<PathBuf>,
) -> Result<usize, String> {
    let (value, next_index) = os_flag_value(args, index, flag, inline_value)?;
    *target = Some(PathBuf::from(value));
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

fn set_registry_version_flag(
    args: &[OsString],
    index: usize,
    flag: &str,
    inline_value: Option<&str>,
    state: &mut RegistryParseState,
) -> Result<usize, String> {
    let (value, next_index) = flag_value(args, index, flag, inline_value, "registry")?;
    validate_registry_version(&value)?;
    state.version = Some(value);
    Ok(next_index)
}

fn registry_subject(
    action: &RegistryAction,
    subcommand: &str,
    positionals: &mut Vec<String>,
) -> Result<String, String> {
    match action {
        RegistryAction::Search => {
            if positionals.is_empty() {
                return Err("runx registry search requires a query".to_owned());
            }
            let subject = positionals.join(" ");
            validate_registry_ref_version(&subject)?;
            Ok(subject)
        }
        RegistryAction::Read | RegistryAction::Resolve | RegistryAction::Install => {
            if positionals.len() != 1 {
                return Err(format!(
                    "runx registry {subcommand} requires exactly one ref"
                ));
            }
            let subject = positionals.remove(0);
            validate_registry_ref_version(&subject)?;
            Ok(subject)
        }
        RegistryAction::Package | RegistryAction::Publish => {
            if positionals.len() != 1 {
                return Err(format!(
                    "runx registry {subcommand} requires exactly one skill markdown path"
                ));
            }
            Ok(positionals.remove(0))
        }
        RegistryAction::Connector(ConnectorAction::Package | ConnectorAction::Publish) => {
            if positionals.len() != 1 {
                return Err(format!(
                    "runx registry {subcommand} requires exactly one connector directory"
                ));
            }
            Ok(positionals.remove(0))
        }
        RegistryAction::Connector(ConnectorAction::Read | ConnectorAction::Install) => {
            if positionals.len() != 1 {
                return Err(format!(
                    "runx registry {subcommand} requires exactly one <publisher>/<id>[@version]"
                ));
            }
            let subject = positionals.remove(0);
            parse_connector_subject(&subject)?;
            Ok(subject)
        }
        RegistryAction::MarkCheck => {
            if positionals.len() != 1 {
                return Err("runx registry mark check requires exactly one mark file".to_owned());
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
