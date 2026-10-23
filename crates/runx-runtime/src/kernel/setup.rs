//! Confirming instances: the setup half of [`super::instances`].
//!
//! Setup reads an authority source once (a signed-in tool's token, a secret
//! the operator keeps in runx, a credential's environment variable, a runx
//! Connect grant), pins what it found in an
//! instance and selects that instance as its connector's default. Admission
//! reads the same source again on every call and refuses when it no longer
//! matches the pin.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use runx_contracts::connector::{CredentialName, LocalAuth, Placement, SecretName, SessionKind};
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{ConnectorInstance, InstanceAuthority, JsonObject, sha256_prefixed};

use super::connectors::LoadedConnector;
use super::instances::{Instances, check_supported, granted_scopes, pinned, served_scopes};
use super::operation::{ToolRunner, credentials, hosted::HostedClient};
use crate::services::WorkspaceEnv;

/// Confirm the connector's local session as signed in now, and select it.
/// `scopes` narrows what the session may serve; empty grants every scope its
/// credential serves.
pub(crate) fn confirm_session(
    instances: &Instances,
    connector: &LoadedConnector,
    scopes: &[String],
    parameters: JsonObject,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConnectorInstance, String> {
    let transport = crate::http::ReqwestHttpTransport::for_provider_operation(
        crate::hosted_private_network_allowed(false, env),
    )
    .map_err(|error| error.to_string())?;
    let readers = AccountReaders {
        transport: &transport,
        tools: &SetupTools { env },
    };
    confirm_session_over(instances, connector, scopes, parameters, &readers, env, cwd)
}

/// What a session's account is read through: a command session's principal
/// request over `transport`, a browser session's installed tool through
/// `tools`.
pub(crate) struct AccountReaders<'a> {
    pub transport: &'a dyn crate::http::RuntimeHttpTransport,
    pub tools: &'a dyn ToolRunner,
}

/// Setup's runner for a connector's installed tool. Setup is outside any
/// run, so the tool runs with no run's artifacts or bounds.
pub(crate) struct SetupTools<'a> {
    pub env: &'a BTreeMap<String, String>,
}

impl ToolRunner for SetupTools<'_> {
    fn run(
        &self,
        tool: &str,
        manifest: &Path,
        inputs: JsonObject,
        scopes: &[String],
        delivery: &crate::credentials::CredentialDelivery,
    ) -> Result<runx_contracts::JsonValue, String> {
        let directory = manifest
            .parent()
            .ok_or_else(|| format!("tool {tool} has no directory"))?;
        let output = crate::tool_catalogs::dispatch::dispatch_connector_tool(
            manifest,
            &crate::tool_catalogs::dispatch::ToolDispatchRequest {
                tool: Cow::Borrowed(tool),
                inputs: Cow::Owned(inputs),
                resolved_inputs: Cow::Owned(JsonObject::new()),
                scopes,
                env: self.env,
                skill_directory: directory,
                credential_delivery: delivery,
                local_artifacts: &crate::services::LocalArtifactService::default(),
                skill_name: tool,
                allow_explicit_manifest_path: false,
                effect_admission: None,
                bounds: crate::services::WorkspaceBounds::of(None),
                call: None,
            },
            Instant::now(),
        )
        .map_err(|error| error.to_string())?;
        match output.succeeded() {
            true => Ok(output.value),
            false => Err(output
                .failure_message()
                .unwrap_or_else(|| format!("tool {tool} failed"))),
        }
    }
}

