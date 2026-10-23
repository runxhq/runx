use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::JsonValue;
use runx_runtime::{HostedApiOperationError, ReceiptNotarization, RuntimeHttpError, WorkspaceEnv};

use crate::cli_args::{flag_value, os_arg, split_flag};

#[derive(Debug, Eq, PartialEq)]
pub struct NotarizePlan {
    pub receipt_path: PathBuf,
    pub api_base_url: Option<String>,
    pub allow_local_api: bool,
    pub json: bool,
}

#[derive(Debug)]
pub enum NotarizeCliError {
    MissingReceipt,
    ExtraReceipt,
    UnknownFlag(String),
    ReadReceipt { path: String, message: String },
    InvalidReceiptJson { path: String, message: String },
    TransportInit(RuntimeHttpError),
    Environment(String),
    Publish(HostedApiOperationError),
    Serialize(String),
}

impl fmt::Display for NotarizeCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingReceipt => write!(formatter, "runx notarize requires a receipt JSON path"),
            Self::ExtraReceipt => write!(
                formatter,
                "runx notarize accepts exactly one receipt JSON path"
            ),
            Self::UnknownFlag(flag) => write!(formatter, "unknown notarize flag {flag}"),
            Self::ReadReceipt { path, message } => {
                write!(formatter, "failed to read receipt {path}: {message}")
            }
            Self::InvalidReceiptJson { path, message } => {
                write!(formatter, "receipt {path} is not valid JSON: {message}")
            }
            Self::TransportInit(error) => {
                write!(formatter, "failed to initialize HTTP transport: {error}")
            }
            Self::Environment(error) => write!(formatter, "{error}"),
            Self::Publish(error) => {
                if let HostedApiOperationError::Api {
                    operation,
                    code,
                    detail,
                    hint,
                    ..
                } = error
                {
                    return formatter.write_str(&notarize_error_message(
                        operation,
                        code,
                        detail,
                        hint.as_deref(),
                    ));
                }
                write!(formatter, "{error}")
            }
            Self::Serialize(message) => {
                write!(formatter, "failed to serialize notarization: {message}")
            }
        }
    }
}

impl std::error::Error for NotarizeCliError {}

impl From<HostedApiOperationError> for NotarizeCliError {
    fn from(error: HostedApiOperationError) -> Self {
        Self::Publish(error)
    }
}

impl From<runx_runtime::HostedApiError> for NotarizeCliError {
    fn from(error: runx_runtime::HostedApiError) -> Self {
        Self::Environment(error.to_string())
    }
}

fn notarize_error_message(operation: &str, code: &str, detail: &str, hint: Option<&str>) -> String {
    // The receipt notary's only scope is `receipts:write`.
    if operation == "receipt notarize" && code == "missing_scope" {
        return [
            "This token can publish skills but not receipts.",
            "The receipt notary requires `receipts:write`.",
            "Set `RUNX_PUBLIC_API_TOKEN` to a receipt-capable token.",
            "Your stored login token is still valid for the scopes it was issued with.",
        ]
        .join(" ");
    }
    let mut message = format!("runx-api notarize returned error [{code}]: {detail}");
    if let Some(hint) = hint.filter(|value| !value.trim().is_empty()) {
        message.push_str(&format!(" Hint: {hint}"));
    }
    message
}

pub fn parse_notarize_plan(args: &[OsString]) -> Result<NotarizePlan, String> {
    let mut receipt_path = None;
    let mut api_base_url = None;
    let mut allow_local_api = false;
    let mut json = false;
    let mut index = 1;
    while index < args.len() {
        let arg = os_arg(args, index, "notarize")?;
        if !arg.starts_with('-') {
            if receipt_path.is_some() {
                return Err(NotarizeCliError::ExtraReceipt.to_string());
            }
            receipt_path = Some(PathBuf::from(arg));
            index += 1;
            continue;
        }
        let (flag, inline_value) = split_flag(arg);
        match flag {
            "--json" | "-j" => {
                if inline_value.is_some() {
                    return Err("--json does not take a value".to_owned());
                }
                json = true;
                index += 1;
            }
            "--api-base-url" => {
                let (value, next_index) = flag_value(args, index, flag, inline_value, "notarize")?;
                api_base_url = Some(value);
                index = next_index;
            }
            "--allow-local-api" => {
                if inline_value.is_some() {
                    return Err("--allow-local-api does not take a value".to_owned());
                }
                allow_local_api = true;
                index += 1;
            }
            _ => return Err(NotarizeCliError::UnknownFlag(flag.to_owned()).to_string()),
        }
    }
    Ok(NotarizePlan {
        receipt_path: receipt_path.ok_or_else(|| NotarizeCliError::MissingReceipt.to_string())?,
        api_base_url,
        allow_local_api,
        json,
    })
}

