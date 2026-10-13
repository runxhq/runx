// Module rationale: native CLI command dispatch remains one audited
// boundary so release shims and exit-code handling are visible in one place.
use std::env;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_cli::router::{HarnessPlan, RouterAction, command_help_text, help_text};

const HARNESS_STALE_RECEIPT_STORE_HINT: &str = "runx: hint: the receipt store contains entries that do not verify with the current issuer; retry with --receipt-dir .runx/harness-receipts for an isolated project-local harness run.";

fn main() -> ExitCode {
    if let Err(error) = install_interrupt_handler() {
        let _ignored = write_stderr_line(&format!(
            "runx: failed to install terminal interrupt handler: {error}"
        ));
        return ExitCode::from(70);
    }
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let workspace = match load_workspace(&args) {
        Ok(workspace) => workspace,
        Err(exit_code) => return exit_code,
    };
    let action = match workspace.as_ref() {
        Some(workspace) => runx_cli::router::route_args_with_workspace(args, workspace),
        None => runx_cli::router::route_args(args),
    };
    let exit_code = dispatch(action, workspace.as_ref());
    if runx_runtime::interrupt::was_interrupted() {
        ExitCode::from(130)
    } else {
        exit_code
    }
}

fn install_interrupt_handler() -> Result<(), ctrlc::Error> {
    ctrlc::set_handler(|| {
        if !runx_runtime::interrupt::terminate_active_processes() {
            std::process::exit(130);
        }
        if std::thread::Builder::new()
            .name("runx-interrupt-watchdog".to_owned())
            .spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(2));
                std::process::exit(130);
            })
            .is_err()
        {
            std::process::exit(130);
        }
    })
}

fn dispatch(action: RouterAction, workspace: Option<&runx_runtime::WorkspaceEnv>) -> ExitCode {
    match action {
        RouterAction::Error(message) => {
            let _ignored = write_stderr_line(&format!("runx: {message}"));
            ExitCode::from(64)
        }
        RouterAction::JsonError(plan) => {
            write_json_failure(&plan.message, &plan.code, plan.exit_code)
        }
        RouterAction::PrintHelp => write_stdout(&help_text()),
        RouterAction::PrintHelpJson => write_command_catalog(),
        RouterAction::PrintCommandHelp(command) => {
            write_stdout(&command_help_text(command).unwrap_or_else(help_text))
        }
        RouterAction::PrintCommandUsageError(command) => {
            let help = command_help_text(command).unwrap_or_else(help_text);
            let _ignored = write_stderr_line(&help);
            ExitCode::from(64)
        }
        RouterAction::PrintVersion => {
            write_stdout_line(&format!("runx-cli {}", env!("CARGO_PKG_VERSION")))
        }
        command => {
            let Some(workspace) = workspace else {
                return write_json_failure(
                    "command execution requires a resolved Runx workspace",
                    "workspace_required",
                    1,
                );
            };
            dispatch_command(command, workspace)
        }
    }
}

