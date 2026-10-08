use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_runtime::{ManagedAgentPolicy, WorkspaceEnv};

const MAX_STDIN_ANSWERS_BYTES: u64 = 4 * 1024 * 1024;

use crate::managed_agent::{managed_agent_policy, parse_boolean_flag, parse_managed_agent_rounds};
use crate::skill::{SkillAction, SkillPlan};

#[derive(Debug, PartialEq, Eq)]
pub struct ResumePlan {
    pub run_id: String,
    pub answers_path: PathBuf,
    pub receipt_dir: Option<PathBuf>,
    pub expected_package_digest: Option<String>,
    pub expected_execution_closure_digest: Option<String>,
    pub json: bool,
    pub diagnostics: bool,
    pub managed_agent: ManagedAgentPolicy,
}

pub(crate) struct SkillResumeCommand<'a> {
    pub(crate) run_id: &'a str,
    pub(crate) receipt_dir: Option<&'a Path>,
    pub(crate) answers_path: Option<&'a Path>,
}

// Function rationale: resume parsing keeps UTF-8 identifiers and native path arguments in one audited cursor.
pub fn parse_resume_plan(args: &[OsString]) -> Result<ResumePlan, String> {
    if args.first().and_then(|arg| arg.to_str()) != Some("resume") {
        return Err("internal error: resume dispatcher received non-resume command".to_owned());
    }
    let mut receipt_dir = None;
    let mut expected_package_digest = None;
    let mut expected_execution_closure_digest = None;
    let mut json = false;
    let mut diagnostics = false;
    let mut managed_agent = false;
    let mut managed_agent_rounds = None;
    let mut positionals = Vec::<OsString>::new();
    let mut index = 1;
    while index < args.len() {
        let Some(token) = args[index].to_str() else {
            positionals.push(args[index].clone());
            index += 1;
            continue;
        };
        match token {
            "--json" | "-j" => {
                json = true;
                index += 1;
            }
            "--diagnostics" => {
                diagnostics = true;
                index += 1;
            }
            "--managed-agent" => {
                managed_agent = true;
                index += 1;
            }
            value if value.starts_with("--managed-agent=") => {
                managed_agent = parse_boolean_flag(
                    "resume",
                    "--managed-agent",
                    value.trim_start_matches("--managed-agent="),
                )?;
                index += 1;
            }
            value if value.starts_with("--managed-agent-rounds=") => {
                managed_agent_rounds = Some(parse_managed_agent_rounds(
                    "resume",
                    value.trim_start_matches("--managed-agent-rounds="),
                )?);
                index += 1;
            }
            "--managed-agent-rounds" => {
                index += 1;
                managed_agent_rounds = Some(parse_managed_agent_rounds(
                    "resume",
                    args.get(index)
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| "--managed-agent-rounds requires a value".to_owned())?,
                )?);
                index += 1;
            }
            value if value.starts_with("--receipt-dir=") => {
                receipt_dir = Some(PathBuf::from(value.trim_start_matches("--receipt-dir=")));
                index += 1;
            }
            "--receipt-dir" | "-R" => {
                index += 1;
                receipt_dir = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| format!("{token} requires a directory"))?
                        .clone(),
                ));
                index += 1;
            }
            value if value.starts_with("--package-digest=") => {
                expected_package_digest = Some(binding_flag_value(
                    "--package-digest",
                    value.trim_start_matches("--package-digest="),
                )?);
                index += 1;
            }
            "--package-digest" => {
                index += 1;
                expected_package_digest = Some(binding_flag_value(
                    "--package-digest",
                    args.get(index)
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| "--package-digest requires a value".to_owned())?,
                )?);
                index += 1;
            }
            value if value.starts_with("--execution-closure-digest=") => {
                expected_execution_closure_digest = Some(binding_flag_value(
                    "--execution-closure-digest",
                    value.trim_start_matches("--execution-closure-digest="),
                )?);
                index += 1;
            }
            "--execution-closure-digest" => {
                index += 1;
                expected_execution_closure_digest = Some(binding_flag_value(
                    "--execution-closure-digest",
                    args.get(index)
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| "--execution-closure-digest requires a value".to_owned())?,
                )?);
                index += 1;
            }
            "-" => {
                positionals.push(OsString::from("-"));
                index += 1;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown runx resume option {value}"));
            }
            value => {
                positionals.push(OsString::from(value));
                index += 1;
            }
        }
    }
    if positionals.len() != 2 {
        return Err("runx resume requires <run-id> <answers.json|->".to_owned());
    }
    Ok(ResumePlan {
        run_id: positionals
            .remove(0)
            .into_string()
            .map_err(|_| "runx resume run id must be UTF-8".to_owned())?,
        answers_path: PathBuf::from(positionals.remove(0)),
        receipt_dir,
        expected_package_digest,
        expected_execution_closure_digest,
        json,
        diagnostics,
        managed_agent: managed_agent_policy("resume", managed_agent, managed_agent_rounds)?,
    })
}