pub fn run_native_notarize(plan: NotarizePlan, workspace: &WorkspaceEnv) -> ExitCode {
    match run_notarize_command(&plan, workspace.env(), workspace.cwd()) {
        Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
        Err(error) => {
            if plan.json {
                let body = serde_json::json!({
                    "status": "failure",
                    "error": {
                        "message": error.to_string(),
                        "code": "notarize_failed",
                    },
                });
                let serialized = serde_json::to_string_pretty(&body)
                    .unwrap_or_else(|_| "{\"status\":\"failure\"}".to_owned());
                return crate::cli_io::write_stdout_code(&format!("{serialized}\n"), 1);
            }
            let _ignored = crate::cli_io::write_stderr(&format!("runx notarize: {error}\n"));
            ExitCode::from(1)
        }
    }
}

fn run_notarize_command(
    plan: &NotarizePlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<String, NotarizeCliError> {
    let receipt = read_receipt_json(&plan.receipt_path)?;
    let environment = runx_runtime::HostedApiEnvironment::resolve_publish(
        plan.api_base_url.as_deref(),
        env,
        cwd,
    )?;
    let transport = runx_runtime::hosted_api_transport(allow_local_api(plan, env))
        .map_err(NotarizeCliError::TransportInit)?;
    let authenticated = environment.authenticate(&transport)?;
    let response = runx_runtime::notarize_hosted_receipt(
        &transport,
        authenticated.base_url(),
        authenticated.token(),
        &receipt,
    )?;
    render_notarize_result(plan.json, &response)
}

fn allow_local_api(plan: &NotarizePlan, env: &BTreeMap<String, String>) -> bool {
    runx_runtime::hosted_private_network_allowed(plan.allow_local_api, env)
}

fn read_receipt_json(path: &PathBuf) -> Result<JsonValue, NotarizeCliError> {
    let text = fs::read_to_string(path).map_err(|error| NotarizeCliError::ReadReceipt {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    serde_json::from_str(&text).map_err(|error| NotarizeCliError::InvalidReceiptJson {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

fn render_notarize_result(
    json: bool,
    response: &ReceiptNotarization,
) -> Result<String, NotarizeCliError> {
    if json {
        return serde_json::to_string_pretty(response)
            .map(|value| format!("{value}\n"))
            .map_err(|error| NotarizeCliError::Serialize(error.to_string()));
    }
    let mut out = String::new();
    let verb = if response.published {
        "published"
    } else {
        "notarized"
    };
    out.push_str(&format!(
        "{verb} receipt {} ({})\n",
        response.digest, response.mode
    ));
    out.push_str(&format!("  status:        {}\n", response.status));
    out.push_str(&format!("  published:     {}\n", response.published));
    out.push_str(&format!("  public digest: {}\n", response.public_digest));
    if let Some(receipt_id) = &response.receipt_id {
        out.push_str(&format!("  receipt id:    {receipt_id}\n"));
    }
    if let Some(url) = &response.public_url {
        out.push_str(&format!("  public url:    {url}\n"));
    }
    if let Some(replay_status) = &response.replay_status {
        out.push_str(&format!("  replay:        {replay_status}\n"));
    }
    if let Some(verdict) = &response.verdict {
        out.push_str(&format!(
            "  verdict:       {}\n",
            compact_json(verdict)
                .map_err(|error| NotarizeCliError::Serialize(error.to_string()))?
        ));
    }
    Ok(out)
}

fn compact_json(value: &JsonValue) -> Result<String, serde_json::Error> {
    serde_json::to_string(value)
}

#[cfg(test)]
#[path = "notarize_tests.rs"]
mod notarize_tests;
