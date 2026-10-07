//! `runx connector`: the official connectors and those of the plugin the
//! workspace is in, their readiness here, and the instances that bind them to
//! an account.
//!
//! Instances live in the kernel store of the receipt root (`RUNX_RECEIPT_DIR`,
//! else the project's `.runx/receipts`). Confirming and selecting one is
//! setup, done once; a run never chooses an identity.

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;

use runx_contracts::connector::{
    ConnectorId, ConnectorManifest, Digest, Implementation, InstanceId, OperationName,
};
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{ConnectorInstance, Readiness};
use runx_runtime::kernel::{admission, setup};
use runx_runtime::{
    ConnectorScope, InstanceRecord, Instances, KernelStore, LoadedConnector, WorkspaceEnv,
};
use serde::Serialize;

use crate::cli_args::{flag_value, os_arg, split_flag};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorPlan {
    pub action: ConnectorAction,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectorAction {
    List,
    Inspect {
        connector: String,
    },
    Instances,
    Revoke {
        instance_id: String,
    },
    Use {
        connector: String,
        choice: UseChoice,
    },
    /// The tools an MCP server serves, with the digests a manifest pins.
    ImportMcp(McpImport),
}

/// The MCP server `import-mcp` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpImport {
    StreamableHttp {
        url: String,
        bearer_stdin: bool,
    },
    /// A stdio server, started with the command line after `--`.
    Stdio {
        command: String,
        args: Vec<String>,
    },
}

/// What `runx connector use` selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UseChoice {
    /// An instance confirmed earlier.
    Instance(String),
    /// The connector's local session, as signed in now.
    Session,
    /// An existing local credential profile.
    Profile { name: String, scopes: Vec<String> },
    /// An active runx Connect grant.
    Grant(String),
}

