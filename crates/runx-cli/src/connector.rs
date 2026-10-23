//! `runx connector`: the connectors in reach (runtime capabilities, the
//! plugin's, the project's installed versions), their readiness here, and the
//! one place their authority is set up (`use`), listed (`list`) and removed
//! (`revoke`): an instance, a local session, a secret it names or a Connect
//! grant. The secrets themselves are `runx secret`'s.
//!
//! Instances live in the project's kernel store (`RUNX_PROJECT_DIR`, else the
//! workspace's `.runx`). Confirming and selecting one is setup, done once; a
//! run never chooses an identity.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;

use runx_contracts::connector::{
    ConnectorManifest, CredentialName, Implementation, InstanceId, OperationName, SecretName,
};
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{ConnectorInstance, InstanceAuthority, JsonObject, JsonValue, Readiness};
use runx_runtime::kernel::connector_setup::{
    AuthoritySources, ConnectorSetup, Revoked, UseOptions, UseOutcome, UseRequest, setup_forms,
};
use runx_runtime::kernel::{admission, setup};
use runx_runtime::{
    ConnectorScope, HostedProviderGrant, InstanceRecord, Instances, KernelStore, LoadedConnector,
    WorkspaceEnv,
};
use serde::Serialize;

use crate::cli_args::{flag_value, os_arg, split_flag};

/// How long `use --hosted` waits for the person to authorize before it
/// returns the command that finishes the grant.
const HOSTED_GRANT_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorPlan {
    pub action: ConnectorAction,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectorAction {
    /// Every connector in reach, with the instances and grants that hold its
    /// authority.
    List,
    Inspect {
        connector: String,
    },
    /// Remove one instance or grant.
    Revoke {
        name: String,
    },
    Use {
        connector: String,
        choice: UseChoice,
        scopes: Vec<String>,
        parameters: BTreeMap<String, String>,
    },
    /// The tools an MCP server serves, with the digests a manifest pins.
    ImportMcp(McpImport),
}

/// The MCP server `import-mcp` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpImport {
    /// A server at `url`, sent the secret `secret` names as its bearer token
    /// when it needs one.
    StreamableHttp {
        url: String,
        secret: Option<SecretName>,
    },
    /// A stdio server, started with the command line after `--`.
    Stdio { command: String, args: Vec<String> },
}

/// What `runx connector use` sets up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UseChoice {
    /// No selector: the one available path.
    Default,
    /// An instance, grant or pending Connect session, by its id.
    Existing(String),
    /// The connector's local session, as signed in now.
    Session,
    /// The named credential: its variable when that is set, else asked for
    /// and kept as a new secret.
    Credential { credential: CredentialName },
    /// The named credential, or the connector's only one, bound to a secret
    /// `runx secret set` keeps.
    Secret {
        credential: Option<CredentialName>,
        secret: SecretName,
    },
    /// A new runx Connect grant; the credential its placement names, if any,
    /// comes from the secret, its variable or is asked for.
    Hosted {
        target: Option<String>,
        secret: Option<SecretName>,
    },
}

const USE_USAGE: &str = "runx connector use <connector> [<instance|grant>] [--session | --credential <name> | [--credential <name>] --secret <name> | --hosted [--secret <name>] [--target <locator>]] [--scope <scope>...] [--parameter <name>=<value>...]";