/// [`confirm_session`], reading the session's account through `readers`.
pub(crate) fn confirm_session_over(
    instances: &Instances,
    connector: &LoadedConnector,
    scopes: &[String],
    parameters: JsonObject,
    readers: &AccountReaders<'_>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConnectorInstance, String> {
    let manifest = &connector.manifest;
    let session = manifest
        .auth
        .session
        .as_ref()
        .ok_or_else(|| format!("connector {} declares no local session", manifest.id))?;
    let instance = match session.kind() {
        SessionKind::Command => {
            if !parameters.is_empty() {
                return Err("a command session declares no instance parameters".to_owned());
            }
            let session = super::session::CommandSession::of(manifest)
                .ok_or_else(|| format!("connector {} declares no command session", manifest.id))?;
            let scopes = served_scopes(connector, session.credential, scopes)?;
            let token = session.token(env, cwd)?;
            let principal = session.principal(&token, readers.transport)?;
            pinned(
                connector,
                super::instances::instance_id(&[
                    "session",
                    connector.name.as_str(),
                    &principal.login,
                ])?,
                Placement::Local,
                principal.principal_uri(manifest.id.as_str()),
                Some(principal.login),
                InstanceAuthority::Session {
                    session: SessionKind::Command,
                    scopes: scopes.into_iter().map(Into::into).collect(),
                },
            )
        }
        SessionKind::ManagedCommand => {
            let session = super::session::ManagedCommandSession::of(manifest)
                .ok_or("the connector declares no managed-command session")?;
            let granted = tool_scopes(connector, scopes, "this CLI session")?;
            let principal = session.principal(env, cwd)?;
            pinned(
                connector,
                super::instances::instance_id(&[
                    "session",
                    connector.name.as_str(),
                    &principal.id,
                ])?,
                Placement::Local,
                principal.principal_uri(manifest.id.as_str()),
                Some(principal.login),
                InstanceAuthority::Session {
                    session: SessionKind::ManagedCommand,
                    scopes: granted.into_iter().map(Into::into).collect(),
                },
            )
        }
        SessionKind::HostBrowser => {
            let parameters = instance_parameters(connector, parameters)?;
            let granted = tool_scopes(connector, scopes, "the browser session")?;
            let principal = super::session::browser::principal(
                connector,
                &parameters,
                env,
                cwd,
                readers.tools,
            )?;
            let mut instance = pinned(
                connector,
                super::instances::instance_id(&[
                    "session",
                    connector.name.as_str(),
                    &principal.id,
                ])?,
                Placement::Local,
                principal.principal_uri(manifest.id.as_str()),
                Some(principal.login),
                InstanceAuthority::Session {
                    session: SessionKind::HostBrowser,
                    scopes: granted.into_iter().map(Into::into).collect(),
                },
            );
            instance.parameters = parameters;
            instance
        }
    };
    confirm(instances, connector, instance)
}

/// Bind the connector's `credential` to the secret named `secret`, and
/// select that instance. The secret is read now to check it against the
/// declaration; the instance records its name and the proof this runx home
/// makes for the binding, never the value, and every call reads it again.
/// `scopes` narrows the grant; empty means every scope the credential can
/// serve.
pub(crate) fn bind_secret(
    instances: &Instances,
    connector: &LoadedConnector,
    credential: &CredentialName,
    secret: &SecretName,
    scopes: &[String],
    parameters: JsonObject,
    workspace: &WorkspaceEnv,
) -> Result<ConnectorInstance, String> {
    let parameters = instance_parameters(connector, parameters)?;
    let declaration = credential_declaration(connector, credential)?;
    let value = crate::secrets::read(
        workspace.env(),
        workspace.cwd(),
        secret,
        crate::secrets::Purpose::Bind,
    )
    .map_err(|error| error.to_string())?;
    credentials::secret_fields(declaration, secret, value.expose())?;
    let scopes = credential_scopes(connector, credential, scopes)?;
    let mut instance = pinned(
        connector,
        super::instances::instance_id(&[
            "secret",
            connector.name.as_str(),
            credential.as_str(),
            secret.as_str(),
        ])?,
        Placement::Local,
        format!(
            "runx:principal:local-secret:{}:{secret}",
            connector.manifest.id
        ),
        None,
        InstanceAuthority::Secret {
            credential: credential.clone(),
            secret: secret.clone(),
            scopes: scopes.into_iter().map(Into::into).collect(),
            proof: NonEmptyString::new(credentials::UNPROVEN).ok_or("an empty proof")?,
        },
    );
    instance.parameters = parameters;
    // The proof covers the binding as it is confirmed, parameters included.
    let proof = credentials::binding_proof(workspace.env(), workspace.cwd(), &instance)?;
    if let InstanceAuthority::Secret { proof: held, .. } = &mut instance.authority {
        *held = NonEmptyString::new(proof).ok_or("an empty proof")?;
    }
    confirm(instances, connector, instance)
}

