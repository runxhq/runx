//! Assistant argument parsing and presentation over the OSS operator application.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::cli_args::{flag_value, os_arg, split_flag};
use runx_assistant::{AssistantAction, AssistantRequest};
use runx_runtime::WorkspaceEnv;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssistantPlan {
    pub action: AssistantAction,
    pub profile_path: PathBuf,
    pub json: bool,
    pub memory_id: Option<String>,
    pub text_file: Option<PathBuf>,
    pub cursor: Option<String>,
    pub work_id: Option<String>,
    pub message_locator: Option<String>,
}

pub fn parse_assistant_plan(args: &[OsString]) -> Result<AssistantPlan, String> {
    if args.first().and_then(|part| part.to_str()) != Some("assistant") {
        return Err("internal error: assistant dispatcher received another command".to_owned());
    }
    let action = match os_arg(args, 1, "assistant")? {
        "check" => AssistantAction::Check,
        "tick" => AssistantAction::Tick,
        "status" => AssistantAction::Status,
        "pause" => AssistantAction::Pause,
        "resume" => AssistantAction::Resume,
        "grant" => AssistantAction::Grant,
        "revoke" => AssistantAction::Revoke,
        "install-timer" => AssistantAction::InstallTimer,
        "remove-timer" => AssistantAction::RemoveTimer,
        "remember" => AssistantAction::Remember,
        "forget" => AssistantAction::Forget,
        "memories" => AssistantAction::Memories,
        "work" => AssistantAction::Work,
        "retry-work" => AssistantAction::RetryWork,
        "execute" => AssistantAction::Execute,
        "report" => AssistantAction::Report,
        "discard-notification" => AssistantAction::DiscardNotification,
        "reconcile-notification" => AssistantAction::ReconcileNotification,
        _ => return Err("assistant action must be check, tick, execute, status, report, pause, resume, grant, revoke, install-timer, remove-timer, remember, forget, memories, work, retry-work, discard-notification or reconcile-notification".to_owned()),
    };
    let mut profile_path = None;
    let mut json = false;
    let mut memory_id = None;
    let mut text_file = None;
    let mut cursor = None;
    let mut work_id = None;
    let mut message_locator = None;
    let mut index = 2;
    while index < args.len() {
        let token = os_arg(args, index, "assistant")?;
        let (flag, inline) = split_flag(token);
        match flag {
            "--profile" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if profile_path.replace(PathBuf::from(value)).is_some() {
                    return Err("assistant --profile may be supplied only once".to_owned());
                }
                index = next;
            }
            "--json" | "-j" if inline.is_none() => {
                json = true;
                index += 1;
            }
            "--memory-id" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if memory_id.replace(value.to_owned()).is_some() {
                    return Err("assistant --memory-id may be supplied only once".to_owned());
                }
                index = next;
            }
            "--text-file" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if text_file.replace(PathBuf::from(value)).is_some() {
                    return Err("assistant --text-file may be supplied only once".to_owned());
                }
                index = next;
            }
            "--cursor" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if value.is_empty()
                    || value.len() > 500
                    || cursor.replace(value.to_owned()).is_some()
                {
                    return Err(
                        "assistant --cursor must be a single bounded nonempty value".to_owned()
                    );
                }
                index = next;
            }
            "--work-id" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if value.is_empty()
                    || value.len() > 80
                    || work_id.replace(value.to_owned()).is_some()
                {
                    return Err("assistant --work-id must be one bounded nonempty ID".to_owned());
                }
                index = next;
            }
            "--message-locator" => {
                let (value, next) = flag_value(args, index, flag, inline, "assistant")?;
                if value.is_empty()
                    || value.len() > 400
                    || message_locator.replace(value.to_owned()).is_some()
                {
                    return Err(
                        "assistant --message-locator must be one bounded nonempty locator"
                            .to_owned(),
                    );
                }
                index = next;
            }
            _ => return Err(format!("unknown assistant argument {token}")),
        }
    }
    match action {
        AssistantAction::Remember if memory_id.is_some() && text_file.is_some() => {}
        AssistantAction::Forget if memory_id.is_some() && text_file.is_none() => {}
        AssistantAction::Remember | AssistantAction::Forget => {
            return Err(
                "remember requires --memory-id and --text-file; forget requires --memory-id only"
                    .to_owned(),
            );
        }
        _ if memory_id.is_some() || text_file.is_some() => {
            return Err("assistant memory flags are only valid for remember or forget".to_owned());
        }
        _ => {}
    }
    if cursor.is_some() && action != AssistantAction::Work {
        return Err("assistant --cursor is only valid for work".to_owned());
    }
    if (action == AssistantAction::RetryWork) != work_id.is_some() {
        return Err("retry-work requires --work-id; other actions cannot use it".to_owned());
    }
    if (action == AssistantAction::ReconcileNotification) != message_locator.is_some() {
        return Err(
            "reconcile-notification requires --message-locator; other actions cannot use it"
                .to_owned(),
        );
    }
    Ok(AssistantPlan {
        action,
        profile_path: profile_path.ok_or("assistant requires --profile path")?,
        json,
        memory_id,
        text_file,
        cursor,
        work_id,
        message_locator,
    })
}

pub fn run_native_assistant(plan: AssistantPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let request = AssistantRequest {
        action: plan.action,
        profile_path: plan.profile_path.clone(),
        memory_id: plan.memory_id.clone(),
        text_file: plan.text_file.clone(),
        cursor: plan.cursor.clone(),
        work_id: plan.work_id.clone(),
        message_locator: plan.message_locator.clone(),
    };
    let output = runx_assistant::execute(&request, workspace);
    match output {
        Ok(value) => {
            let rendered = if plan.json {
                serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned())
            } else {
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned())
            };
            crate::cli_io::write_stdout_code(&format!("{rendered}\n"), 0)
        }
        Err(message) if plan.json => crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(&message, "assistant_error"),
            1,
        ),
        Err(message) => {
            let _ignored = crate::cli_io::write_stderr_code(&format!("runx: {message}\n"));
            ExitCode::from(1)
        }
    }
}
