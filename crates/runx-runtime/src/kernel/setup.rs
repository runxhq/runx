//! Confirming instances: the setup half of [`super::instances`].
//!
//! Setup reads an authority source once (a signed-in tool's token, a stored
//! credential profile, a runx Connect grant), pins what it found in an
//! instance and selects that instance as its connector's default. Admission
//! reads the same source again on every call and refuses when it no longer
//! matches the pin.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use runx_contracts::connector::credential_declaration_digest;
use runx_contracts::connector::{CredentialName, LocalAuth, Placement, SessionKind};
use runx_contracts::{ConnectorInstance, InstanceAuthority, JsonObject, sha256_prefixed};

use super::connectors::LoadedConnector;
use super::instances::{Instances, check_supported, pinned};
use super::operation::{credentials, hosted::HostedClient};
use crate::config::{load_local_credential_secret, load_runx_config_file};
use crate::credential_profiles::set_local_credential_profile;
use crate::services::WorkspaceEnv;

/// Confirm the connector's local session as signed in now, and select it.
/// `scopes` narrows what the session may serve; empty grants every scope its
/// credential serves.
pub fn confirm_session(
    instances: &Instances,
    connector: &LoadedConnector,
    scopes: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConnectorInstance, String> {
    let transport = crate::http::ReqwestHttpTransport::for_provider_operation(
        crate::hosted_private_network_allowed(false, env),
    )
    .map_err(|error| error.to_string())?;
    confirm_session_over(instances, connector, scopes, &transport, env, cwd)
}

/// [`confirm_session`], reading a command session's account through
/// `transport`.
pub(crate) fn confirm_session_over(
    instances: &Instances,
    connector: &LoadedConnector,
    scopes: &[String],
    transport: &dyn crate::http::RuntimeHttpTransport,
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
            let session = super::session::CommandSession::of(manifest)
                .ok_or_else(|| format!("connector {} declares no command session", manifest.id))?;
            let scopes = served_scopes(connector, session.credential, scopes)?;
            let token = session.token(env, cwd)?;
            let principal = session.principal(&token, transport)?;
            pinned(
                connector,
                super::instances::instance_id(&[
                    "session",
                    connector.connector_ref.as_str(),
                    &principal.login,
                ])?,
                Placement::Local,
                principal.principal_ref(manifest.id.as_str()),
                Some(principal.login),
                InstanceAuthority::Session {
                    session: SessionKind::Command,
                    scopes: scopes.into_iter().map(Into::into).collect(),
                },
            )
        }
        SessionKind::HostBrowser => {
            return Err(format!(
                "connector {}'s host browser session names no account the runtime can pin",
                manifest.id
            ));
        }
    };
    confirm(instances, connector, instance)
}

/// Store `secret` in a local profile as the connector credential it is,
/// then confirm and select the instance that holds it. `scopes` narrows the
/// grant; empty means every scope the credential can serve.
#[allow(clippy::too_many_arguments)]
pub fn set_credential(
    instances: &Instances,
    connector: &LoadedConnector,
    profile: &str,
    credential: &CredentialName,
    secret: &str,
    scopes: &[String],
    parameters: JsonObject,
    workspace: &WorkspaceEnv,
) -> Result<ConnectorInstance, String> {
    let parameters = instance_parameters(connector, parameters)?;
    let declaration = credential_declaration(connector, credential)?;
    credentials::fields(declaration, secret).map_err(|reason| format!("the secret {reason}"))?;
    let scopes = credential_scopes(connector, credential, scopes)?;
    let material_ref = set_local_credential_profile(
        workspace,
        profile,
        &connector.connector_ref,
        &credential_declaration_digest(declaration).map_err(|error| error.to_string())?,
        credential.as_str(),
        secret,
    )
    .map_err(|error| error.to_string())?;
    confirm_credential_instance(
        instances,
        connector,
        profile,
        credential,
        scopes,
        material_ref,
        parameters,
    )
}

