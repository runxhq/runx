use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use runx_runtime::WorkspaceEnv;

use crate::router::NewPlan;
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
        ("objective".to_owned(), plan.objective),
        ("package_name".to_owned(), name),
        ("repo_root".to_owned(), ".".to_owned()),
        ("target_dir".to_owned(), target_dir),
    ]);
    if let Some(project_context) = plan.project_context {
        inputs.insert("project_context".to_owned(), project_context);
    }

    crate::skill::run_native_skill_with_workspace(
        SkillPlan {
            action: SkillAction::Run,
            skill_path: PathBuf::from("runx/skill-build"),
            runner: Some("build".to_owned()),
            receipt_dir: plan.receipt_dir,
            run_id: None,
            registry: None,
            expected_digest: None,
            expected_package_digest: None,
            expected_execution_closure_digest: None,
            json: plan.json,
            diagnostics: false,
            inputs: crate::skill::CliInputs { texts: inputs },
            input_document: None,
            act: None,
            run_authority: None,
            context: None,
            managed_agent: plan.managed_agent,
        },
        workspace,
    )
}

fn resolve_new_target(
    name: &str,
    directory: Option<&Path>,
    workspace: &Path,
) -> Result<String, String> {
    let requested = directory
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new("skills").join(name));
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
