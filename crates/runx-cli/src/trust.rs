//! `runx trust <reference>[#method]`: the person states their trust in a
//! package ahead of any turn that loads it. It binds the package as a load
//! does and records each closure in their home as their approval of a load's
//! trust would, so a later load of it binds at once. Running it is the
//! statement, so it asks nothing; it is refused inside a run, and no MCP tool
//! offers it, since trust is the person's to give.

use std::ffi::OsString;
use std::process::ExitCode;

use runx_runtime::{StatedTrust, WorkspaceEnv};
use serde::Serialize;

use crate::cli_args::{os_arg, split_flag};

const USAGE: &str = "runx trust <reference>[#method] [-j|--json]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustPlan {
    pub reference: String,
    pub method: Option<String>,
    pub json: bool,
}

pub fn parse_trust_plan(args: &[OsString]) -> Result<TrustPlan, String> {
    let mut positionals = Vec::new();
    let mut json = false;
    for index in 1..args.len() {
        let token = os_arg(args, index, "trust")?;
        let (flag, inline) = split_flag(token);
        match flag {
            "--json" | "-j" if inline.is_none() => json = true,
            _ if token.starts_with('-') => {
                return Err(format!("unknown trust argument {token}"));
            }
            _ => positionals.push(token.to_owned()),
        }
    }
    let [named] = positionals.as_slice() else {
        return Err(format!("usage: {USAGE}"));
    };
    let (reference, method) = match named.split_once('#') {
        Some((reference, method)) if !reference.is_empty() && !method.is_empty() => {
            (reference.to_owned(), Some(method.to_owned()))
        }
        Some(_) => return Err(format!("usage: {USAGE}")),
        None => (named.clone(), None),
    };
    Ok(TrustPlan {
        reference,
        method,
        json,
    })
}

pub fn run_native_trust(plan: TrustPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let stated = runx_runtime::state_trust(
        &plan.reference,
        plan.method.as_deref(),
        workspace.env(),
        workspace.cwd(),
    );
    match stated {
        Ok(stated) if plan.json => match render_json(&stated) {
            Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
            Err(error) => crate::cli_io::write_stderr_code(&format!("runx trust: {error}\n"), 1),
        },
        Ok(stated) => crate::cli_io::write_stdout_code(&render_human(&stated), 0),
        Err(error) if plan.json => crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(&error, "trust_refused"),
            1,
        ),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx trust: {error}\n"), 1),
    }
}

/// What it recorded: each closure, its method, the connectors it binds, and
/// the package's tier.
fn render_human(stated: &[StatedTrust]) -> String {
    stated
        .iter()
        .map(|trusted| {
            let tier = trusted
                .tier
                .map_or("the project's own", runx_contracts::TrustTier::as_str);
            let connectors = if trusted.connectors.is_empty() {
                "no connector".to_owned()
            } else {
                trusted.connectors.join(", ")
            };
            format!(
                "trusted {}#{}, {tier}: closure {}, reaching {connectors}\n",
                trusted.package, trusted.method, trusted.closure_digest
            )
        })
        .collect()
}

fn render_json(stated: &[StatedTrust]) -> Result<String, String> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        status: &'static str,
        trusted: &'a [StatedTrust],
    }
    serde_json::to_string_pretty(&Envelope {
        status: "success",
        trusted: stated,
    })
    .map(|value| format!("{value}\n"))
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(args: &[&str]) -> Result<TrustPlan, String> {
        parse_trust_plan(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn a_reference_and_its_method_parse_and_anything_else_is_refused() -> Result<(), String> {
        assert_eq!(
            plan(&["trust", "acme/echo@1.0.0"])?,
            TrustPlan {
                reference: "acme/echo@1.0.0".to_owned(),
                method: None,
                json: false,
            }
        );
        assert_eq!(
            plan(&["trust", "acme/echo@1.0.0#say", "--json"])?,
            TrustPlan {
                reference: "acme/echo@1.0.0".to_owned(),
                method: Some("say".to_owned()),
                json: true,
            }
        );
        for refused in [
            &["trust"][..],
            &["trust", "a", "b"],
            &["trust", "acme/echo@1.0.0#"],
            &["trust", "#say"],
            &["trust", "acme/echo@1.0.0", "--yes"],
        ] {
            assert!(plan(refused).is_err(), "{refused:?}");
        }
        Ok(())
    }
}
