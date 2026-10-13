use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_runtime::WorkspaceEnv;
use runx_runtime::export::{RunxExportLoadError, RunxExportLoadOptions};
use serde::Serialize;

mod managed;
mod parser;
mod report;
mod shim;

pub use parser::parse_export_plan;

const CLAUDE_MARKER: &str = "runx-export:claude";
const CODEX_MARKER: &str = "runx-export:codex";
const CODEX_RULE_START: &str = "# >>> runx-export start (managed) >>>";
const CODEX_RULE_END: &str = "# <<< runx-export end <<<";
const CODEX_RULE_RUNX_ON_PATH: &str = "prefix_rule(pattern = [\"runx\", \"skill\"], decision = \"allow\", justification = \"runx skill invocations are trusted\")";
const CODEX_RULE_RUNX_RESUME_ON_PATH: &str = "prefix_rule(pattern = [\"runx\", \"resume\"], decision = \"allow\", justification = \"runx resume invocations are trusted\")";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportPlan {
    pub target: Target,
    pub refs: Vec<String>,
    pub project: bool,
    pub json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Claude,
    Codex,
}

impl Target {
    fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Claude => CLAUDE_MARKER,
            Self::Codex => CODEX_MARKER,
        }
    }
}

#[derive(Clone, Debug)]
struct GeneratedFile {
    path: PathBuf,
    contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExportReport {
    pub target: String,
    pub scope: String,
    pub exported: Vec<ExportedFile>,
    pub pruned: Vec<String>,
    pub rules_file: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExportedFile {
    pub skill: String,
    pub path: String,
}

#[derive(Debug)]
pub enum ExportError {
    InvalidArgs(String),
    Io {
        context: String,
        source: std::io::Error,
    },
    Parse(String),
    Unsupported(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArgs(message) | Self::Parse(message) | Self::Unsupported(message) => {
                formatter.write_str(message)
            }
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<RunxExportLoadError> for ExportError {
    fn from(error: RunxExportLoadError) -> Self {
        match error {
            RunxExportLoadError::InvalidArgs(message) => Self::InvalidArgs(message),
            RunxExportLoadError::Io { context, source } => Self::Io { context, source },
            RunxExportLoadError::Parse(message) => Self::Parse(message),
        }
    }
}

pub fn run_native_export(plan: ExportPlan, workspace: &WorkspaceEnv) -> ExitCode {
    match run_export_command(&plan, workspace.cwd(), workspace.env()) {
        Ok(report) => report::write_report(&report, plan.json),
        Err(ExportError::InvalidArgs(message)) => {
            let _ignored = writeln!(std::io::stderr(), "runx: {message}");
            ExitCode::from(64)
        }
        Err(error) => {
            let _ignored = writeln!(std::io::stderr(), "runx: {error}");
            ExitCode::from(1)
        }
    }
}

pub fn run_export_command(
    plan: &ExportPlan,
    cwd: &Path,
    env: &BTreeMap<String, String>,
) -> Result<ExportReport, ExportError> {
    validate_export_plan(plan)?;
    let root = canonicalize(cwd, "canonicalizing export root")?;
    validate_full_export_root(plan, &root)?;
    let runx_bin = exported_runx_binary(env)?;
    let skills = runx_runtime::export::load_export_skills_with_options(RunxExportLoadOptions {
        root: &root,
        refs: &plan.refs,
        source_roots: runx_runtime::RunxPaths::resolve(env, cwd).skill_roots(env),
        execution_env: Some(env),
    })?;
    let skill_dir = target_skill_dir(plan.target, plan.project, cwd, env);
    let files = shim::plan_files(
        plan.target,
        plan.project,
        &root,
        &skills,
        &skill_dir,
        &runx_bin,
    )?;
    managed::validate_write_targets(plan.target, &files, &root, plan.refs.is_empty())?;
    let warnings = managed::managed_drift_warnings(plan.target, &files)?;
    managed::write_files(&files)?;
    let pruned = if plan.refs.is_empty() {
        managed::prune_managed_files(plan.target, &skill_dir, &files, &root)?
    } else {
        Vec::new()
    };
    let rules_file = if plan.target == Target::Codex && !plan.project {
        Some(managed::merge_codex_rules(
            &codex_rules_file(env, cwd),
            &runx_bin,
        )?)
    } else {
        None
    };

    Ok(export_report(plan, &files, pruned, rules_file, warnings))
}

fn exported_runx_binary(env: &BTreeMap<String, String>) -> Result<PathBuf, ExportError> {
    if let Some(value) = env
        .get("RUNX_EXPORT_BIN")
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(PathBuf::from(value));
    }
    std::env::current_exe().map_err(|source| ExportError::Io {
        context: "resolving current runx binary for export shim".to_owned(),
        source,
    })
}

fn validate_export_plan(plan: &ExportPlan) -> Result<(), ExportError> {
    if plan.target == Target::Codex && plan.project {
        return Err(ExportError::Unsupported(
            "runx export codex --project is not supported until Codex project skill and rules paths are verified".to_owned(),
        ));
    }
    Ok(())
}

fn validate_full_export_root(plan: &ExportPlan, root: &Path) -> Result<(), ExportError> {
    if plan.refs.is_empty() && !root.join("SKILL.md").is_file() && !root.join("skills").is_dir() {
        return Err(ExportError::InvalidArgs(format!(
            "{} is not a skill workspace; run a full export from a directory containing SKILL.md or skills/, or name the skill refs to export",
            display_path(root)
        )));
    }
    Ok(())
}

fn export_report(
    plan: &ExportPlan,
    files: &[GeneratedFile],
    pruned: Vec<String>,
    rules_file: Option<PathBuf>,
    warnings: Vec<String>,
) -> ExportReport {
    ExportReport {
        target: plan.target.as_str().to_owned(),
        scope: scope_name(plan.project).to_owned(),
        exported: files
            .iter()
            .map(|file| ExportedFile {
                skill: file
                    .path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown")
                    .to_owned(),
                path: display_path(&file.path),
            })
            .collect(),
        pruned,
        rules_file: rules_file.map(|path| display_path(&path)),
        warnings,
    }
}

fn target_skill_dir(
    target: Target,
    project: bool,
    cwd: &Path,
    env: &BTreeMap<String, String>,
) -> PathBuf {
    if project {
        return match target {
            Target::Claude => cwd.join(".claude").join("skills"),
            Target::Codex => cwd.join(".codex").join("skills"),
        };
    }
    let home = home_dir(env, cwd);
    match target {
        Target::Claude => home.join(".claude").join("skills"),
        Target::Codex => home.join(".codex").join("skills"),
    }
}

fn codex_rules_file(env: &BTreeMap<String, String>, cwd: &Path) -> PathBuf {
    home_dir(env, cwd)
        .join(".codex")
        .join("rules")
        .join("default.rules")
}

fn canonicalize(path: &Path, context: &str) -> Result<PathBuf, ExportError> {
    fs::canonicalize(path).map_err(|source| ExportError::Io {
        context: format!("{context} {}", display_path(path)),
        source,
    })
}

fn home_dir(env: &BTreeMap<String, String>, cwd: &Path) -> PathBuf {
    env.get("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn scope_name(project: bool) -> &'static str {
    if project { "project" } else { "global" }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
