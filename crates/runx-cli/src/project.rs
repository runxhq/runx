use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::JsonValue;
use runx_runtime::WorkspaceEnv;
use runx_runtime::{InitAction, InitGeneratedValues, RunxInitOptions, RunxInitResult, runx_init};
use serde::Serialize;

use crate::router::{InitPlan, NewPlan};
use crate::skill::{SkillAction, SkillPlan};

pub fn run_native_new_with_workspace(plan: NewPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let name = match normalize_skill_name(&plan.name) {
        Ok(name) => name,
        Err(error) => return write_new_failure(&error, plan.json, "invalid_args"),
    };
    let target_dir = match resolve_new_target(&name, plan.directory.as_deref(), workspace.cwd()) {
        Ok(target_dir) => target_dir,
        Err(error) => return write_new_failure(&error, plan.json, "invalid_target"),
    };

    let mut inputs = BTreeMap::from([
        ("objective".to_owned(), JsonValue::String(plan.objective)),
        ("package_name".to_owned(), JsonValue::String(name)),
        ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
        ("target_dir".to_owned(), JsonValue::String(target_dir)),
    ]);
    if let Some(project_context) = plan.project_context {
        inputs.insert(
            "project_context".to_owned(),
            JsonValue::String(project_context),
        );
    }

    crate::skill::run_native_skill_with_workspace(
        SkillPlan {
            action: SkillAction::Run,
            skill_path: PathBuf::from("runx/skill-build"),
            runner: Some("build".to_owned()),
            receipt_dir: plan.receipt_dir,
            run_id: None,
            answers: None,
            registry: None,
            expected_digest: None,
            expected_package_digest: None,
            expected_execution_closure_digest: None,
            json: plan.json,
            diagnostics: false,
            trusted_command_execution: true,
            full_operator_context: false,
            inputs,
            input_document: None,
            act: None,
            managed_agent: plan.managed_agent,
        },
        workspace,
    )
}

pub fn run_native_init(plan: InitPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let paths = workspace.paths();
    let options = RunxInitOptions {
        action: if plan.global {
            InitAction::Global
        } else {
            InitAction::Project
        },
        paths: paths.clone(),
        official_cache_dir: crate::registry::official_skills_cache_root(paths, workspace.env()),
        prefetch_official: plan.prefetch_official,
        generated: InitGeneratedValues::generate(),
    };

    match runx_init(&options) {
        Ok(result) => render_init_result(plan.json, &result),
        Err(error) => {
            let _ignored = write_stderr_line(&format!("runx: {error}"));
            ExitCode::from(1)
        }
    }
}

fn write_json<T: serde::Serialize>(command: &str, result: &T) -> ExitCode {
    match serde_json::to_string_pretty(result) {
        Ok(output) => write_stdout_line(&output),
        Err(error) => {
            let _ignored = write_stderr_line(&format!(
                "runx: failed to serialize {command} result: {error}"
            ));
            ExitCode::from(1)
        }
    }
}

fn render_init_result(json: bool, result: &RunxInitResult) -> ExitCode {
    if json {
        return write_json(
            "init",
            &InitJsonResult {
                status: "success",
                init: result,
            },
        );
    }
    let title = match &result.action {
        InitAction::Global => "runx global init",
        InitAction::Project => "runx project init",
    };
    write_stdout(&render_key_values(
        title,
        &[
            (
                "created",
                Some(if result.created { "yes" } else { "no" }.to_owned()),
            ),
            (
                "project",
                result
                    .project_dir
                    .as_ref()
                    .map(|path| path.display().to_string()),
            ),
            ("project_id", result.project_id.clone()),
            (
                "home",
                result
                    .global_home_dir
                    .as_ref()
                    .map(|path| path.display().to_string()),
            ),
            ("installation_id", result.installation_id.clone()),
            (
                "official_cache",
                result
                    .official_cache_dir
                    .as_ref()
                    .map(|path| path.display().to_string()),
            ),
        ],
    ))
}

fn render_key_values(title: &str, rows: &[(&str, Option<String>)]) -> String {
    let mut output = format!("\n  {title}  success\n\n");
    for (key, value) in rows {
        output.push_str(&format!("  {key}  {}\n", value.as_deref().unwrap_or("-")));
    }
    output.push('\n');
    output
}

fn resolve_new_target(
    name: &str,
    directory: Option<&Path>,
    workspace: &Path,
) -> Result<String, String> {
    let requested = directory
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(name));
    let relative = if requested.is_absolute() {
        let workspace = workspace.canonicalize().map_err(|error| {
            format!(
                "failed to resolve runx workspace {}: {error}",
                workspace.display()
            )
        })?;
        requested
            .strip_prefix(&workspace)
            .map_err(|_| "runx new --directory must stay inside the active workspace".to_owned())?
    } else {
        requested.as_path()
    };
    relative_posix_path(relative)
}

fn relative_posix_path(path: &Path) -> Result<String, String> {
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => segments.push(
                segment
                    .to_str()
                    .ok_or_else(|| "runx new --directory must be UTF-8".to_owned())?,
            ),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(
                    "runx new --directory must be a child path inside the active workspace"
                        .to_owned(),
                );
            }
        }
    }
    if segments.is_empty() {
        return Err("runx new --directory must name a child path".to_owned());
    }
    Ok(segments.join("/"))
}

fn normalize_skill_name(value: &str) -> Result<String, String> {
    let mut normalized = String::with_capacity(value.len());
    let mut replacing = false;
    for character in value.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '_' | '.' | '-')
        {
            normalized.push(character);
            replacing = false;
        } else if !replacing {
            normalized.push('-');
            replacing = true;
        }
    }
    let normalized = normalized
        .trim_matches(|character| matches!(character, '.' | '_' | '-'))
        .to_owned();
    if normalized.is_empty() {
        return Err("runx new package name must contain a letter or number".to_owned());
    }
    Ok(normalized)
}

#[derive(Serialize)]
struct InitJsonResult<'a> {
    status: &'static str,
    init: &'a RunxInitResult,
}

fn write_stdout(message: &str) -> ExitCode {
    crate::cli_io::write_stdout_code(message, 0)
}

fn write_stdout_line(message: &str) -> ExitCode {
    crate::cli_io::write_stdout_code(&format!("{message}\n"), 0)
}

fn write_stderr_line(message: &str) -> ExitCode {
    crate::cli_io::write_stderr_code(&format!("{message}\n"))
}

fn write_new_failure(message: &str, json: bool, code: &str) -> ExitCode {
    if json {
        return crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(message, code),
            1,
        );
    }
    let _ignored = write_stderr_line(&format!("runx: {message}"));
    ExitCode::from(1)
}
