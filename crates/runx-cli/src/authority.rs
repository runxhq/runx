//! `runx authority`: the authorities a person saves in their runx home, by
//! name and for the workspaces they list, which a run or a server applies
//! only where it names one with `--authority <name>` (authority.md).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use runx_runtime::saved_authorities::{self, SavedAuthority};
use runx_runtime::{RunAuthority, WorkspaceEnv};
use serde::Serialize;

use crate::cli_args::{os_arg, os_flag_value, split_flag};
use crate::document_input::DocumentInputSource;

const USAGE: &str = "runx authority save <name> <file|-> [--workspace <path>...] | list | show <name> | remove <name>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorityPlan {
    pub action: AuthorityAction,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorityAction {
    /// Save the authority a file or stdin holds as `name`, for `workspaces`
    /// or, with none, the workspace it's made in.
    Save {
        name: String,
        source: DocumentInputSource,
        workspaces: Vec<PathBuf>,
    },
    List,
    Show {
        name: String,
    },
    Remove {
        name: String,
    },
}

pub fn parse_authority_plan(args: &[OsString]) -> Result<AuthorityPlan, String> {
    let subcommand = args
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("usage: {USAGE}"))?;
    let mut positionals = Vec::new();
    let mut workspaces = Vec::new();
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        let token = os_arg(args, index, "authority")?;
        let (flag, inline) = split_flag(token);
        match flag {
            "--json" | "-j" if inline.is_none() => json = true,
            "--workspace" => {
                let (value, next) = os_flag_value(args, index, flag, inline)?;
                workspaces.push(PathBuf::from(value));
                index = next;
                continue;
            }
            "-" => positionals.push(token.to_owned()),
            _ if token.starts_with('-') => {
                return Err(format!("unknown authority argument {token}"));
            }
            _ => positionals.push(token.to_owned()),
        }
        index += 1;
    }
    let action = match (subcommand, positionals.as_slice()) {
        ("save", [name, source]) => AuthorityAction::Save {
            name: name.clone(),
            source: if source == "-" {
                DocumentInputSource::Stdin
            } else {
                DocumentInputSource::Path(PathBuf::from(source))
            },
            workspaces,
        },
        ("list", []) if workspaces.is_empty() => AuthorityAction::List,
        ("show", [name]) if workspaces.is_empty() => AuthorityAction::Show { name: name.clone() },
        ("remove", [name]) if workspaces.is_empty() => {
            AuthorityAction::Remove { name: name.clone() }
        }
        _ => return Err(format!("usage: {USAGE}")),
    };
    Ok(AuthorityPlan { action, json })
}

pub fn run_native_authority(plan: AuthorityPlan, workspace: &WorkspaceEnv) -> ExitCode {
    match execute(&plan, workspace) {
        Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
        Err(error) if plan.json => crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(&error, "authority_failed"),
            1,
        ),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx authority: {error}\n"), 1),
    }
}

/// The authority saved as `name`, for a run or a server started in
/// `workspace` that names it with `--authority`.
///
/// # Errors
///
/// Returns why it doesn't apply here.
pub fn named_authority(name: &str, workspace: &WorkspaceEnv) -> Result<RunAuthority, String> {
    saved_authorities::named(workspace.paths(), name, &runx_runtime::now_iso8601())
        .map_err(|error| error.to_string())
}

