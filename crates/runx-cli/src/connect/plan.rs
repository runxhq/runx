use std::ffi::OsString;

use runx_runtime::validate_provider_grant_id;

use crate::cli_args::{flag_value, os_arg, split_flag};

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectPlan {
    pub action: ConnectAction,
    pub api_base_url: Option<String>,
    pub allow_local_api: bool,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnectAction {
    List,
    Start(ConnectStartPlan),
    Status { session_id: String },
    Revoke { grant_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectStartPlan {
    pub provider: String,
    pub scopes: Vec<String>,
    pub credentials_from_stdin: bool,
    pub scope_family: Option<String>,
    pub authority_kind: Option<String>,
    pub target_repo: Option<String>,
    pub target_locator: Option<String>,
    pub binding_id: Option<String>,
    pub credential_grant_id: Option<String>,
}

#[derive(Default)]
struct ParsedConnectArgs {
    api_base_url: Option<String>,
    allow_local_api: bool,
    credentials_from_stdin: bool,
    json: bool,
    scopes: Vec<String>,
    scope_family: Option<String>,
    authority_kind: Option<String>,
    target_repo: Option<String>,
    target_locator: Option<String>,
    binding_id: Option<String>,
    credential_grant_id: Option<String>,
    positional: Vec<String>,
}

pub fn parse_connect_plan(args: &[OsString]) -> Result<ConnectPlan, String> {
    let subcommand = args
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| "runx connect requires list, start, status, or revoke".to_owned())?;
    let mut parsed = parse_args(args)?;
    let action = connect_action(subcommand, &mut parsed)?;
    Ok(ConnectPlan {
        action,
        api_base_url: parsed.api_base_url,
        allow_local_api: parsed.allow_local_api,
        json: parsed.json,
    })
}

fn parse_args(args: &[OsString]) -> Result<ParsedConnectArgs, String> {
    let mut parsed = ParsedConnectArgs::default();
    let mut index = 2;
    while index < args.len() {
        let arg = os_arg(args, index, "connect")?;
        if !arg.starts_with('-') {
            parsed.positional.push(arg.to_owned());
            index += 1;
            continue;
        }
        let (flag, inline_value) = split_flag(arg);
        if apply_switch(&mut parsed, flag, inline_value)? {
            index += 1;
            continue;
        }
        if !takes_value(flag) {
            return Err(format!("unknown connect flag {flag}"));
        }
        let (value, next) = flag_value(args, index, flag, inline_value, "connect")?;
        apply_value(&mut parsed, flag, value)?;
        index = next;
    }
    Ok(parsed)
}

fn apply_switch(
    parsed: &mut ParsedConnectArgs,
    flag: &str,
    inline_value: Option<&str>,
) -> Result<bool, String> {
    let target = match flag {
        "--json" | "-j" => Some(&mut parsed.json),
        "--allow-local-api" => Some(&mut parsed.allow_local_api),
        "--credentials-from-stdin" => Some(&mut parsed.credentials_from_stdin),
        _ => None,
    };
    let Some(target) = target else {
        return Ok(false);
    };
    if inline_value.is_some() {
        return Err(format!("{flag} does not take a value"));
    }
    *target = true;
    Ok(true)
}

fn takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--api-base-url"
            | "--scope"
            | "--scope-family"
            | "--authority-kind"
            | "--target-repo"
            | "--target-locator"
            | "--binding"
            | "--credential-grant"
    )
}

fn apply_value(parsed: &mut ParsedConnectArgs, flag: &str, value: String) -> Result<(), String> {
    match flag {
        "--api-base-url" => parsed.api_base_url = Some(value),
        "--scope" if value.trim().is_empty() => {
            return Err("--scope requires a non-blank capability".to_owned());
        }
        "--scope" => parsed.scopes.push(value),
        "--scope-family" => parsed.scope_family = Some(value),
        "--authority-kind" => parsed.authority_kind = Some(value),
        "--target-repo" => parsed.target_repo = Some(value),
        "--target-locator" => parsed.target_locator = Some(value),
        "--binding" => parsed.binding_id = Some(value),
        "--credential-grant" => parsed.credential_grant_id = Some(provider_grant_id(value)?),
        _ => return Err(format!("unknown connect flag {flag}")),
    }
    Ok(())
}

fn connect_action(
    subcommand: &str,
    parsed: &mut ParsedConnectArgs,
) -> Result<ConnectAction, String> {
    if subcommand != "start" && parsed.credentials_from_stdin {
        return Err("--credentials-from-stdin is only valid for runx connect start".to_owned());
    }
    match subcommand {
        "list" if parsed.positional.is_empty() => Ok(ConnectAction::List),
        "start" if parsed.positional.len() == 1 => start_action(parsed),
        "status" if parsed.positional.len() == 1 => Ok(ConnectAction::Status {
            session_id: path_identifier("session id", parsed.positional.remove(0))?,
        }),
        "revoke" if parsed.positional.len() == 1 => Ok(ConnectAction::Revoke {
            grant_id: provider_grant_id(parsed.positional.remove(0))?,
        }),
        "list" | "start" | "status" | "revoke" => {
            Err(format!("invalid arguments for runx connect {subcommand}"))
        }
        _ => Err(format!("unknown connect subcommand {subcommand}")),
    }
}

fn start_action(parsed: &mut ParsedConnectArgs) -> Result<ConnectAction, String> {
    if parsed.scopes.is_empty() {
        return Err("runx connect start requires at least one --scope capability".to_owned());
    }
    if parsed.credentials_from_stdin && parsed.credential_grant_id.is_some() {
        return Err(
            "--credentials-from-stdin and --credential-grant are mutually exclusive".to_owned(),
        );
    }
    if parsed.binding_id.is_some() && parsed.credential_grant_id.is_some() {
        return Err("--binding and --credential-grant are mutually exclusive".to_owned());
    }
    Ok(ConnectAction::Start(ConnectStartPlan {
        provider: parsed.positional.remove(0),
        scopes: std::mem::take(&mut parsed.scopes),
        credentials_from_stdin: parsed.credentials_from_stdin,
        scope_family: parsed.scope_family.take(),
        authority_kind: parsed.authority_kind.take(),
        target_repo: parsed.target_repo.take(),
        target_locator: parsed.target_locator.take(),
        binding_id: parsed.binding_id.take(),
        credential_grant_id: parsed.credential_grant_id.take(),
    }))
}

fn path_identifier(label: &str, value: String) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 200
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '?' | '#'))
    {
        return Err(format!(
            "runx connect {label} must be a safe, non-empty URL path identifier"
        ));
    }
    Ok(value.to_owned())
}

fn provider_grant_id(value: String) -> Result<String, String> {
    let value = value.trim();
    validate_provider_grant_id(value).map_err(|error| error.to_string())?;
    Ok(value.to_owned())
}