// Function rationale: resume reconstructs one guarded continuation
// request and keeps its path, receipt, and output error handling in one transaction.
pub fn run_native_resume_with_workspace(plan: ResumePlan, workspace: &WorkspaceEnv) -> ExitCode {
    let paused = match runx_runtime::run_record::paused_run(
        workspace.env(),
        workspace.cwd(),
        &plan.run_id,
    ) {
        Ok(Some(paused)) => paused,
        Ok(None) => {
            return write_resume_failure(
                &format!("no paused run found for {}", plan.run_id),
                plan.json,
                1,
            );
        }
        Err(error) => {
            return write_resume_failure(
                &format!("could not read run {}: {error}", plan.run_id),
                plan.json,
                1,
            );
        }
    };
    let expected_package_digest = match resume_binding(
        "package",
        plan.expected_package_digest.as_deref(),
        &paused.package_digest,
    ) {
        Ok(digest) => digest,
        Err(error) => return write_resume_failure(&error, plan.json, 1),
    };
    let expected_execution_closure_digest = match resume_binding(
        "execution closure",
        plan.expected_execution_closure_digest.as_deref(),
        &paused.execution_closure_digest,
    ) {
        Ok(digest) => digest,
        Err(error) => return write_resume_failure(&error, plan.json, 1),
    };
    let answers_path = if plan.answers_path == Path::new("-") {
        match materialize_stdin_answers(workspace, &plan.run_id) {
            Ok(path) => path,
            Err(error) => return write_resume_failure(&error, plan.json, 1),
        }
    } else {
        plan.answers_path
    };
    let skill_plan = SkillPlan {
        action: SkillAction::Run,
        skill_path: PathBuf::from(&paused.skill_ref),
        runner: Some(paused.runner.clone()),
        receipt_dir: plan.receipt_dir,
        run_id: Some(plan.run_id),
        answers: Some(answers_path),
        registry: None,
        expected_digest: None,
        expected_package_digest: Some(expected_package_digest),
        expected_execution_closure_digest: Some(expected_execution_closure_digest),
        json: plan.json,
        diagnostics: plan.diagnostics,
        trusted_command_execution: false,
        full_operator_context: false,
        inputs: BTreeMap::new(),
        input_document: None,
        managed_agent: plan.managed_agent,
    };
    crate::skill::run_native_skill_with_workspace(skill_plan, workspace)
}

fn materialize_stdin_answers(workspace: &WorkspaceEnv, run_id: &str) -> Result<PathBuf, String> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_STDIN_ANSWERS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read resume answers from stdin: {error}"))?;
    if bytes.len() as u64 > MAX_STDIN_ANSWERS_BYTES {
        return Err(format!(
            "resume answers from stdin exceed the {} byte limit",
            MAX_STDIN_ANSWERS_BYTES
        ));
    }
    let value = serde_json::from_slice::<runx_contracts::JsonValue>(&bytes)
        .map_err(|error| format!("resume answers from stdin are not valid JSON: {error}"))?;
    if value.as_object().is_none() {
        return Err("resume answers from stdin must be a JSON object".to_owned());
    }
    let canonical = serde_json::to_vec(&value)
        .map_err(|error| format!("failed to encode resume answers: {error}"))?;
    let workspace_base =
        runx_runtime::resolve_runx_workspace_base(workspace.env(), workspace.cwd());
    let project_runx_dir = runx_runtime::resolve_project_runx_dir(workspace.env(), &workspace_base);
    let directory = project_runx_dir.join("continuations");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create {}: {error}", directory.display()))?;
    let digest = runx_contracts::sha256_prefixed(&canonical);
    let digest = digest.strip_prefix("sha256:").unwrap_or(&digest);
    let path = directory.join(format!(
        "{}-{digest}.answers.json",
        safe_path_segment(run_id)
    ));
    if !path.exists() {
        let temporary = directory.join(format!(
            ".{}-{digest}.answers.tmp-{}",
            safe_path_segment(run_id),
            std::process::id()
        ));
        fs::write(&temporary, &canonical)
            .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
        if let Err(error) = fs::rename(&temporary, &path) {
            if !path.exists() {
                let _ = fs::remove_file(&temporary);
                return Err(format!("failed to commit {}: {error}", path.display()));
            }
            let _ = fs::remove_file(&temporary);
        }
    }
    Ok(path)
}