/// Bind the connector's `credential` to the variable its declaration
/// delivers it in, set now in the workspace's environment, and select that
/// instance. The value is checked against the declaration and never stored:
/// every call reads the variable again. `scopes` narrows the grant; empty
/// means every scope the credential can serve.
pub(crate) fn bind_environment(
    instances: &Instances,
    connector: &LoadedConnector,
    credential: &CredentialName,
    scopes: &[String],
    parameters: JsonObject,
    workspace: &WorkspaceEnv,
) -> Result<ConnectorInstance, String> {
    let parameters = instance_parameters(connector, parameters)?;
    let variable = credential_declaration(connector, credential)?
        .env()
        .cloned()
        .ok_or_else(|| {
            format!(
                "connector {}'s {credential} credential is held by runx for a purchase; nobody binds it",
                connector.manifest.id
            )
        })?;
    credentials::from_environment(workspace.env(), connector, credential, &variable)?;
    let scopes = credential_scopes(connector, credential, scopes)?;
    let mut instance =
        super::instances::environment_instance(connector, credential, variable, scopes)?;
    instance.parameters = parameters;
    confirm(instances, connector, instance)
}

/// Confirm an instance that holds no authority, with the parameters it
/// pins, and select it: a store at a location that needs no password.
pub(crate) fn confirm_public(
    instances: &Instances,
    connector: &LoadedConnector,
    parameters: JsonObject,
) -> Result<ConnectorInstance, String> {
    let manifest = &connector.manifest;
    if !manifest
        .placements
        .local
        .as_ref()
        .is_some_and(|local| local.auth.contains(&LocalAuth::None))
    {
        return Err(format!(
            "connector {} does not act without authority locally",
            manifest.id
        ));
    }
    let parameters = instance_parameters(connector, parameters)?;
    let digest =
        sha256_prefixed(&serde_json::to_vec(&parameters).map_err(|error| error.to_string())?);
    let mut instance = pinned(
        connector,
        super::instances::instance_id(&[
            "public",
            manifest.id.as_str(),
            &digest.trim_start_matches("sha256:")[..12],
        ])?,
        Placement::Local,
        "runx:public".to_owned(),
        None,
        InstanceAuthority::Public,
    );
    instance.parameters = parameters;
    confirm(instances, connector, instance)
}

/// The parameters an instance pins: none for a connector that declares
/// none, else exactly what its schema admits.
fn instance_parameters(
    connector: &LoadedConnector,
    parameters: JsonObject,
) -> Result<JsonObject, String> {
    let manifest = &connector.manifest;
    let Some(schema) = &manifest.parameters else {
        return match parameters.keys().next() {
            None => Ok(parameters),
            Some(name) => Err(format!(
                "connector {} takes no instance parameter {name}",
                manifest.id
            )),
        };
    };
    let schema = serde_json::to_value(schema).map_err(|error| error.to_string())?;
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| {
            format!(
                "connector {}'s parameter schema is invalid: {error}",
                manifest.id
            )
        })?;
    let instance = serde_json::to_value(&parameters).map_err(|error| error.to_string())?;
    match validator.iter_errors(&instance).next() {
        None => Ok(parameters),
        Some(error) => Err(format!(
            "the parameters at {:?} do not satisfy connector {}'s schema: {error}",
            error.instance_path().as_str(),
            manifest.id
        )),
    }
}

fn credential_declaration<'a>(
    connector: &'a LoadedConnector,
    credential: &CredentialName,
) -> Result<&'a runx_contracts::connector::CredentialDeclaration, String> {
    connector
        .manifest
        .auth
        .credentials
        .get(credential)
        .ok_or_else(|| {
            format!(
                "connector {} declares no credential {credential}",
                connector.manifest.id
            )
        })
}

fn credential_scopes(
    connector: &LoadedConnector,
    credential: &CredentialName,
    scopes: &[String],
) -> Result<BTreeSet<String>, String> {
    let manifest = &connector.manifest;
    if !manifest
        .placements
        .local
        .as_ref()
        .is_some_and(|local| local.auth.contains(&LocalAuth::Credentials))
    {
        return Err(format!(
            "connector {} takes no stored credential locally",
            manifest.id
        ));
    }
    served_scopes(connector, credential, scopes)
}

/// What a session that exports no token serves: the operations its
/// connector's installed tools run.
fn tool_scopes(
    connector: &LoadedConnector,
    scopes: &[String],
    session: &str,
) -> Result<BTreeSet<String>, String> {
    granted_scopes(
        connector,
        |operation| {
            matches!(
                operation.implementation,
                Some(runx_contracts::connector::Implementation::Process(_))
            )
        },
        scopes,
        session,
    )
}

