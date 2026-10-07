use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::JsonValue;
use runx_runtime::{
    HostedApiOperationError, ReceiptPublishResponse, RuntimeHttpError, WorkspaceEnv,
};

use crate::cli_args::{flag_value, os_arg, split_flag};

#[derive(Debug, Eq, PartialEq)]
pub struct PublishPlan {
    pub receipt_path: PathBuf,
    pub api_base_url: Option<String>,
    pub allow_local_api: bool,
    pub json: bool,
}

#[derive(Debug)]
pub enum PublishCliError {
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

impl fmt::Display for PublishCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingReceipt => write!(formatter, "runx publish requires a receipt JSON path"),
            Self::ExtraReceipt => write!(
                formatter,
                "runx publish accepts exactly one receipt JSON path"
            ),
            Self::UnknownFlag(flag) => write!(formatter, "unknown publish flag {flag}"),
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
                    code, detail, hint, ..
                } = error
                {
                    return formatter.write_str(&publish_error_message(
                        code,
                        detail,
                        hint.as_deref(),
                    ));
                }
                write!(formatter, "{error}")
            }
            Self::Serialize(message) => {
                write!(formatter, "failed to serialize publish result: {message}")
            }
        }
    }
}

impl std::error::Error for PublishCliError {}

impl From<HostedApiOperationError> for PublishCliError {
    fn from(error: HostedApiOperationError) -> Self {
        Self::Publish(error)
    }
}

impl From<runx_runtime::HostedApiError> for PublishCliError {
    fn from(error: runx_runtime::HostedApiError) -> Self {
        Self::Environment(error.to_string())
    }
}

fn publish_error_message(code: &str, detail: &str, hint: Option<&str>) -> String {
    if code == "missing_scope" && detail.contains("receipts:write") {
        return [
            "This token can publish skills but not receipts.",
            "The receipt notary requires `receipts:write`.",
            "Set `RUNX_PUBLIC_API_TOKEN` to a receipt-capable token.",
            "Your stored login token is still valid for the scopes it was issued with.",
        ]
        .join(" ");
    }
    let mut message = format!("runx-api publish returned error [{code}]: {detail}");
    if let Some(hint) = hint.filter(|value| !value.trim().is_empty()) {
        message.push_str(&format!(" Hint: {hint}"));
    }
    message
}

pub fn parse_publish_plan(args: &[OsString]) -> Result<PublishPlan, String> {
    let mut receipt_path = None;
    let mut api_base_url = None;
    let mut allow_local_api = false;
    let mut json = false;
    let mut index = 1;
    while index < args.len() {
        let arg = os_arg(args, index, "publish")?;
        if !arg.starts_with('-') {
            if receipt_path.is_some() {
                return Err(PublishCliError::ExtraReceipt.to_string());
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
                let (value, next_index) = flag_value(args, index, flag, inline_value, "publish")?;
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
            _ => return Err(PublishCliError::UnknownFlag(flag.to_owned()).to_string()),
        }
    }
    Ok(PublishPlan {
        receipt_path: receipt_path.ok_or_else(|| PublishCliError::MissingReceipt.to_string())?,
        api_base_url,
        allow_local_api,
        json,
    })
}

pub fn run_native_publish(plan: PublishPlan, workspace: &WorkspaceEnv) -> ExitCode {
    match run_publish_command(&plan, workspace.env(), workspace.cwd()) {
        Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
        Err(error) => {
            if plan.json {
                let body = serde_json::json!({
                    "status": "failure",
                    "error": {
                        "message": error.to_string(),
                        "code": "publish_failed",
                    },
                });
                let serialized = serde_json::to_string_pretty(&body)
                    .unwrap_or_else(|_| "{\"status\":\"failure\"}".to_owned());
                return crate::cli_io::write_stdout_code(&format!("{serialized}\n"), 1);
            }
            let _ignored = crate::cli_io::write_stderr(&format!("runx publish: {error}\n"));
            ExitCode::from(1)
        }
    }
}

fn run_publish_command(
    plan: &PublishPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<String, PublishCliError> {
    let receipt = read_receipt_json(&plan.receipt_path)?;
    let environment = runx_runtime::HostedApiEnvironment::resolve_publish(
        plan.api_base_url.as_deref(),
        env,
        cwd,
    )?;
    let transport = runx_runtime::hosted_api_transport(allow_local_api(plan, env))
        .map_err(PublishCliError::TransportInit)?;
    let authenticated = environment.authenticate(&transport)?;
    let response = runx_runtime::publish_hosted_receipt(
        &transport,
        authenticated.base_url(),
        authenticated.token(),
        &receipt,
    )?;
    render_publish_result(plan.json, &response)
}

fn allow_local_api(plan: &PublishPlan, env: &BTreeMap<String, String>) -> bool {
    runx_runtime::hosted_private_network_allowed(plan.allow_local_api, env)
}

fn read_receipt_json(path: &PathBuf) -> Result<JsonValue, PublishCliError> {
    let text = fs::read_to_string(path).map_err(|error| PublishCliError::ReadReceipt {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    serde_json::from_str(&text).map_err(|error| PublishCliError::InvalidReceiptJson {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

fn render_publish_result(
    json: bool,
    response: &ReceiptPublishResponse,
) -> Result<String, PublishCliError> {
    if json {
        return serde_json::to_string_pretty(response)
            .map(|value| format!("{value}\n"))
            .map_err(|error| PublishCliError::Serialize(error.to_string()));
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
    out.push_str(&format!("  status:      {}\n", response.status));
    out.push_str(&format!("  published:   {}\n", response.published));
    out.push_str(&format!("  public hash: {}\n", response.public_hash));
    if let Some(receipt_id) = &response.receipt_id {
        out.push_str(&format!("  receipt id:  {receipt_id}\n"));
    }
    if let Some(url) = &response.public_url {
        out.push_str(&format!("  public url:  {url}\n"));
    }
    if let Some(replay_status) = &response.replay_status {
        out.push_str(&format!("  replay:      {replay_status}\n"));
    }
    if let Some(verdict) = &response.verdict {
        out.push_str(&format!(
            "  verdict:     {}\n",
            compact_json(verdict).map_err(|error| PublishCliError::Serialize(error.to_string()))?
        ));
    }
    Ok(out)
}

fn compact_json(value: &JsonValue) -> Result<String, serde_json::Error> {
    serde_json::to_string(value)
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod publish_tests;