pub fn parse_connector_plan(args: &[OsString]) -> Result<ConnectorPlan, String> {
    let subcommand = args
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            "runx connector requires list, inspect, instances, use, revoke, or import-mcp"
                .to_owned()
        })?;
    let mut positionals = Vec::new();
    let mut session = false;
    let mut bearer_stdin = false;
    let mut grant = None;
    let mut profile = None;
    let mut scopes = Vec::new();
    let mut json = false;
    let mut stdio = None;
    let mut index = 2;
    while index < args.len() {
        let token = os_arg(args, index, "connector")?;
        if token == "--" {
            // Everything after `--` is a stdio server's command line.
            stdio = Some(
                (index + 1..args.len())
                    .map(|position| os_arg(args, position, "connector").map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            break;
        }
        let (flag, inline) = split_flag(token);
        match flag {
            "--json" | "-j" if inline.is_none() => json = true,
            "--session" if inline.is_none() => session = true,
            "--bearer-stdin" if inline.is_none() => bearer_stdin = true,
            "--grant" => {
                let (value, next) = flag_value(args, index, flag, inline, "connector")?;
                grant = Some(value);
                index = next;
                continue;
            }
            "--profile" => {
                let (value, next) = flag_value(args, index, flag, inline, "connector")?;
                profile = Some(value);
                index = next;
                continue;
            }
            "--scope" => {
                let (value, next) = flag_value(args, index, flag, inline, "connector")?;
                scopes.push(value);
                index = next;
                continue;
            }
            _ if token.starts_with('-') => {
                return Err(format!("unknown connector argument {token}"));
            }
            _ => positionals.push(token.to_owned()),
        }
        index += 1;
    }
    if stdio.is_some() && subcommand != "import-mcp" {
        return Err(format!("runx connector {subcommand}: unexpected arguments"));
    }
    let only_json = |action: ConnectorAction, count: usize| {
        if positionals.len() != count
            || session
            || bearer_stdin
            || grant.is_some()
            || profile.is_some()
            || !scopes.is_empty()
        {
            return Err(format!("runx connector {subcommand}: unexpected arguments"));
        }
        Ok(ConnectorPlan { action, json })
    };
    match subcommand {
        "list" => only_json(ConnectorAction::List, 0),
        "instances" => only_json(ConnectorAction::Instances, 0),
        "revoke" => match positionals.as_slice() {
            [instance_id] => only_json(ConnectorAction::Revoke { instance_id: instance_id.clone() }, 1),
            _ => Err("runx connector revoke requires <instance>".to_owned()),
        },
        "inspect" => match positionals.as_slice() {
            [connector] => only_json(
                ConnectorAction::Inspect {
                    connector: connector.clone(),
                },
                1,
            ),
            _ => Err("runx connector inspect requires <connector>".to_owned()),
        },
        "import-mcp" => match (positionals.as_slice(), session, grant, profile, stdio) {
            ([url], false, None, None, None) if scopes.is_empty() => Ok(ConnectorPlan {
                action: ConnectorAction::ImportMcp(McpImport::StreamableHttp {
                    url: url.clone(),
                    bearer_stdin,
                }),
                json,
            }),
            ([], false, None, None, Some(command_line)) if !bearer_stdin && scopes.is_empty() => {
                match command_line.split_first() {
                    Some((command, args)) => Ok(ConnectorPlan {
                        action: ConnectorAction::ImportMcp(McpImport::Stdio {
                            command: command.clone(),
                            args: args.to_vec(),
                        }),
                        json,
                    }),
                    None => Err("runx connector import-mcp -- requires a command".to_owned()),
                }
            }
            _ => Err(
                "runx connector import-mcp requires <url> [--bearer-stdin] or -- <command> [args...]"
                    .to_owned(),
            ),
        },
        "use" => {
            let choice = match (positionals.as_slice(), session, grant, profile) {
                _ if bearer_stdin => {
                    return Err("runx connector use takes no --bearer-stdin".to_owned());
                }
                ([_, instance], false, None, None) if scopes.is_empty() => UseChoice::Instance(instance.clone()),
                ([_], true, None, None) if scopes.is_empty() => UseChoice::Session,
                ([_], false, Some(grant), None) if scopes.is_empty() => UseChoice::Grant(grant),
                ([_], false, None, Some(name)) => UseChoice::Profile { name, scopes },
                _ => {
                    return Err(
                        "runx connector use requires <connector> and one of <instance>, --session, --profile <name>, or --grant <grant-id>"
                            .to_owned(),
                    );
                }
            };
            Ok(ConnectorPlan {
                action: ConnectorAction::Use {
                    connector: positionals[0].clone(),
                    choice,
                },
                json,
            })
        }
        _ => Err(format!("unknown connector subcommand {subcommand}")),
    }
}

pub fn run_native_connector(plan: ConnectorPlan, workspace: &WorkspaceEnv) -> ExitCode {
    match execute(&plan, workspace) {
        Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
        Err(error) if plan.json => crate::cli_io::write_stdout_code(
            &crate::cli_error::json_failure_output(&error, "connector_failed"),
            1,
        ),
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx connector: {error}\n")),
    }
}

fn execute(plan: &ConnectorPlan, workspace: &WorkspaceEnv) -> Result<String, String> {
    let instances = Instances::new(
        KernelStore::for_workspace(workspace.env(), workspace.cwd())
            .map_err(|error| error.to_string())?,
    );
    let records = |instances: &Instances| instances.list().map_err(|error| error.to_string());
    match &plan.action {
        ConnectorAction::List => {
            let connectors = ConnectorScope::for_workspace(workspace.cwd())
                .and_then(|scope| scope.connectors())
                .map_err(|error| error.to_string())?;
            let records = records(&instances)?;
            let selected = |id: &str| {
                records
                    .iter()
                    .find(|record| {
                        record.selected
                            && !record.revoked
                            && record.instance.connector.as_str() == id
                    })
                    .map(|record| &record.instance.instance_id)
            };
            if plan.json {
                let connectors = connectors
                    .iter()
                    .map(|connector| {
                        let manifest = &connector.manifest;
                        ConnectorSummary {
                            id: &manifest.id,
                            name: &manifest.name,
                            description: &manifest.description,
                            placements: placements(manifest),
                            operations: manifest.operations.keys().collect(),
                            selected_instance: selected(manifest.id.as_str()),
                        }
                    })
                    .collect();
                return render_json(&Output::Connectors { connectors });
            }
            let mut out = String::new();
            for connector in &connectors {
                let manifest = &connector.manifest;
                out.push_str(&format!(
                    "{}  {}  [{}]  {} operations{}\n",
                    manifest.id,
                    manifest.name,
                    placements(manifest).join(", "),
                    manifest.operations.len(),
                    selected(manifest.id.as_str())
                        .map(|instance| format!("  using {instance}"))
                        .unwrap_or_default()
                ));
            }
            Ok(out)
        }
        ConnectorAction::Inspect { connector } => {
            let connector = in_workspace(workspace, connector)?;
            let readiness =
                admission::connector_readiness(&connector, workspace.env(), workspace.cwd())?;
            let records = records(&instances)?
                .into_iter()
                .filter(|record| record.instance.connector == connector.manifest.id)
                .collect::<Vec<_>>();
            if plan.json {
                return render_json(&Output::Inspect {
                    connector: &connector.manifest,
                    manifest_digest: &connector.manifest_digest,
                    instances: records.iter().map(InstanceView::from).collect(),
                    readiness: &readiness,
                });
            }
            Ok(inspect_text(&connector, &records, &readiness))
        }
        ConnectorAction::Instances => {
            let records = records(&instances)?;
            if plan.json {
                return render_json(&Output::Instances {
                    instances: records.iter().map(InstanceView::from).collect(),
                });
            }
            if records.is_empty() {
                return Ok("no connector instances; set one up with `runx connector use <connector> --session`, `runx connector use <connector> --profile <name>`, `runx credential set` or `runx connector use <connector> --grant <grant-id>`\n".to_owned());
            }
            Ok(records.iter().map(instance_line).collect())
        }
        ConnectorAction::Revoke { instance_id } => {
            instances
                .revoke(instance_id)
                .map_err(|error| error.to_string())?;
            if plan.json {
                return render_json(&Output::Revoke { instance_id });
            }
            Ok(format!("revoked connector instance {instance_id}\n"))
        }
        ConnectorAction::Use { connector, choice } => {
            let connector = in_workspace(workspace, connector)?;
            let (env, cwd) = (workspace.env(), workspace.cwd());
            let instance = match choice {
                UseChoice::Session => setup::confirm_session(&instances, &connector, env, cwd)?,
                UseChoice::Profile { name, scopes } => setup::confirm_credential_profile(
                    &instances, &connector, name, scopes, workspace,
                )?,
                UseChoice::Grant(grant_id) => {
                    setup::confirm_grant(&instances, &connector, grant_id, env, cwd)?
                }
                UseChoice::Instance(instance_id) => select(&instances, &connector, instance_id)?,
            };
            if plan.json {
                return render_json(&Output::Use {
                    selected: &instance,
                });
            }
            Ok(format!(
                "{} uses {}\n",
                connector.manifest.id,
                instance_summary(&instance)
            ))
        }
        ConnectorAction::ImportMcp(server) => {
            let (tools, server) = match server {
                McpImport::StreamableHttp { url, bearer_stdin } => {
                    let bearer = match bearer_stdin {
                        true => Some(crate::cli_io::read_stdin_secret("bearer token")?),
                        false => None,
                    };
                    (
                        setup::served_mcp_tools(url, bearer.as_deref(), workspace.env())?,
                        url.clone(),
                    )
                }
                McpImport::Stdio { command, args } => (
                    setup::served_stdio_mcp_tools(command, args, workspace.env())?,
                    std::iter::once(command)
                        .chain(args)
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
            };
            if plan.json {
                return render_json(&Output::ImportMcp { tools: &tools });
            }
            let mut out = format!("{server} serves {} tools\n", tools.len());
            for tool in &tools {
                out.push_str(&format!("  {}  {}\n", tool.name, tool.tool_digest));
            }
            out.push_str(
                "pin one in an operation as implementation: {mcp: {tool: <name>, tool_digest: <digest>, arguments: ...}}\n",
            );
            Ok(out)
        }
    }
}

/// An official connector, or one of the plugin the workspace is in.
fn in_workspace(workspace: &WorkspaceEnv, connector: &str) -> Result<Arc<LoadedConnector>, String> {
    ConnectorScope::for_workspace(workspace.cwd())
        .and_then(|scope| scope.connector(connector))
        .map_err(|error| error.to_string())
}

/// Select an instance confirmed earlier, refusing one of another connector
/// before anything is recorded.
fn select(
    instances: &Instances,
    connector: &LoadedConnector,
    instance_id: &str,
) -> Result<ConnectorInstance, String> {
    let record = instances
        .get(instance_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("instance {instance_id} does not exist"))?;
    if record.instance.connector != connector.manifest.id {
        return Err(format!(
            "instance {instance_id} belongs to connector {}, not {}",
            record.instance.connector, connector.manifest.id
        ));
    }
    instances
        .select(instance_id)
        .map_err(|error| error.to_string())
}

fn placements(manifest: &ConnectorManifest) -> Vec<&'static str> {
    [
        manifest.placements.hosted.as_ref().map(|_| "hosted"),
        manifest.placements.local.as_ref().map(|_| "local"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn inspect_text(
    connector: &LoadedConnector,
    records: &[InstanceRecord],
    readiness: &[Readiness],
) -> String {
    let manifest = &connector.manifest;
    let mut out = format!(
        "{} ({})\n{}\n",
        manifest.name, manifest.id, manifest.description
    );
    out.push_str(&format!(
        "placements: {}\n",
        placements(manifest).join(", ")
    ));
    let auth = &manifest.auth;
    let mut authorities = Vec::new();
    if auth.delegated.is_some() {
        authorities.push("delegated grant".to_owned());
    }
    for (name, declaration) in &auth.credentials {
        authorities.push(format!("credential {name} ({})", declaration.kind));
    }
    if let Some(session) = &auth.session {
        authorities.push(format!("{} session", session.kind().as_str()));
    }
    if auth.none.is_some() {
        authorities.push("public".to_owned());
    }
    out.push_str(&format!("authority: {}\n", authorities.join(", ")));
    out.push_str("operations:\n");
    for (name, operation) in &manifest.operations {
        let kind = operation.implementation.as_ref().map(Implementation::kind);
        let implementations = [operation.hosted.as_ref().map(|_| "hosted"), kind]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        out.push_str(&format!(
            "  {name}  {}  [{}]  {}\n",
            operation.verb.as_str(),
            operation
                .scopes
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if implementations.is_empty() {
                "declared only".to_owned()
            } else {
                implementations.join(", ")
            }
        ));
    }
    if records.is_empty() {
        out.push_str("instances: none\n");
        for command in setup_commands(manifest) {
            out.push_str(&format!("  set up: {command}\n"));
        }
    } else {
        out.push_str("instances:\n");
        for record in records {
            out.push_str(&format!("  {}", instance_line(record)));
        }
    }
    for line in readiness.iter().map(crate::skill::readiness_line) {
        out.push_str(&line);
    }
    out
}

/// The commands that confirm an instance under each authority the
/// connector accepts.
fn setup_commands(manifest: &ConnectorManifest) -> Vec<String> {
    let id = &manifest.id;
    let mut commands = Vec::new();
    if manifest.auth.session.is_some() {
        commands.push(format!("runx connector use {id} --session"));
    }
    for name in manifest.auth.credentials.keys() {
        commands.push(format!(
            "runx credential set {id} --credential {name} --from-stdin"
        ));
    }
    if manifest.placements.hosted.is_some() {
        commands.push(format!(
            "runx connect start {id} --scope <scope>, then runx connector use {id} --grant <grant-id>"
        ));
    }
    commands
}

fn instance_line(record: &InstanceRecord) -> String {
    format!(
        "{}{}{}\n",
        instance_summary(&record.instance),
        if record.selected { ", selected" } else { "" },
        if record.revoked { ", revoked" } else { "" }
    )
}

fn instance_summary(instance: &ConnectorInstance) -> String {
    format!(
        "{} ({}, {} {}{})",
        instance.instance_id,
        instance.connector,
        instance.placement.as_str(),
        instance.authority.kind(),
        instance
            .account_ref
            .as_ref()
            .map(|account| format!(" as {account}"))
            .unwrap_or_default()
    )
}

#[derive(Serialize)]
#[serde(untagged)]
enum Output<'a> {
    Connectors {
        connectors: Vec<ConnectorSummary<'a>>,
    },
    Inspect {
        connector: &'a ConnectorManifest,
        manifest_digest: &'a Digest,
        instances: Vec<InstanceView<'a>>,
        readiness: &'a [Readiness],
    },
    Instances {
        instances: Vec<InstanceView<'a>>,
    },
    Revoke {
        instance_id: &'a str,
    },
    Use {
        selected: &'a ConnectorInstance,
    },
    ImportMcp {
        tools: &'a [setup::ServedTool],
    },
}

#[derive(Serialize)]
struct ConnectorSummary<'a> {
    id: &'a ConnectorId,
    name: &'a NonEmptyString,
    description: &'a NonEmptyString,
    placements: Vec<&'static str>,
    operations: Vec<&'a OperationName>,
    selected_instance: Option<&'a InstanceId>,
}

#[derive(Serialize)]
struct InstanceView<'a> {
    #[serde(flatten)]
    instance: &'a ConnectorInstance,
    selected: bool,
    revoked: bool,
}

impl<'a> From<&'a InstanceRecord> for InstanceView<'a> {
    fn from(record: &'a InstanceRecord) -> Self {
        Self {
            instance: &record.instance,
            selected: record.selected,
            revoked: record.revoked,
        }
    }
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

    fn plan(args: &[&str]) -> Result<ConnectorPlan, String> {
        parse_connector_plan(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn import_mcp_takes_one_url_and_reads_its_token_from_stdin_only() {
        assert_eq!(
            plan(&[
                "connector",
                "import-mcp",
                "https://mcp.example.com/mcp",
                "--bearer-stdin",
                "-j"
            ]),
            Ok(ConnectorPlan {
                action: ConnectorAction::ImportMcp(McpImport::StreamableHttp {
                    url: "https://mcp.example.com/mcp".to_owned(),
                    bearer_stdin: true,
                }),
                json: true,
            })
        );
        assert_eq!(
            plan(&[
                "connector",
                "import-mcp",
                "-j",
                "--",
                "npx",
                "-y",
                "server",
                "--flag"
            ]),
            Ok(ConnectorPlan {
                action: ConnectorAction::ImportMcp(McpImport::Stdio {
                    command: "npx".to_owned(),
                    args: vec!["-y".to_owned(), "server".to_owned(), "--flag".to_owned()],
                }),
                json: true,
            })
        );
        assert!(plan(&["connector", "import-mcp", "--"]).is_err());
        assert!(plan(&["connector", "import-mcp", "https://a.example", "--", "npx"]).is_err());
        assert!(plan(&["connector", "import-mcp", "--bearer-stdin", "--", "npx"]).is_err());
        assert!(plan(&["connector", "list", "--", "npx"]).is_err());
        assert!(plan(&["connector", "import-mcp"]).is_err());
        assert!(plan(&["connector", "import-mcp", "https://a.example", "--session"]).is_err());
        assert!(plan(&["connector", "use", "github", "--session", "--bearer-stdin"]).is_err());
        assert!(plan(&["connector", "list", "--bearer-stdin"]).is_err());
    }
}