fn dispatch_command(action: RouterAction, workspace: &runx_runtime::WorkspaceEnv) -> ExitCode {
    match action {
        RouterAction::RunInit(plan) => runx_cli::run_native_init(plan, workspace),
        RouterAction::RunNew(plan) => runx_cli::run_native_new_with_workspace(plan, workspace),
        RouterAction::RunHistory(plan) => run_native_history(plan.args, workspace),
        RouterAction::RunVerify(plan) => run_native_verify(plan.args, workspace),
        RouterAction::RunList(plan) => run_native_list(plan, workspace),
        RouterAction::RunLogin(plan) => runx_cli::login::run_native_login(plan, workspace),
        RouterAction::RunMcp(plan) => runx_cli::mcp::run_native_mcp_with_workspace(plan, workspace),
        RouterAction::RunTurn(plan) => {
            runx_cli::turn::run_native_turn_with_workspace(plan, workspace)
        }
        RouterAction::RunHarness(plan) => run_native_harness(plan, workspace),
        RouterAction::RunConfig(plan) => run_native_config(plan, workspace),
        RouterAction::RunConnect(plan) => {
            runx_cli::connect::run_native_connect_with_stdin(plan, workspace, io::stdin())
        }
        RouterAction::RunConnector(plan) => {
            runx_cli::connector::run_native_connector(plan, workspace)
        }
        RouterAction::RunCredential(plan) => {
            runx_cli::credential::run_native_credential_with_workspace(plan, workspace)
        }
        RouterAction::RunPolicy(plan) => runx_cli::policy::run_native_policy(plan, workspace),
        RouterAction::RunNotarize(plan) => runx_cli::notarize::run_native_notarize(plan, workspace),
        RouterAction::RunRegistry(plan) => runx_cli::registry::run_native_registry(plan, workspace),
        RouterAction::RunResume(plan) => {
            runx_cli::resume::run_native_resume_with_workspace(plan, workspace)
        }
        RouterAction::RunSkill(plan) => {
            runx_cli::skill::run_native_skill_with_workspace(plan, workspace)
        }
        RouterAction::RunDoctor(plan) => runx_cli::doctor::run_native_doctor(plan, workspace),
        RouterAction::RunDev(plan) => runx_cli::dev::run_native_dev(plan, workspace),
        RouterAction::RunExport(plan) => runx_cli::export::run_native_export(plan, workspace),
        RouterAction::RunTool(plan) => runx_cli::tool::run_native_tool(plan, workspace),
        RouterAction::Error(_)
        | RouterAction::JsonError(_)
        | RouterAction::PrintHelp
        | RouterAction::PrintHelpJson
        | RouterAction::PrintCommandHelp(_)
        | RouterAction::PrintCommandUsageError(_)
        | RouterAction::PrintVersion => {
            let _ignored =
                write_stderr_line("runx: internal command routing error after workspace admission");
            ExitCode::from(70)
        }
    }
}

fn load_workspace(args: &[OsString]) -> Result<Option<runx_runtime::WorkspaceEnv>, ExitCode> {
    if invocation_is_informational(args) {
        return Ok(None);
    }
    let cwd = env::current_dir()
        .map_err(|error| workspace_error(args, &format!("failed to resolve cwd: {error}")))?;
    runx_runtime::WorkspaceEnv::load_process(cwd)
        .map(Some)
        .map_err(|error| workspace_error(args, &error.to_string()))
}

fn workspace_error(args: &[OsString], message: &str) -> ExitCode {
    if runx_cli::router::json_requested(args) {
        return write_json_failure(message, "workspace_env_error", 1);
    }
    let _ignored = write_stderr_line(&format!("runx: {message}"));
    ExitCode::from(1)
}

fn invocation_is_informational(args: &[OsString]) -> bool {
    args.is_empty()
        || args
            .iter()
            .any(|arg| matches!(arg.to_str(), Some("--help" | "-h")))
        || matches!(
            args.first().and_then(|arg| arg.to_str()),
            Some("help" | "--version" | "-V")
        )
}

fn run_native_history(args: Vec<OsString>, workspace: &runx_runtime::WorkspaceEnv) -> ExitCode {
    match runx_cli::history::run_history_command(&args, workspace.env(), workspace.cwd()) {
        Ok(output) => write_stdout(&output.output),
        Err(runx_cli::history::HistoryCliError::InvalidArgs(message)) => {
            let _ignored = write_stderr_line(&format!("runx: {message}"));
            ExitCode::from(64)
        }
        Err(error) => {
            let _ignored = write_stderr_line(&format!("runx: {error}"));
            ExitCode::from(1)
        }
    }
}

fn run_native_verify(args: Vec<OsString>, workspace: &runx_runtime::WorkspaceEnv) -> ExitCode {
    let json = runx_cli::router::json_requested(&args);
    match runx_cli::verify::run_verify_command_with_stdin(
        &args,
        workspace.env(),
        workspace.cwd(),
        io::stdin(),
    ) {
        Ok(result) => {
            let exit = write_stdout(&result.output);
            if result.failed {
                ExitCode::from(3)
            } else {
                exit
            }
        }
        Err(runx_cli::verify::VerifyCliError::InvalidArgs(message)) => {
            if json {
                return write_json_failure(&message, "invalid_args", 64);
            }
            let _ignored = write_stderr_line(&format!("runx: {message}"));
            ExitCode::from(64)
        }
        Err(error) => {
            if json {
                return write_json_failure(&error.to_string(), "runtime_error", 1);
            }
            let _ignored = write_stderr_line(&format!("runx: {error}"));
            ExitCode::from(1)
        }
    }
}