/// Bind an already stored local credential profile to this connector. The
/// material is checked before an instance is selected, and its reference is
/// pinned so replacing the profile invalidates later calls.
pub fn confirm_credential_profile(
    instances: &Instances,
    connector: &LoadedConnector,
    profile: &str,
    scopes: &[String],
    parameters: JsonObject,
    workspace: &WorkspaceEnv,
) -> Result<ConnectorInstance, String> {
    let parameters = instance_parameters(connector, parameters)?;
    let config = load_runx_config_file(&workspace.paths().config_file())
        .map_err(|error| error.to_string())?;
    let stored = config
        .credentials
        .and_then(|credentials| credentials.profiles.get(profile).cloned())
        .ok_or_else(|| format!("credential profile {profile} is not configured"))?;
    if stored.connector != connector.connector_ref {
        return Err(format!(
            "credential profile {profile} belongs to {}, not {}",
            stored.connector, connector.connector_ref
        ));
    }
    let credential = CredentialName::new(stored.credential.clone())
        .ok_or_else(|| format!("credential profile {profile} has an invalid credential name"))?;
    let declaration = credential_declaration(connector, &credential)?;
    if stored.credential_declaration_digest
        != credential_declaration_digest(declaration).map_err(|error| error.to_string())?
    {
        return Err(format!(
            "credential profile {profile} was stored for a different credential declaration; set it again"
        ));
    }
    let scopes = credential_scopes(connector, &credential, scopes)?;
    let secret = load_local_credential_secret(workspace.paths(), &stored.secret_ref)
        .map_err(|error| format!("credential profile {profile} cannot be read: {error}"))?;
    credentials::fields(declaration, &secret)
        .map_err(|reason| format!("credential profile {profile} {reason}"))?;
    confirm_credential_instance(
        instances,
        connector,
        profile,
        &credential,
        scopes,
        stored.secret_ref,
        parameters,
    )
}

/// Confirm an instance that holds no authority, with the parameters it
/// pins, and select it: a store at a location that needs no password.
pub fn confirm_public(
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

/// What the operator grants an instance holding `credential`: the requested
/// scopes, each served by an operation that takes the credential, or every
/// such scope when none is requested.
fn served_scopes(
    connector: &LoadedConnector,
    credential: &CredentialName,
    scopes: &[String],
) -> Result<BTreeSet<String>, String> {
    let manifest = &connector.manifest;
    let servable = manifest
        .operations
        .values()
        .filter(|operation| operation.credentials.contains(credential))
        .flat_map(|operation| {
            operation
                .scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
        })
        .collect::<BTreeSet<_>>();
    if let Some(scope) = scopes
        .iter()
        .find(|scope| !servable.contains(scope.as_str()))
    {
        return Err(format!(
            "credential {credential} of connector {} serves no operation with scope {scope}",
            manifest.id
        ));
    }
    Ok(if scopes.is_empty() {
        servable
    } else {
        scopes.iter().cloned().collect()
    })
}

fn confirm_credential_instance(
    instances: &Instances,
    connector: &LoadedConnector,
    profile: &str,
    credential: &CredentialName,
    scopes: BTreeSet<String>,
    material_ref: String,
    parameters: JsonObject,
) -> Result<ConnectorInstance, String> {
    let manifest = &connector.manifest;
    let mut instance = pinned(
        connector,
        super::instances::instance_id(&["credential", connector.connector_ref.as_str(), profile])?,
        Placement::Local,
        format!("runx:principal:local-credential:{}:{profile}", manifest.id),
        None,
        InstanceAuthority::Credential {
            profile: profile.to_owned().into(),
            credential: credential.clone(),
            scopes: scopes.into_iter().map(Into::into).collect(),
            material_ref: material_ref.into(),
        },
    );
    instance.parameters = parameters;
    confirm(instances, connector, instance)
}

/// Confirm an active runx Connect grant for the connector, and select it.
pub fn confirm_grant(
    instances: &Instances,
    connector: &LoadedConnector,
    grant_id: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<ConnectorInstance, String> {
    confirm_grant_from(
        instances,
        connector,
        &HostedClient::default(),
        grant_id,
        env,
        cwd,
    )
}

/// [`confirm_grant`], reading the grant through `transport`: the wire an
/// embedder gives [`super::operation::OperationEffect::with_http_transport`].
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

/// [`confirm_grant`], reading the grant through `hosted`: a setup's own
/// client, so a grant it just opened is read over the same wire.
pub(crate) fn confirm_grant_from(
    instances: &Instances,
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
    let instance = pinned(
        connector,
        super::instances::instance_id(&["hosted", connector.connector_ref.as_str(), grant_id])?,
        Placement::Hosted,
        grant.principal_ref,
        None,
        InstanceAuthority::HostedGrant {
            grant_id: grant.grant_id.into(),
        },
    );
    confirm(instances, connector, instance)
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
/// transport, for an author to pin in a connector manifest.
#[cfg(feature = "mcp")]
pub fn served_mcp_tools(
    url: &str,
    bearer: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<Vec<ServedTool>, String> {
    let transport = crate::http::ReqwestHttpTransport::for_provider_operation(
        crate::hosted_private_network_allowed(false, env),
    )
    .map_err(|error| error.to_string())?;
    Ok(crate::drivers::mcp::served_tools(&transport, url, bearer)?
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
