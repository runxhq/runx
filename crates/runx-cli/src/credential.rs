use std::process::ExitCode;

use runx_contracts::ConnectorInstance;
use runx_contracts::connector::CredentialName;
use runx_runtime::{
    Instances, KernelStore, WorkspaceEnv, list_local_credential_profiles,
    remove_local_credential_profile,
};
use serde::Serialize;

mod parser;

pub use parser::parse_credential_plan;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialPlan {
    pub action: CredentialAction,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialAction {
    Set {
        connector: String,
        credential: String,
        profile: String,
        scopes: Vec<String>,
        parameters: std::collections::BTreeMap<String, String>,
        from_stdin: bool,
    },
    List,
    Remove {
        profile: String,
    },
}

pub fn run_native_credential_with_workspace(
    plan: CredentialPlan,
    workspace: &WorkspaceEnv,
) -> ExitCode {
    match execute(&plan, workspace) {
        Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
        Err(error) => fail(&plan, &error),
    }
}

fn execute(plan: &CredentialPlan, workspace: &WorkspaceEnv) -> Result<String, String> {
    let result = match &plan.action {
        CredentialAction::Set {
            connector,
            credential,
            profile,
            scopes,
            parameters,
            from_stdin,
        } => {
            if !from_stdin {
                return Err("credential secret input must come from stdin".to_owned());
            }
            let connector = runx_runtime::ConnectorScope::at(workspace.paths().cwd())
                .and_then(|scope| {
                    scope
                        .in_project(workspace.paths().connectors())
                        .named(connector)
                })
                .map_err(|error| error.to_string())?;
            let credential = CredentialName::new(credential.as_str())
                .ok_or_else(|| format!("{credential} is not a credential name"))?;
            let secret = crate::cli_io::read_stdin_secret("credential secret")?;
            let instances = Instances::new(
                KernelStore::for_project(workspace.paths()).map_err(|error| error.to_string())?,
            );
            let instance = runx_runtime::kernel::setup::set_credential(
                &instances,
                &connector,
                profile,
                &credential,
                &secret,
                scopes,
                crate::connector::instance_parameters(parameters),
                workspace,
            )?;
            CredentialResult::Set {
                profile: profile.clone(),
                instance: Box::new(instance),
            }
        }
        CredentialAction::List => CredentialResult::List {
            profiles: list_local_credential_profiles(workspace)
                .map_err(|error| error.to_string())?,
        },
        CredentialAction::Remove { profile } => CredentialResult::Remove {
            profile: profile.clone(),
            removed: remove_local_credential_profile(workspace, profile)
                .map_err(|error| error.to_string())?,
        },
    };
    if plan.json {
        serde_json::to_string_pretty(&CredentialJsonOutput {
            status: "success",
            credential: &result,
        })
        .map(|value| format!("{value}\n"))
        .map_err(|error| error.to_string())
    } else {
        Ok(render_text(&result))
    }
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum CredentialResult {
    Set {
        profile: String,
        instance: Box<ConnectorInstance>,
    },
    List {
        profiles: Vec<runx_runtime::CredentialProfileSummary>,
    },
    Remove {
        profile: String,
        removed: bool,
    },
}

#[derive(Serialize)]
struct CredentialJsonOutput<'a> {
    status: &'static str,
    credential: &'a CredentialResult,
}

fn render_text(result: &CredentialResult) -> String {
    match result {
        CredentialResult::Set { profile, instance } => format!(
            "credential profile '{profile}' stored; {} uses instance {}\n",
            instance.connector, instance.instance_id
        ),
        CredentialResult::List { profiles } if profiles.is_empty() => {
            "no credential profiles configured\n".to_owned()
        }
        CredentialResult::List { profiles } => profiles
            .iter()
            .map(|profile| {
                format!(
                    "{}  {}  {}\n",
                    profile.name, profile.connector, profile.credential
                )
            })
            .collect(),
        CredentialResult::Remove { profile, removed } => format!(
            "credential profile '{profile}' {}\n",
            if *removed { "removed" } else { "not found" }
        ),
    }
}

fn fail(plan: &CredentialPlan, message: &str) -> ExitCode {
    if plan.json {
        return crate::cli_io::write_stdout_code(
            &crate::router::json_failure_output(message, "credential_error"),
            1,
        );
    }
    let _ignored = crate::cli_io::write_stderr(&format!("runx credential: {message}\n"));
    ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use super::{CredentialAction, CredentialPlan, parse_credential_plan};

    #[test]
    fn parses_stdin_connector_credential() {
        let args = [
            "credential",
            "set",
            "nitrosend",
            "--credential",
            "api_key",
            "--profile",
            "account-one",
            "--scope",
            "content.review",
            "--from-stdin",
            "--json",
        ]
        .map(Into::into);
        assert_eq!(
            parse_credential_plan(&args),
            Ok(CredentialPlan {
                action: CredentialAction::Set {
                    connector: "nitrosend".to_owned(),
                    credential: "api_key".to_owned(),
                    profile: "account-one".to_owned(),
                    scopes: vec!["content.review".to_owned()],
                    parameters: std::collections::BTreeMap::new(),
                    from_stdin: true,
                },
                json: true,
            })
        );
    }

    #[test]
    fn set_requires_the_connector_credential_name() {
        let args = ["credential", "set", "nitrosend", "--from-stdin"].map(Into::into);
        assert_eq!(
            parse_credential_plan(&args),
            Err("runx credential set requires --credential <name>".to_owned())
        );
    }
}
