//! `runx secret`: the one place a secret the operator keeps in runx is set,
//! listed, removed and read. A connector binding (`connector use --secret`),
//! the managed agent (`config set agent.secret`) and a host (`secret read`)
//! name a secret; none of them holds its value.

use std::ffi::OsString;
use std::process::ExitCode;

use runx_contracts::connector::{SecretName, SecretReference};
use runx_runtime::WorkspaceEnv;
use runx_runtime::secrets::{self, SecretSource, SecretSummary};
use serde::Serialize;

use crate::cli_args::{flag_value, os_arg, split_flag};

const USAGE: &str = "runx secret set <name> [--from <reference>] | list | rm <name> | read <name>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretPlan {
    pub action: SecretAction,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretAction {
    /// Set `name` to a value given without echo or on stdin, or to where a
    /// secrets manager holds it.
    Set {
        name: SecretName,
        from: Option<SecretReference>,
    },
    List,
    Remove {
        name: SecretName,
    },
    /// Print `name`'s value, for a program the operator runs.
    Read {
        name: SecretName,
    },
}

pub fn parse_secret_plan(args: &[OsString]) -> Result<SecretPlan, String> {
    let subcommand = args
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("usage: {USAGE}"))?;
    let mut positionals = Vec::new();
    let mut from = None;
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        let token = os_arg(args, index, "secret")?;
        let (flag, inline) = split_flag(token);
        match flag {
            "--json" | "-j" if inline.is_none() => json = true,
            "--from" => {
                let (value, next) = flag_value(args, index, flag, inline, "secret")?;
                if from.replace(value).is_some() {
                    return Err("runx secret set takes --from once".to_owned());
                }
                index = next;
                continue;
            }
            _ if token.starts_with('-') => return Err(format!("unknown secret argument {token}")),
            _ => positionals.push(token.to_owned()),
        }
        index += 1;
    }
    let name = |value: &String| {
        SecretName::new(value.as_str()).ok_or_else(|| format!("{value} is not a secret name"))
    };
    // A value is never an argument: one passed by mistake must not reach
    // the shell's history, a log or another user's process list.
    let action = match (subcommand, positionals.as_slice(), from) {
        ("set", [secret], from) => SecretAction::Set {
            name: name(secret)?,
            from: from
                .map(|value| {
                    SecretReference::new(value).ok_or_else(|| {
                        runx_contracts::connector::SECRET_REFERENCE_EXPECTED.to_owned()
                    })
                })
                .transpose()?,
        },
        ("set", [_, ..], None) => {
            return Err(
                "runx secret set takes the value without echo or on stdin, never as an argument"
                    .to_owned(),
            );
        }
        ("list", [], None) => SecretAction::List,
        ("rm", [secret], None) => SecretAction::Remove {
            name: name(secret)?,
        },
        ("read", [secret], None) if !json => SecretAction::Read {
            name: name(secret)?,
        },
        ("read", [_], None) => {
            return Err("runx secret read prints the value alone; it takes no --json".to_owned());
        }
        _ => return Err(format!("usage: {USAGE}")),
    };
    Ok(SecretPlan { action, json })
}

pub fn run_native_secret(plan: SecretPlan, workspace: &WorkspaceEnv) -> ExitCode {
    match execute(&plan, workspace) {
        Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
        Err(error) if plan.json => crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(&error, "secret_failed"),
            1,
        ),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx secret: {error}\n"), 1),
    }
}

fn execute(plan: &SecretPlan, workspace: &WorkspaceEnv) -> Result<String, String> {
    let paths = workspace.paths();
    let failed = |error: secrets::SecretError| error.to_string();
    match &plan.action {
        SecretAction::Set { name, from } => {
            match from {
                Some(reference) => {
                    secrets::set_reference(workspace.env(), workspace.cwd(), name, reference)
                }
                None => secrets::set_value(
                    paths,
                    name,
                    &crate::cli_io::read_secret(&format!("secret {name}"))?,
                ),
            }
            .map_err(failed)?;
            let summary = secrets::list(paths)
                .map_err(failed)?
                .into_iter()
                .find(|summary| summary.name == *name)
                .ok_or_else(|| format!("secret {name} was not kept"))?;
            if plan.json {
                return render_json(&Output::Set { secret: &summary });
            }
            Ok(format!("secret {}\n", line(&summary)))
        }
        SecretAction::List => {
            let listed = secrets::list(paths).map_err(failed)?;
            if plan.json {
                return render_json(&Output::List { secrets: &listed });
            }
            if listed.is_empty() {
                return Ok("no secrets; set one with `runx secret set <name>`\n".to_owned());
            }
            Ok(listed
                .iter()
                .map(|summary| format!("{}\n", line(summary)))
                .collect())
        }
        SecretAction::Remove { name } => {
            if !secrets::remove(paths, name).map_err(failed)? {
                return Err(format!("no secret is named {name}"));
            }
            if plan.json {
                return render_json(&Output::Remove { removed: name });
            }
            Ok(format!(
                "removed secret {name}; whatever still names it refuses until it is set again\n"
            ))
        }
        SecretAction::Read { name } => {
            let value = secrets::reveal(workspace.env(), workspace.cwd(), name).map_err(failed)?;
            Ok(format!("{value}\n"))
        }
    }
}

