use std::ffi::OsString;

use crate::cli_args::{os_arg, split_flag};

use super::{CredentialAction, CredentialPlan};

pub fn parse_credential_plan(args: &[OsString]) -> Result<CredentialPlan, String> {
    if os_arg(args, 0, "credential")? != "credential" {
        return Err("credential parser requires the credential command".to_owned());
    }
    let subcommand = args
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| "runx credential requires set, list, or remove".to_owned())?;
    match subcommand {
        "set" => parse_set(args),
        "list" => parse_list(args),
        "remove" => parse_remove(args),
        _ => Err(format!("unknown credential subcommand {subcommand}")),
    }
}

fn parse_set(args: &[OsString]) -> Result<CredentialPlan, String> {
    let connector = positional(args, 2, "runx credential set requires <connector>")?;
    let mut credential = None;
    let mut profile = None;
    let mut scopes = Vec::new();
    let mut parameters = std::collections::BTreeMap::new();
    let mut from_stdin = false;
    let mut json = false;
    let mut index = 3;
    while index < args.len() {
        let token = os_arg(args, index, "credential")?;
        if !token.starts_with('-') {
            return Err(
                "credential secret material must be provided through --from-stdin".to_owned(),
            );
        }
        let (flag, inline) = split_flag(token);
        match flag {
            "--credential" => credential = Some(flag_value(args, &mut index, flag, inline)?),
            "--profile" => profile = Some(flag_value(args, &mut index, flag, inline)?),
            "--scope" => scopes.push(flag_value(args, &mut index, flag, inline)?),
            "--parameter" => {
                let (name, value) =
                    crate::connector::parameter(&flag_value(args, &mut index, flag, inline)?)?;
                if parameters.insert(name.clone(), value).is_some() {
                    return Err(format!("runx credential set sets parameter {name} once"));
                }
            }
            "--from-stdin" => {
                reject_inline(flag, inline)?;
                from_stdin = true;
            }
            "--json" | "-j" => {
                reject_inline(flag, inline)?;
                json = true;
            }
            _ => return Err(format!("unknown credential set flag {flag}")),
        }
        index += 1;
    }
    if !from_stdin {
        return Err("runx credential set requires --from-stdin".to_owned());
    }
    let credential =
        credential.ok_or_else(|| "runx credential set requires --credential <name>".to_owned())?;
    Ok(CredentialPlan {
        action: CredentialAction::Set {
            profile: profile.unwrap_or_else(|| connector.clone()),
            connector,
            credential,
            scopes,
            parameters,
            from_stdin,
        },
        json,
    })
}

fn parse_list(args: &[OsString]) -> Result<CredentialPlan, String> {
    let json = parse_json_only(args, 2, "credential list")?;
    Ok(CredentialPlan {
        action: CredentialAction::List,
        json,
    })
}

fn parse_remove(args: &[OsString]) -> Result<CredentialPlan, String> {
    let profile = positional(args, 2, "runx credential remove requires <profile>")?;
    let json = parse_json_only(args, 3, "credential remove")?;
    Ok(CredentialPlan {
        action: CredentialAction::Remove { profile },
        json,
    })
}

fn positional(args: &[OsString], index: usize, message: &str) -> Result<String, String> {
    args.get(index)
        .and_then(|value| value.to_str())
        .filter(|value| !value.starts_with('-') && !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| message.to_owned())
}

fn flag_value(
    args: &[OsString],
    index: &mut usize,
    flag: &str,
    inline: Option<&str>,
) -> Result<String, String> {
    let value = if let Some(value) = inline {
        value.to_owned()
    } else {
        *index += 1;
        os_arg(args, *index, "credential")?.to_owned()
    };
    if value.trim().is_empty() {
        return Err(format!("{flag} requires a non-empty value"));
    }
    Ok(value)
}

fn reject_inline(flag: &str, inline: Option<&str>) -> Result<(), String> {
    if inline.is_some() {
        return Err(format!("{flag} does not take a value"));
    }
    Ok(())
}

fn parse_json_only(args: &[OsString], start: usize, command: &str) -> Result<bool, String> {
    let mut json = false;
    for index in start..args.len() {
        match os_arg(args, index, "credential")? {
            "--json" | "-j" => json = true,
            token => return Err(format!("runx {command} does not accept {token}")),
        }
    }
    Ok(json)
}
