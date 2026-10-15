use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::ResumeDocument;
use runx_runtime::{ManagedAgentPolicy, ResumeRequest, WorkspaceEnv};

use crate::managed_agent::{managed_agent_policy, parse_boolean_flag, parse_managed_agent_rounds};

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

/// Continue a paused run with the host's answers. The run's skill, runner
/// and digests come from its record; pinned digests must match them.
pub fn run_native_resume_with_workspace(plan: ResumePlan, workspace: &WorkspaceEnv) -> ExitCode {
    let answers = match read_answers(&plan.answers_path, workspace) {
        Ok(answers) => answers,
        Err(error) => return write_resume_failure(&error, plan.json, 1),
    };
    let _driver = match crate::runtime::drive(workspace, &plan.run_id) {
        Ok(driver) => driver,
        Err(error) => return write_resume_failure(&error, plan.json, 1),
    };
    if let Err(error) = crate::runtime::stop_on_terminate() {
        return write_resume_failure(&error, plan.json, 1);
    }
    let orchestrator = match crate::runtime::local_orchestrator(workspace.env()) {
        Ok(orchestrator) => orchestrator,
        Err(error) => {
            return write_resume_failure(
                &format!("failed to initialize runtime effects: {error}"),
                plan.json,
                1,
            );
        }
    };
    let result = orchestrator.resume(&ResumeRequest {
        run_id: plan.run_id,
        answers,
        receipt_dir: plan.receipt_dir.clone(),
        env: workspace.env().clone(),
        cwd: workspace.cwd().to_path_buf(),
        managed_agent: plan.managed_agent,
        package_digest: plan.expected_package_digest,
        execution_closure_digest: plan.expected_execution_closure_digest,
    });
    match result {
        Ok(result) => crate::skill::write_run_result(
            &result,
            crate::skill::RunOutput {
                json: plan.json,
                diagnostics: plan.diagnostics,
                receipt_dir: plan.receipt_dir.as_deref(),
                answers_path: Some(&plan.answers_path),
                artifacts: &workspace.paths().artifacts(),
            },
        ),
        Err(error) => write_resume_failure(&error.to_string(), plan.json, 1),
    }
}

/// The resume document (`runx.resume.v1`) at `path`, or on stdin for `-`,
/// read under the same bound as a skill's inputs. Nothing is written: the run
/// records what it consumes.
fn read_answers(path: &Path, workspace: &WorkspaceEnv) -> Result<ResumeDocument, String> {
    let text = if path == Path::new("-") {
        crate::document_input::read_document_input(
            &crate::document_input::DocumentInputSource::Stdin,
            workspace.env(),
            workspace.cwd(),
        )
    } else {
        crate::document_input::read_operator_document(path)
    }
    .map_err(|error| format!("failed to read resume answers: {error}"))?;
    serde_json::from_str::<ResumeDocument>(&text).map_err(|error| {
        format!(
            "resume answers are not a runx.resume.v1 document: {error}; each entry under answers and approvals carries the request_digest the run reported"
        )
    })
}

fn binding_flag_value(flag: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{flag} requires a non-empty value"));
    }
    Ok(value.to_owned())
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