pub fn parse_connector_plan(args: &[OsString]) -> Result<ConnectorPlan, String> {
    let subcommand = args
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            "runx connector requires list, inspect, use, revoke, or import-mcp".to_owned()
        })?;
    let mut positionals = Vec::new();
    let mut session = false;
    let mut hosted = false;
    let mut credential = None;
    let mut target = None;
    let mut secret = None;
    let mut scopes = Vec::new();
    let mut parameters = BTreeMap::new();
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
        let mut value = |target: &mut Option<String>| -> Result<(), String> {
            let (value, next) = flag_value(args, index, flag, inline, "connector")?;
            if target.replace(value).is_some() {
                return Err(format!("runx connector {subcommand} takes {flag} once"));
            }
            index = next;
            Ok(())
        };
        match flag {
            "--json" | "-j" if inline.is_none() => json = true,
            "--session" if inline.is_none() => session = true,
            "--hosted" if inline.is_none() => hosted = true,
            "--credential" => {
                value(&mut credential)?;
                continue;
            }
            "--target" => {
                value(&mut target)?;
                continue;
            }
            "--secret" => {
                value(&mut secret)?;
                continue;
            }
            "--parameter" => {
                let mut raw = None;
                value(&mut raw)?;
                let (name, value) = parameter(raw.as_deref().unwrap_or_default())?;
                if parameters.insert(name.clone(), value).is_some() {
                    return Err(format!("runx connector use sets parameter {name} once"));
                }
                continue;
            }
            "--scope" => {
                let mut scope = None;
                value(&mut scope)?;
                scopes.extend(scope);
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
    let secret = secret
        .map(|name| {
            SecretName::new(name.as_str()).ok_or_else(|| format!("{name} is not a secret name"))
        })
        .transpose()?;
    let setup_flags = session
        || hosted
        || credential.is_some()
        || target.is_some()
        || !scopes.is_empty()
        || !parameters.is_empty();
    let only_json = |action: ConnectorAction, count: usize| {
        if positionals.len() != count || setup_flags || secret.is_some() {
            return Err(format!("runx connector {subcommand}: unexpected arguments"));
        }
        Ok(ConnectorPlan { action, json })
    };
    match subcommand {
        "list" => only_json(ConnectorAction::List, 0),
        "revoke" => match positionals.as_slice() {
            [name] => only_json(ConnectorAction::Revoke { name: name.clone() }, 1),
            _ => Err("runx connector revoke requires <instance|grant>".to_owned()),
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
        "import-mcp" => match (positionals.as_slice(), setup_flags, stdio) {
            ([url], false, None) => Ok(ConnectorPlan {
                action: ConnectorAction::ImportMcp(McpImport::StreamableHttp {
                    url: url.clone(),
                    secret,
                }),
                json,
            }),
            ([], false, Some(command_line)) if secret.is_none() => match command_line.split_first() {
                Some((command, args)) => Ok(ConnectorPlan {
                    action: ConnectorAction::ImportMcp(McpImport::Stdio {
                        command: command.clone(),
                        args: args.to_vec(),
                    }),
                    json,
                }),
                None => Err("runx connector import-mcp -- requires a command".to_owned()),
            },
            _ => Err(
                "runx connector import-mcp requires <url> [--secret <name>] or -- <command> [args...]"
                    .to_owned(),
            ),
        },
        "use" => {
            if target.is_some() && !hosted {
                return Err("--target narrows a new hosted grant; it needs --hosted".to_owned());
            }
            if hosted && credential.is_some() {
                return Err("--hosted sends the one credential the connector's hosted placement names; --credential chooses a local one".to_owned());
            }
            let credential = credential
                .map(|name| {
                    CredentialName::new(name.as_str())
                        .ok_or_else(|| format!("{name} is not a credential name"))
                })
                .transpose()?;
            let choice = match (positionals.as_slice(), session, hosted) {
                ([_, existing], false, false) if credential.is_none() && secret.is_none() => UseChoice::Existing(existing.clone()),
                ([_], true, false) if credential.is_none() && secret.is_none() => UseChoice::Session,
                ([_], false, true) => UseChoice::Hosted { target, secret },
                ([_], false, false) => match (credential, secret) {
                    (credential, Some(secret)) => UseChoice::Secret { credential, secret },
                    (Some(credential), None) => UseChoice::Credential { credential },
                    (None, None) => UseChoice::Default,
                },
                _ => return Err(format!("usage: {USE_USAGE}")),
            };
            Ok(ConnectorPlan {
                action: ConnectorAction::Use {
                    connector: positionals[0].clone(),
                    choice,
                    scopes,
                    parameters,
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
        Err(error) => crate::cli_io::write_stderr_code(&format!("runx connector: {error}\n"), 1),
    }
}

fn execute(plan: &ConnectorPlan, workspace: &WorkspaceEnv) -> Result<String, String> {
    let instances = Instances::new(
        KernelStore::for_project(workspace.paths()).map_err(|error| error.to_string())?,
    );
    let records = |instances: &Instances| instances.list().map_err(|error| error.to_string());
    match &plan.action {
        ConnectorAction::List => {
            let connectors = ConnectorScope::in_project(workspace.paths().cwd(), workspace.paths())
                .and_then(|scope| scope.reachable())
                .map_err(|error| error.to_string())?;
            let sources = ConnectorSetup::new(&instances, workspace).sources()?;
            if plan.json {
                let connectors = connectors
                    .iter()
                    .map(|connector| {
                        let manifest = &connector.manifest;
                        ConnectorSummary {
                            connector_binding: connector.binding(),
                            name: &manifest.name,
                            description: &manifest.description,
                            placements: placements(manifest),
                            operations: manifest.operations.keys().collect(),
                            selected_instance: sources
                                .selected(connector)
                                .map(|record| &record.instance.instance_id),
                        }
                    })
                    .collect();
                return render_json(&Output::Connectors {
                    connectors,
                    instances: sources.instances.iter().map(InstanceView::from).collect(),
                    grants: sources.grants.as_ref().ok(),
                    grants_unavailable: sources.grants.as_ref().err(),
                });
            }
            Ok(list_text(&connectors, &sources))
        }
        ConnectorAction::Inspect { connector } => {
            let connector = in_workspace(workspace, connector)?;
            let readiness = admission::connector_readiness(
                &connector,
                workspace.env(),
                workspace.paths().cwd(),
            )?;
            let records = records(&instances)?
                .into_iter()
                .filter(|record| record.instance.connector == connector.name)
                .collect::<Vec<_>>();
            if plan.json {
                return render_json(&Output::Inspect {
                    connector: &connector.manifest,
                    connector_binding: connector.binding(),
                    instances: records.iter().map(InstanceView::from).collect(),
                    readiness: &readiness,
                });
            }
            Ok(inspect_text(&connector, &records, &readiness))
        }
        ConnectorAction::Revoke { name } => {
            let revoked = ConnectorSetup::new(&instances, workspace).revoke(name)?;
            let (kind, name) = match &revoked {
                Revoked::Instance(name) => ("instance", name),
                Revoked::Grant(name) => ("grant", name),
            };
            if plan.json {
                return render_json(&Output::Revoke {
                    revoked: kind,
                    name,
                });
            }
            Ok(format!("revoked {kind} {name}\n"))
        }
        ConnectorAction::Use {
            connector,
            choice,
            scopes,
            parameters,
        } => {
            let connector = in_workspace(workspace, connector)?;
            let request = match choice {
                UseChoice::Default => UseRequest::Default,
                UseChoice::Existing(name) => UseRequest::Existing(name),
                UseChoice::Session => UseRequest::Session,
                UseChoice::Credential { credential } => UseRequest::Credential { credential },
                UseChoice::Secret { credential, secret } => UseRequest::Secret {
                    credential: credential.as_ref(),
                    secret,
                },
                UseChoice::Hosted { target, secret } => UseRequest::Hosted {
                    target: target.as_deref(),
                    secret: secret.as_ref(),
                },
            };
            // A person is shown the link and waited for; a program asking
            // for JSON gets the link and the command that finishes the grant
            // at once, to hand on.
            let options = UseOptions {
                scopes: scopes.clone(),
                parameters: instance_parameters(parameters),
                wait: if plan.json {
                    std::time::Duration::ZERO
                } else {
                    HOSTED_GRANT_WAIT
                },
            };
            // A credential whose variable is not set is asked for, never
            // taken from argv.
            let setup = ConnectorSetup::new(&instances, workspace)
                .reading_secrets(&crate::cli_io::read_secret);
            let setup = if plan.json {
                setup
            } else {
                setup.showing_links(&crate::cli_io::show_link)
            };
            match setup.use_connector(&connector, request, &options)? {
                UseOutcome::Selected(instance) => {
                    if plan.json {
                        return render_json(&Output::Use {
                            selected: &instance,
                        });
                    }
                    Ok(format!(
                        "{} uses {}\n",
                        connector.name,
                        instance_summary(&instance)
                    ))
                }
                UseOutcome::Pending {
                    session_id,
                    authorization_url,
                    finish,
                } => {
                    if plan.json {
                        return render_pending(&session_id, authorization_url.as_deref(), &finish);
                    }
                    // The link was shown on stderr before the wait.
                    Ok(format!(
                        "{} is not authorized yet; once it is, run:\n  {finish}\n",
                        connector.name
                    ))
                }
            }
        }
        ConnectorAction::ImportMcp(server) => {
            let (tools, server) = match server {
                McpImport::StreamableHttp { url, secret } => (
                    setup::served_mcp_tools(
                        url,
                        secret.as_ref(),
                        workspace.env(),
                        workspace.cwd(),
                    )?,
                    url.clone(),
                ),
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

/// A runtime capability, one of the plugin runx was invoked in, or a version
/// runx added, by name.
fn in_workspace(workspace: &WorkspaceEnv, connector: &str) -> Result<Arc<LoadedConnector>, String> {
    ConnectorScope::in_project(workspace.paths().cwd(), workspace.paths())
        .and_then(|scope| scope.named(connector))
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
        // How to set one up is the whole answer: each operation would only
        // repeat that it has no instance.
        out.push_str("instances: none\n");
        for form in setup_forms(connector) {
            out.push_str(&format!("  set up: {form}\n"));
        }
        return out;
    }
    out.push_str("instances:\n");
    for record in records {
        out.push_str(&format!("  {}", instance_line(record)));
    }
    for line in readiness.iter().map(crate::skill::readiness_line) {
        out.push_str(&line);
    }
    out
}

/// Each connector in reach with the instance it uses, then every instance
/// and the signed-in account's grants.
fn list_text(connectors: &[Arc<LoadedConnector>], sources: &AuthoritySources) -> String {
    let mut out = String::new();
    for connector in connectors {
        let manifest = &connector.manifest;
        let selected = sources.selected(connector);
        out.push_str(&format!(
            "{}  {}  [{}]  {} operations{}\n",
            connector.name,
            manifest.name,
            placements(manifest).join(", "),
            manifest.operations.len(),
            selected
                .map(|record| format!("  using {}", record.instance.instance_id))
                .unwrap_or_default()
        ));
    }
    if !sources.instances.is_empty() {
        out.push_str("instances:\n");
        for record in &sources.instances {
            out.push_str(&format!("  {}", instance_line(record)));
        }
    }
    match &sources.grants {
        Ok(grants) if !grants.is_empty() => {
            out.push_str("grants:\n");
            for grant in grants {
                out.push_str(&format!(
                    "  {}  {}  {}\n",
                    grant.grant_id, grant.provider, grant.status
                ));
            }
        }
        Ok(_) => {}
        Err(reason) => out.push_str(&format!("grants: unavailable ({reason})\n")),
    }
    out
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
    let source = match &instance.authority {
        InstanceAuthority::Secret { secret, .. } => format!(" from secret {secret}"),
        InstanceAuthority::Environment { variable, .. } => format!(" from {variable}"),
        _ => instance
            .account_id
            .as_ref()
            .map(|account| format!(" as {account}"))
            .unwrap_or_default(),
    };
    format!(
        "{} ({}, {} {}{source})",
        instance.instance_id,
        instance.connector,
        instance.placement.as_str(),
        instance.authority.kind(),
    )
}

#[derive(Serialize)]
#[serde(untagged)]
enum Output<'a> {
    Connectors {
        connectors: Vec<ConnectorSummary<'a>>,
        /// Every instance in the project, each naming its connector.
        instances: Vec<InstanceView<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        grants: Option<&'a Vec<HostedProviderGrant>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        grants_unavailable: Option<&'a String>,
    },
    Inspect {
        connector: &'a ConnectorManifest,
        connector_binding: runx_contracts::connector::ConnectorBinding,
        instances: Vec<InstanceView<'a>>,
        readiness: &'a [Readiness],
    },
    Revoke {
        revoked: &'static str,
        name: &'a str,
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
    connector_binding: runx_contracts::connector::ConnectorBinding,
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

/// A hosted grant waiting on the person: the session and what finishes it.
fn render_pending(
    session_id: &str,
    authorization_url: Option<&str>,
    finish: &str,
) -> Result<String, String> {
    serde_json::to_string_pretty(&serde_json::json!({
        "status": "pending",
        "session_id": session_id,
        "authorization_url": authorization_url,
        "finish": finish,
    }))
    .map(|value| format!("{value}\n"))
    .map_err(|error| error.to_string())
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

/// The parameters an instance pins, from `--parameter` flags.
pub(crate) fn instance_parameters(parameters: &BTreeMap<String, String>) -> JsonObject {
    parameters
        .iter()
        .map(|(name, value)| (name.clone(), JsonValue::String(value.clone())))
        .collect()
}

/// `<name>=<value>` from a `--parameter` flag.
pub(crate) fn parameter(value: &str) -> Result<(String, String), String> {
    match value.split_once('=') {
        Some((name, value)) if !name.is_empty() && !value.is_empty() => {
            Ok((name.to_owned(), value.to_owned()))
        }
        _ => Err(format!("--parameter takes <name>=<value>, not {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(args: &[&str]) -> Result<ConnectorPlan, String> {
        parse_connector_plan(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn session_parameters_reach_native_confirmation() -> Result<(), String> {
        let parsed = plan(&[
            "connector",
            "use",
            "reddit",
            "--session",
            "--parameter",
            "account=example_builder",
        ])?;
        let ConnectorAction::Use {
            choice, parameters, ..
        } = parsed.action
        else {
            return Err("expected connector setup".to_owned());
        };
        assert_eq!(choice, UseChoice::Session);
        assert_eq!(
            parameters.get("account").map(String::as_str),
            Some("example_builder")
        );
        Ok(())
    }

    #[test]
    fn a_secret_is_bound_by_name_and_the_flags_it_replaced_are_gone() -> Result<(), String> {
        let ConnectorAction::Use { choice, .. } = plan(&[
            "connector",
            "use",
            "sentry",
            "--credential",
            "token",
            "--secret",
            "sentry-work",
        ])?
        .action
        else {
            return Err("expected connector setup".to_owned());
        };
        assert_eq!(
            choice,
            UseChoice::Secret {
                credential: CredentialName::new("token"),
                secret: SecretName::new("sentry-work").ok_or("a secret name")?,
            }
        );
        let ConnectorAction::Use { choice, .. } = plan(&[
            "connector",
            "use",
            "ausca/inbox",
            "--hosted",
            "--secret",
            "inbox",
        ])?
        .action
        else {
            return Err("expected connector setup".to_owned());
        };
        assert_eq!(
            choice,
            UseChoice::Hosted {
                target: None,
                secret: SecretName::new("inbox"),
            }
        );
        for removed in [
            &["connector", "use", "sentry", "--profile", "work"][..],
            &["connector", "use", "sentry", "--from", "op://a/b/c"][..],
            &[
                "connector",
                "import-mcp",
                "https://a.example",
                "--bearer-stdin",
            ][..],
        ] {
            let refused = plan(removed).err().ok_or("a removed flag parsed")?;
            assert!(
                refused.starts_with("unknown connector argument"),
                "{refused}"
            );
        }
        assert!(plan(&["connector", "use", "sentry", "existing", "--secret", "x"]).is_err());
        assert!(plan(&["connector", "use", "github", "--session", "--secret", "x"]).is_err());
        assert!(plan(&["connector", "list", "--secret", "x"]).is_err());
        Ok(())
    }

    #[test]
    fn import_mcp_takes_one_url_and_its_token_from_a_secret_only() {
        assert_eq!(
            plan(&[
                "connector",
                "import-mcp",
                "https://mcp.example.com/mcp",
                "--secret",
                "mcp-example",
                "-j"
            ]),
            Ok(ConnectorPlan {
                action: ConnectorAction::ImportMcp(McpImport::StreamableHttp {
                    url: "https://mcp.example.com/mcp".to_owned(),
                    secret: SecretName::new("mcp-example"),
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
        assert!(plan(&["connector", "import-mcp", "--secret", "x", "--", "npx"]).is_err());
        assert!(plan(&["connector", "list", "--", "npx"]).is_err());
        assert!(plan(&["connector", "import-mcp"]).is_err());
        assert!(plan(&["connector", "import-mcp", "https://a.example", "--session"]).is_err());
    }
}