fn run_native_list(
    plan: runx_cli::router::ListPlan,
    workspace: &runx_runtime::WorkspaceEnv,
) -> ExitCode {
    match runx_cli::list::run_list_command(&plan, workspace.env(), workspace.cwd()) {
        Ok(output) => write_stdout(&output),
        Err(error) => {
            let _ignored = write_stderr_line(&format!("runx: {error}"));
            ExitCode::from(1)
        }
    }
}

fn run_native_config(
    mut plan: runx_cli::config::ConfigPlan,
    workspace: &runx_runtime::WorkspaceEnv,
) -> ExitCode {
    if plan.value_from_stdin {
        let mut value = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut value) {
            let _ignored =
                write_stderr_line(&format!("runx: failed to read config value: {error}"));
            return ExitCode::from(1);
        }
        let value = value.trim_end_matches(['\r', '\n']);
        if value.is_empty() {
            let _ignored =
                write_stderr_line("runx: secret config value from stdin must not be empty");
            return ExitCode::from(64);
        }
        plan.value = Some(value.to_owned());
    }
    match runx_cli::config::run_config_command(&plan, workspace.env(), workspace.cwd()) {
        Ok(output) => write_stdout(&output),
        Err(runx_cli::config::ConfigCliError::InvalidArgs(message)) => {
            let _ignored = write_stderr_line(&format!("runx: {message}"));
            ExitCode::from(64)
        }
        Err(error) => {
            let _ignored = write_stderr_line(&format!("runx: {error}"));
            ExitCode::from(1)
        }
    }
}

fn run_native_harness(plan: HarnessPlan, workspace: &runx_runtime::WorkspaceEnv) -> ExitCode {
    if contains_skill_package(&plan.fixture_paths) {
        let [target] = plan.fixture_paths.as_slice() else {
            let _ignored = write_stderr_line(
                "runx harness accepts one skill package, or one or more fixture files, not a mix",
            );
            return ExitCode::from(64);
        };
        return run_package_harness(Path::new(target), plan.receipt_dir.as_ref(), workspace);
    }
    run_standalone_harness(plan.fixture_paths, plan.receipt_dir, workspace)
}

fn run_standalone_harness(
    fixture_paths: Vec<OsString>,
    receipt_dir: Option<OsString>,
    workspace: &runx_runtime::WorkspaceEnv,
) -> ExitCode {
    let mut outputs = Vec::new();
    let orchestrator = match runx_cli::runtime::local_orchestrator(workspace.env()) {
        Ok(orchestrator) => orchestrator,
        Err(error) => {
            let _ignored = write_stderr_line(&format!(
                "runx: failed to initialize runtime effects: {error}"
            ));
            return ExitCode::from(1);
        }
    };
    for fixture_path in fixture_paths {
        let request = runx_runtime::HarnessRunRequest {
            fixture_path: PathBuf::from(fixture_path),
            receipt_dir: receipt_dir.as_ref().map(PathBuf::from),
        };
        match orchestrator.run_harness_fixture(&request) {
            Ok(output) => {
                outputs.push(
                    match serde_json::to_value(&output.receipt)
                        .and_then(serde_json::from_value::<runx_contracts::JsonValue>)
                    {
                        Ok(value) => value,
                        Err(error) => {
                            let _ignored = write_stderr_line(&format!(
                                "runx: failed to serialize receipt: {error}"
                            ));
                            return ExitCode::from(1);
                        }
                    },
                );
            }
            Err(error) => {
                let error_message = error.to_string();
                let _ignored = write_stderr_line(&format!(
                    "runx: native harness replay failed for {}: {error_message}",
                    request.fixture_path.display()
                ));
                return ExitCode::from(1);
            }
        }
    }
    write_harness_receipts(outputs)
}

fn write_harness_receipts(mut outputs: Vec<runx_contracts::JsonValue>) -> ExitCode {
    let output = if outputs.len() == 1 {
        outputs.pop().unwrap_or(runx_contracts::JsonValue::Null)
    } else {
        runx_contracts::JsonValue::Array(outputs)
    };
    match serde_json::to_string_pretty(&output) {
        Ok(json) => write_stdout_line(&json),
        Err(error) => {
            let _ignored =
                write_stderr_line(&format!("runx: failed to serialize receipt: {error}"));
            ExitCode::from(1)
        }
    }
}

// A skill package (directory or SKILL.md) runs its inline `harness.cases` and
// conventional `fixtures/*.yaml`; standalone fixture paths replay as receipts.
fn contains_skill_package(paths: &[OsString]) -> bool {
    paths.iter().any(|path| is_skill_package(Path::new(path)))
}