fn safe_path_segment(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.is_empty() {
        "run".to_owned()
    } else {
        value
    }
}

fn binding_flag_value(flag: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{flag} requires a non-empty value"));
    }
    Ok(value.to_owned())
}

/// A digest the caller pins must be the one the run was started under.
fn resume_binding(label: &str, requested: Option<&str>, recorded: &str) -> Result<String, String> {
    if let Some(requested) = requested
        && requested != recorded
    {
        return Err(format!(
            "resume {label} binding mismatch: requested {requested}, the run was started under {recorded}"
        ));
    }
    Ok(recorded.to_owned())
}

pub(crate) fn render_skill_resume_command(command: SkillResumeCommand<'_>) -> String {
    let mut parts = vec![
        "runx".to_owned(),
        "resume".to_owned(),
        shell_token(command.run_id),
    ];
    parts.push(shell_token(
        &command
            .answers_path
            .map_or_else(|| "-".into(), Path::to_string_lossy),
    ));
    if let Some(receipt_dir) = command.receipt_dir {
        parts.push("--receipt-dir".to_owned());
        parts.push(shell_token(&receipt_dir.to_string_lossy()));
    }
    parts.join(" ")
}

pub(crate) fn shell_token(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }
    if value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-' | ':' | '@')
    }) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_resume_failure(message: &str, json: bool, exit_code: u8) -> ExitCode {
    if json {
        return crate::cli_io::write_stdout_code(
            &crate::router::json_failure_output(message, "resume_error"),
            exit_code,
        );
    }
    let _ignored = writeln!(io::stderr(), "runx: {message}");
    ExitCode::from(exit_code)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::{SkillResumeCommand, render_skill_resume_command};

    #[test]
    fn resume_managed_agent_requires_fresh_explicit_consent() -> Result<(), String> {
        let plan = super::parse_resume_plan(
            &[
                "resume",
                "run_123",
                "answers.json",
                "--managed-agent",
                "--managed-agent-rounds=2",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>(),
        )?;
        assert_eq!(
            plan.managed_agent,
            runx_runtime::ManagedAgentPolicy::Inline { max_rounds: 2 }
        );
        Ok(())
    }

    #[test]
    fn resume_accepts_execution_bindings_without_treating_them_as_answers() -> Result<(), String> {
        let plan = super::parse_resume_plan(
            &[
                "resume",
                "run_123",
                "answers.json",
                "--package-digest=sha256:package",
                "--execution-closure-digest",
                "sha256:closure",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>(),
        )?;
        assert_eq!(
            plan.expected_package_digest.as_deref(),
            Some("sha256:package")
        );
        assert_eq!(
            plan.expected_execution_closure_digest.as_deref(),
            Some("sha256:closure")
        );
        Ok(())
    }

    #[test]
    fn resume_rejects_a_binding_that_disagrees_with_its_run() -> Result<(), String> {
        let error = match super::resume_binding(
            "execution closure",
            Some("sha256:requested"),
            "sha256:recorded",
        ) {
            Err(error) => error,
            Ok(_) => return Err("resume replaced its durable execution binding".to_owned()),
        };
        assert!(error.contains("resume execution closure binding mismatch"));
        Ok(())
    }

    #[test]
    fn resume_command_quotes_operator_supplied_tokens() {
        let command = render_skill_resume_command(SkillResumeCommand {
            run_id: "run abc",
            receipt_dir: Some(Path::new("custom receipts")),
            answers_path: Some(Path::new("my answers.json")),
        });

        assert_eq!(
            command,
            "runx resume 'run abc' 'my answers.json' --receipt-dir 'custom receipts'"
        );
    }

    #[test]
    fn resume_command_uses_safe_defaults_when_metadata_is_missing() {
        let command = render_skill_resume_command(SkillResumeCommand {
            run_id: "rx_123",
            receipt_dir: None,
            answers_path: None,
        });

        assert_eq!(command, "runx resume rx_123 -");
    }
}