fn execute(plan: &AuthorityPlan, workspace: &WorkspaceEnv) -> Result<String, String> {
    let paths = workspace.paths();
    let failed = |error: saved_authorities::SavedAuthorityError| error.to_string();
    match &plan.action {
        AuthorityAction::Save {
            name,
            source,
            workspaces,
        } => {
            let text = match source {
                DocumentInputSource::Path(path) => {
                    crate::document_input::read_operator_document(path)
                }
                DocumentInputSource::Stdin => crate::document_input::read_document_input(
                    source,
                    workspace.env(),
                    workspace.cwd(),
                ),
            }
            .map_err(|error| error.to_string())?;
            let authority: RunAuthority = serde_json::from_str(&text).map_err(|error| {
                format!("the authority to save is not a run authority: {error}")
            })?;
            let workspaces = workspaces
                .iter()
                .map(|path| workspace.cwd().join(path))
                .collect::<Vec<_>>();
            let saved = saved_authorities::save(
                workspace.env(),
                workspace.cwd(),
                name,
                authority,
                &workspaces,
                &runx_runtime::now_iso8601(),
            )
            .map_err(failed)?;
            render(
                plan.json,
                &Shown {
                    name,
                    saved: &saved,
                },
                || {
                    format!(
                        "saved authority {name} for {} workspace(s); name it with --authority {name}\n",
                        saved.workspaces.len()
                    )
                },
            )
        }
        AuthorityAction::List => {
            let listed = saved_authorities::list(paths).map_err(failed)?;
            let authorities = listed
                .iter()
                .map(|(name, saved)| Listed {
                    name,
                    workspaces: &saved.workspaces,
                })
                .collect::<Vec<_>>();
            render(
                plan.json,
                &List {
                    authorities: &authorities,
                },
                || {
                    if authorities.is_empty() {
                        return "no saved authorities; save one with `runx authority save <name> <file>`\n"
                        .to_owned();
                    }
                    authorities
                        .iter()
                        .map(|listed| {
                            format!(
                                "{}  for {} workspace(s)\n",
                                listed.name,
                                listed.workspaces.len()
                            )
                        })
                        .collect()
                },
            )
        }
        AuthorityAction::Show { name } => {
            let saved = saved_authorities::show(paths, name).map_err(failed)?;
            let shown = Shown {
                name,
                saved: &saved,
            };
            render(plan.json, &shown, || {
                serde_json::to_string_pretty(&shown)
                    .map(|text| format!("{text}\n"))
                    .unwrap_or_default()
            })
        }
        AuthorityAction::Remove { name } => {
            saved_authorities::remove(paths, name).map_err(failed)?;
            render(plan.json, &Removed { removed: name }, || {
                format!("removed authority {name}; naming it refuses until it is saved again\n")
            })
        }
    }
}

#[derive(Serialize)]
struct Shown<'a> {
    name: &'a str,
    #[serde(flatten)]
    saved: &'a SavedAuthority,
}

#[derive(Serialize)]
struct Listed<'a> {
    name: &'a str,
    workspaces: &'a [String],
}

#[derive(Serialize)]
struct List<'a> {
    authorities: &'a [Listed<'a>],
}

#[derive(Serialize)]
struct Removed<'a> {
    removed: &'a str,
}

fn render(
    json: bool,
    output: &impl Serialize,
    text: impl FnOnce() -> String,
) -> Result<String, String> {
    if !json {
        return Ok(text());
    }
    // The envelope `runx workspace` and `runx secret` answer in.
    #[derive(Serialize)]
    struct Envelope<'a, T: Serialize> {
        status: &'static str,
        #[serde(flatten)]
        output: &'a T,
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

    fn plan(args: &[&str]) -> Result<AuthorityPlan, String> {
        parse_authority_plan(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn each_command_parses() -> Result<(), String> {
        assert_eq!(
            plan(&[
                "authority",
                "save",
                "notes",
                "notes.json",
                "--workspace",
                "../other",
                "-j"
            ])?,
            AuthorityPlan {
                action: AuthorityAction::Save {
                    name: "notes".to_owned(),
                    source: DocumentInputSource::Path(PathBuf::from("notes.json")),
                    workspaces: vec![PathBuf::from("../other")],
                },
                json: true,
            }
        );
        assert_eq!(
            plan(&["authority", "save", "notes", "-"])?.action,
            AuthorityAction::Save {
                name: "notes".to_owned(),
                source: DocumentInputSource::Stdin,
                workspaces: Vec::new(),
            }
        );
        assert_eq!(plan(&["authority", "list"])?.action, AuthorityAction::List);
        assert_eq!(
            plan(&["authority", "show", "notes"])?.action,
            AuthorityAction::Show {
                name: "notes".to_owned()
            }
        );
        assert_eq!(
            plan(&["authority", "remove", "notes"])?.action,
            AuthorityAction::Remove {
                name: "notes".to_owned()
            }
        );
        for refused in [
            &["authority"][..],
            &["authority", "save", "notes"],
            &["authority", "list", "--workspace", "x"],
            &["authority", "show"],
            &["authority", "drop", "notes"],
            &["authority", "save", "notes", "a.json", "--trust"],
        ] {
            assert!(plan(refused).is_err(), "{refused:?}");
        }
        Ok(())
    }
}