/// Confirm an active runx Connect grant for the connector, and select it,
/// reading the grant through `transport`: the wire an embedder gives
/// [`super::operation::OperationEffect::with_http_transport`].
pub fn confirm_grant_over(
    instances: &Instances,
    connector: &LoadedConnector,
    transport: std::sync::Arc<dyn crate::http::RuntimeHttpTransport + Send + Sync>,
    grant_id: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConnectorInstance, String> {
    confirm_grant_from(
        instances,
        connector,
        &HostedClient::with_transport(transport),
        grant_id,
        env,
        cwd,
    )
}

/// [`confirm_grant_over`], reading the grant through `hosted`: a setup's own
/// client, so a grant it just opened is read over the same wire.
pub(crate) fn confirm_grant_from(
    instances: &Instances,
    connector: &LoadedConnector,
    hosted: &HostedClient,
    grant_id: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConnectorInstance, String> {
    let instance = hosted_grant_instance(connector, hosted, grant_id, env, cwd)?;
    confirm(instances, connector, instance)
}

/// The instance a hosted grant stands for, read through `hosted` and pinned
/// to `connector` as loaded: the grant must bind exactly that revision and be
/// active. Setup confirms it; a run's host states it.
pub(crate) fn hosted_grant_instance(
    connector: &LoadedConnector,
    hosted: &HostedClient,
    grant_id: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConnectorInstance, String> {
    let grant = hosted.grant(env, cwd, grant_id)?;
    grant.check_binding(connector)?;
    if grant.status != "active" {
        return Err(format!("grant {grant_id} is {}", grant.status));
    }
    Ok(pinned(
        connector,
        super::instances::instance_id(&["hosted", connector.name.as_str(), grant_id])?,
        Placement::Hosted,
        grant.principal_uri,
        None,
        InstanceAuthority::HostedGrant {
            grant_id: grant.grant_id.into(),
        },
    ))
}

fn confirm(
    instances: &Instances,
    connector: &LoadedConnector,
    instance: ConnectorInstance,
) -> Result<ConnectorInstance, String> {
    check_supported(connector, &instance).map_err(|error| error.to_string())?;
    instances
        .confirm(instance.clone(), true)
        .map_err(|error| error.to_string())?;
    Ok(instance)
}

/// A tool an MCP server serves, with the digest a connector pins it by.
#[cfg(feature = "mcp")]
#[derive(Debug, serde::Serialize)]
pub struct ServedTool {
    pub name: String,
    pub tool_digest: runx_contracts::connector::Digest,
    pub definition: runx_contracts::JsonObject,
}

/// The tools the MCP server at `url` serves, read through the guarded
/// transport, for an author to pin in a connector manifest; with the secret
/// `bearer` names presented as its bearer token, read once now.
#[cfg(feature = "mcp")]
pub fn served_mcp_tools(
    url: &str,
    bearer: Option<&SecretName>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<ServedTool>, String> {
    let bearer = bearer
        .map(|name| crate::secrets::read(env, cwd, name, crate::secrets::Purpose::Bind))
        .transpose()
        .map_err(|error| error.to_string())?;
    let transport = crate::http::ReqwestHttpTransport::for_provider_operation(
        crate::hosted_private_network_allowed(false, env),
    )
    .map_err(|error| error.to_string())?;
    Ok(crate::drivers::mcp::served_tools(
        &transport,
        url,
        bearer
            .as_ref()
            .map(crate::credentials::SecretString::expose),
    )?
    .into_iter()
    .map(|(name, tool_digest, definition)| ServedTool {
        name,
        tool_digest,
        definition,
    })
    .collect())
}

/// The tools the stdio MCP server `command args` serves, started in the
/// workspace with the base child environment, for an author to pin.
#[cfg(feature = "mcp")]
pub fn served_stdio_mcp_tools(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
) -> Result<Vec<ServedTool>, String> {
    let server = crate::drivers::mcp::StdioServer {
        command: command.to_owned(),
        args: args.to_vec(),
        cwd: crate::process_invocation::process_workspace(env)
            .map_err(|error| error.to_string())?,
        env: crate::process_invocation::process_base_environment(env)
            .map_err(|error| error.to_string())?,
    };
    Ok(crate::drivers::mcp::served_stdio_tools(
        &server,
        crate::http::PROVIDER_OPERATION_REQUEST_TIMEOUT,
    )?
    .into_iter()
    .map(|(name, tool_digest, definition)| ServedTool {
        name,
        tool_digest,
        definition,
    })
    .collect())
}