// A harness target is a skill package when it
// is a SKILL.md file, or a directory that actually holds a skill package
// (a SKILL.md or run.yaml). A plain directory of fixture `.yaml` files is NOT a
// skill package and falls through to standalone fixture replay.
fn is_skill_package(path: &Path) -> bool {
    if path.is_dir() {
        return path.join("SKILL.md").exists() || path.join(runx_parser::PROFILE_FILE).exists();
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
}

fn run_package_harness(
    skill_path: &Path,
    receipt_dir: Option<&OsString>,
    workspace: &runx_runtime::WorkspaceEnv,
) -> ExitCode {
    let request = runx_runtime::PackageHarnessRequest {
        skill_path: skill_path.to_path_buf(),
        receipt_dir: receipt_dir.map(PathBuf::from),
    };
    let orchestrator = match runx_cli::runtime::local_orchestrator(workspace.env()) {
        Ok(orchestrator) => orchestrator,
        Err(error) => {
            let _ignored = write_stderr_line(&format!(
                "runx: failed to initialize runtime effects: {error}"
            ));
            return ExitCode::from(1);
        }
    };
    let report = match orchestrator.run_package_harness(&request) {
        Ok(report) => report,
        Err(error) => {
            let error_message = error.to_string();
            let _ignored = write_stderr_line(&format!(
                "runx: package harness failed for {}: {error_message}",
                skill_path.display()
            ));
            return ExitCode::from(1);
        }
    };
    if report.status == "failed"
        && let Some(hint) = package_harness_report_hint(&report)
    {
        let _ignored = write_stderr_line(hint);
    }
    let json = match serde_json::to_string_pretty(&report) {
        Ok(json) => json,
        Err(error) => {
            let _ignored = write_stderr_line(&format!(
                "runx: failed to serialize harness summary: {error}"
            ));
            return ExitCode::from(1);
        }
    };
    // The summary (including a `failed` one) is the artifact a caller parses, so
    // always emit it; a `failed` suite still exits non-zero for shell use.
    let _ignored = write_stdout_line(&json);
    if report.status == "failed" {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn package_harness_report_hint(
    report: &runx_runtime::PackageHarnessReport,
) -> Option<&'static str> {
    if report
        .assertion_errors
        .iter()
        .any(|error| error.contains("receipt store index is stale"))
    {
        return Some(HARNESS_STALE_RECEIPT_STORE_HINT);
    }
    None
}

fn write_stdout(message: &str) -> ExitCode {
    let mut stdout = io::stdout().lock();
    if stdout.write_all(message.as_bytes()).is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn write_command_catalog() -> ExitCode {
    match runx_cli::command_spec::catalog_json() {
        Ok(catalog) => write_stdout(&catalog),
        Err(error) => {
            let _ignored = write_stderr_line(&format!(
                "runx: failed to serialize the command catalog: {error}"
            ));
            ExitCode::from(1)
        }
    }
}

fn write_json_failure(message: &str, code: &str, exit_code: u8) -> ExitCode {
    let output = runx_cli::router::json_failure_output(message, code);
    let mut stdout = io::stdout().lock();
    if stdout.write_all(output.as_bytes()).is_ok() {
        ExitCode::from(exit_code)
    } else {
        ExitCode::from(1)
    }
}

fn write_stdout_line(message: &str) -> ExitCode {
    let mut stdout = io::stdout().lock();
    if writeln!(stdout, "{message}").is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn write_stderr_line(message: &str) -> ExitCode {
    let mut stderr = io::stderr().lock();
    if writeln!(stderr, "{message}").is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_harness_report_hint_recognizes_stale_receipt_store() {
        let report = failed_package_harness_report(
            "smoke: receipt store index is stale: receipt proof is invalid for sha256:abc",
        );

        assert_eq!(
            package_harness_report_hint(&report),
            Some(HARNESS_STALE_RECEIPT_STORE_HINT)
        );
    }

    fn failed_package_harness_report(error: &str) -> runx_runtime::PackageHarnessReport {
        runx_runtime::PackageHarnessReport {
            status: "failed",
            case_count: 1,
            assertion_error_count: 1,
            assertion_errors: vec![error.to_owned()],
            case_names: vec!["smoke".to_owned()],
            receipt_ids: Vec::new(),
            graph_case_count: 0,
        }
    }
}