/// A secret as one line: its name and where its value is, never the value.
fn line(summary: &SecretSummary) -> String {
    match &summary.source {
        SecretSource::Stored => format!("{}  stored in the runx home", summary.name),
        SecretSource::Reference {
            reference,
            pinned: Some(pinned),
        } => format!("{}  {reference}  at {pinned}", summary.name),
        SecretSource::Reference {
            reference,
            pinned: None,
        } => format!("{}  {reference}", summary.name),
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Output<'a> {
    Set { secret: &'a SecretSummary },
    List { secrets: &'a [SecretSummary] },
    Remove { removed: &'a SecretName },
}

fn render_json(output: &Output<'_>) -> Result<String, String> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        status: &'static str,
        #[serde(flatten)]
        output: &'a Output<'a>,
    }
    serde_json::to_string_pretty(&Envelope {
        status: "success",
        output,
    })
    .map(|value| format!("{value}\n"))
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(args: &[&str]) -> Result<SecretPlan, String> {
        parse_secret_plan(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    fn named(value: &str) -> Result<SecretName, String> {
        SecretName::new(value).ok_or_else(|| "a secret name".to_owned())
    }

    #[test]
    fn each_command_parses_and_a_value_is_never_an_argument() -> Result<(), String> {
        assert_eq!(
            plan(&["secret", "set", "github"])?.action,
            SecretAction::Set {
                name: named("github")?,
                from: None
            }
        );
        assert_eq!(
            plan(&["secret", "set", "sentry", "--from", "op://Dev/Sentry/token"])?.action,
            SecretAction::Set {
                name: named("sentry")?,
                from: SecretReference::new("op://Dev/Sentry/token"),
            }
        );
        assert_eq!(plan(&["secret", "list", "-j"])?.action, SecretAction::List);
        assert_eq!(
            plan(&["secret", "rm", "github"])?.action,
            SecretAction::Remove {
                name: named("github")?
            }
        );
        assert_eq!(
            plan(&["secret", "read", "gemini"])?.action,
            SecretAction::Read {
                name: named("gemini")?
            }
        );
        assert_eq!(
            plan(&["secret", "set", "github", "ghp_on_argv"]),
            Err(
                "runx secret set takes the value without echo or on stdin, never as an argument"
                    .to_owned()
            )
        );
        // The references a secret is set from keep their managers' own forms.
        for reference in [
            "op://Private/Exa API/credential",
            "op://Nitrosend/Sentry/token",
            "aws-sm://team/keys#EXA_API_KEY",
            "aws-sm://arn:aws:secretsmanager:eu-west-2:111122223333:secret:team/keys-AbCdEf#KEY",
            "aws-ps:///team/exa-api-key",
            "vault://secret/team/exa#key",
            "keychain://exa/work",
            "file:///run/secrets/exa_api_key",
        ] {
            assert_eq!(
                plan(&["secret", "set", "exa", "--from", reference])?.action,
                SecretAction::Set {
                    name: named("exa")?,
                    from: Some(SecretReference::new(reference).ok_or("a reference")?),
                },
                "{reference}"
            );
        }
        assert!(plan(&["secret", "set", "x", "--from", "not-a-reference"]).is_err());
        assert!(plan(&["secret", "read", "gemini", "-j"]).is_err());
        assert!(plan(&["secret", "rm", "x", "--from", "op://a/b/c"]).is_err());
        assert!(plan(&["secret"]).is_err());
        Ok(())
    }
}
