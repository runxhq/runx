use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::{
    OperationalPolicy, OperationalPolicyError, OperationalPolicyReadback,
    OperationalPolicyValidationFinding, project_operational_policy_readback,
};
use runx_runtime::WorkspaceEnv;
use serde::Serialize;

/// `runx policy <policy.json>`: validate an operational policy and read it
/// back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyPlan {
    pub path: PathBuf,
    pub json: bool,
}

pub fn run_native_policy(plan: PolicyPlan, workspace: &WorkspaceEnv) -> ExitCode {
    match run_policy_command(&plan, workspace.env(), workspace.cwd()) {
        Ok(output) => crate::cli_io::write_stdout_code(&output.stdout, output.exit_code),
        Err(error) => {
            crate::cli_io::write_stderr_code(&format!("runx: {error}\n"), error.exit_code())
        }
    }
}

pub fn run_policy_command(
    plan: &PolicyPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<PolicyCliOutput, PolicyCliError> {
    let resolved_path = resolve_policy_path(&plan.path, env, cwd);
    let raw = fs::read_to_string(&resolved_path)
        .map_err(|error| PolicyCliError::Read(resolved_path.clone(), error))?;
    let policy = serde_json::from_str::<OperationalPolicy>(&raw)
        .map_err(|error| PolicyCliError::Parse(resolved_path.clone(), error))?;
    let readback = project_operational_policy_readback(&policy)?;
    let findings = readback.findings.clone();
    let result = PolicyCommandResult {
        status: if readback.valid { "success" } else { "failure" },
        path: display_policy_path(&resolved_path, env, cwd),
        policy: readback,
        findings,
    };
    let stdout = if plan.json {
        serde_json::to_string_pretty(&result)
            .map(|json| format!("{json}\n"))
            .map_err(PolicyCliError::Serialize)?
    } else {
        render_policy_result(&result)
    };
    let exit_code = if result.status == "success" { 0 } else { 1 };
    Ok(PolicyCliOutput { stdout, exit_code })
}

#[derive(Debug)]
pub struct PolicyCliOutput {
    pub stdout: String,
    pub exit_code: u8,
}

#[derive(Debug)]
pub enum PolicyCliError {
    Read(PathBuf, io::Error),
    Parse(PathBuf, serde_json::Error),
    Contract(OperationalPolicyError),
    Serialize(serde_json::Error),
}

impl PolicyCliError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Read(_, _) | Self::Parse(_, _) | Self::Contract(_) | Self::Serialize(_) => 1,
        }
    }
}

impl fmt::Display for PolicyCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(path, error) => {
                write!(
                    formatter,
                    "failed to read policy {}: {error}",
                    path.display()
                )
            }
            Self::Parse(path, error) => {
                write!(formatter, "invalid JSON policy {}: {error}", path.display())
            }
            Self::Contract(error) => write!(formatter, "{error}"),
            Self::Serialize(error) => write!(formatter, "failed to serialize policy: {error}"),
        }
    }
}

impl std::error::Error for PolicyCliError {}

impl From<OperationalPolicyError> for PolicyCliError {
    fn from(error: OperationalPolicyError) -> Self {
        Self::Contract(error)
    }
}

#[derive(Serialize)]
struct PolicyCommandResult {
    status: &'static str,
    path: String,
    policy: OperationalPolicyReadback,
    findings: Vec<OperationalPolicyValidationFinding>,
}

impl PolicyCommandResult {
    fn findings(&self) -> &[OperationalPolicyValidationFinding] {
        &self.findings
    }
}

fn render_policy_result(result: &PolicyCommandResult) -> String {
    let mut lines = vec![
        String::new(),
        format!(
            "  {}  policy  {}",
            status_icon(result.status),
            result.status
        ),
    ];
    lines.extend(render_key_value_rows(&[
        ("path", result.path.clone()),
        ("policy", result.policy.policy_id.clone()),
        ("schema", result.policy.schema_version.to_string()),
        ("sources", result.policy.sources.len().to_string()),
        ("targets", result.policy.targets.len().to_string()),
        ("runners", result.policy.runners.len().to_string()),
        ("findings", result.findings().len().to_string()),
    ]));
    push_sources(&mut lines, result);
    push_targets(&mut lines, result);
    push_findings(&mut lines, result.findings());
    lines.push(String::new());
    format!("{}\n", lines.join("\n"))
}

fn push_sources(lines: &mut Vec<String>, result: &PolicyCommandResult) {
    if result.policy.sources.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("  sources".to_owned());
    for source in &result.policy.sources {
        lines.push(format!(
            "  - {}: {}; locators={}; thread={}; actions={}",
            source.source_id,
            source.provider,
            source.locator_count,
            source_thread_label(source.source_thread_required, &source.publish_mode),
            join_actions(&source.allowed_actions)
        ));
    }
}

fn push_targets(lines: &mut Vec<String>, result: &PolicyCommandResult) {
    if result.policy.targets.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("  targets".to_owned());
    for target in &result.policy.targets {
        lines.push(format!(
            "  - {}: runners={}; available={}; owners={}; actions={}",
            target.repo,
            target.runner_ids.join(","),
            target.available_runner_count,
            target.owner_count,
            join_actions(&target.allowed_actions)
        ));
    }
}

fn push_findings(lines: &mut Vec<String>, findings: &[OperationalPolicyValidationFinding]) {
    if findings.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("  findings".to_owned());
    for finding in findings {
        lines.push(format!(
            "  - {} {}: {}",
            finding.code, finding.path, finding.message
        ));
    }
}

fn render_key_value_rows(rows: &[(&str, String)]) -> Vec<String> {
    rows.iter()
        .map(|(key, value)| format!("  {key:<9} {value}"))
        .collect()
}

fn status_icon(status: &str) -> &'static str {
    if status == "success" { "ok" } else { "fail" }
}

fn source_thread_label(
    required: bool,
    mode: &runx_contracts::OperationalPolicyPublishMode,
) -> String {
    if required {
        mode.to_string()
    } else {
        "not-required".to_owned()
    }
}

fn join_actions(actions: &[runx_contracts::OperationalPolicyAction]) -> String {
    actions
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn resolve_policy_path(path: &Path, env: &BTreeMap<String, String>, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    runx_runtime::RunxPaths::resolve(env, cwd)
        .workspace()
        .to_path_buf()
        .join(path)
}

fn display_policy_path(path: &Path, env: &BTreeMap<String, String>, cwd: &Path) -> String {
    let base = runx_runtime::RunxPaths::resolve(env, cwd)
        .workspace()
        .to_path_buf();
    path.strip_prefix(&base)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned())
        })
}
