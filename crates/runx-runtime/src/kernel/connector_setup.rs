//! One place a connector's authority is set up, listed and removed:
//! `runx connector use`, `runx connector list` and `runx connector revoke`.
//!
//! Four authority sources exist: an instance confirmed earlier, a local
//! command session (a signed-in tool such as `gh`), a local secret profile,
//! and a runx Connect grant. Setup composes the confirmations in
//! [`super::setup`]; it never switches accounts silently, and a request it
//! cannot resolve to one source is refused with the exact forms that would.

use std::sync::Arc;
use std::time::{Duration, Instant};

use runx_contracts::connector::{CredentialName, LocalAuth};
use runx_contracts::{ConnectorInstance, JsonObject};
use serde_json::Value;

use super::connectors::LoadedConnector;
use super::instances::Instances;
use super::operation::hosted::HostedClient;
use super::setup;
use crate::credential_profiles::{
    CredentialProfileSummary, list_local_credential_profiles, remove_local_credential_profile,
};
use crate::hosted_api::{
    HostedApiEnvironment, HostedConnectAction, HostedConnectStart, execute_hosted_connect,
};
use crate::http::RuntimeHttpTransport;
use crate::services::WorkspaceEnv;

type Transport = Arc<dyn RuntimeHttpTransport + Send + Sync>;

/// How often a pending Connect session is asked again when it does not say.
const DEFAULT_POLL: Duration = Duration::from_secs(2);
const MIN_POLL: Duration = Duration::from_millis(250);

/// What `runx connector use <ref>` names.
#[derive(Clone, Copy, Debug)]
pub enum UseRequest<'a> {
    /// No selector: the one available path.
    Default,
    /// An instance confirmed earlier, a saved profile, an active grant, or a
    /// pending Connect session, by its id or name.
    Existing(&'a str),
    /// The connector's local command session, as signed in now.
    Session,
    /// A new local secret profile holding `secret` as `credential`.
    Credential {
        credential: &'a CredentialName,
        secret: &'a str,
        profile: &'a str,
    },
    /// A new runx Connect grant: delegated, or holding `credential` when the
    /// connector's hosted placement takes the user's own.
    Hosted {
        credential: Option<(&'a CredentialName, &'a str)>,
        target: Option<&'a str>,
    },
}

/// What narrows the instance `use` confirms.
#[derive(Clone, Debug, Default)]
pub struct UseOptions {
    /// The scopes it serves; empty serves every scope its authority can.
    pub scopes: Vec<String>,
    /// The parameters the instance pins.
    pub parameters: JsonObject,
    /// How long a new hosted grant is waited for before `use` returns what
    /// finishes it.
    pub wait: Duration,
}

/// The outcome of `runx connector use`.
#[derive(Clone, Debug)]
pub enum UseOutcome {
    /// The confirmed instance, now the connector's selected one.
    Selected(Box<ConnectorInstance>),
    /// A hosted grant waiting on the person: authorize at
    /// `authorization_url`, then run `finish`.
    Pending {
        session_id: String,
        authorization_url: Option<String>,
        finish: String,
    },
}

/// Every authority source the project holds for connectors, by source.
#[derive(Clone, Debug)]
pub struct AuthoritySources {
    pub instances: Vec<super::instances::InstanceRecord>,
    pub profiles: Vec<CredentialProfileSummary>,
    /// The Connect grants the signed-in account holds, or why they could not
    /// be read (an unavailable account is never an empty list).
    pub grants: Result<Vec<Value>, String>,
}

/// What `runx connector revoke` removed.
#[derive(Clone, Debug)]
pub enum Revoked {
    Instance(String),
    Profile(String),
    Grant(String),
}

/// One connector setup over a project's instances and its hosted account.
pub struct ConnectorSetup<'a> {
    instances: &'a Instances,
    workspace: &'a WorkspaceEnv,
    hosted: HostedClient,
    transport: Option<Transport>,
}

impl<'a> ConnectorSetup<'a> {
    #[must_use]
    pub fn new(instances: &'a Instances, workspace: &'a WorkspaceEnv) -> Self {
        Self {
            instances,
            workspace,
            hosted: HostedClient::default(),
            transport: None,
        }
    }

    /// A setup reaching the hosted account through `transport`: tests and
    /// embedders verify the production path without live traffic.
    #[must_use]
    pub fn with_transport(
        instances: &'a Instances,
        workspace: &'a WorkspaceEnv,
        transport: Transport,
    ) -> Self {
        Self {
            instances,
            workspace,
            hosted: HostedClient::with_transport(Arc::clone(&transport)),
            transport: Some(transport),
        }
    }

    /// Confirm and select the instance `request` names for `connector`.
    pub fn use_connector(
        &self,
        connector: &LoadedConnector,
        request: UseRequest<'_>,
        options: &UseOptions,
    ) -> Result<UseOutcome, String> {
        let (env, cwd) = (self.workspace.env(), self.workspace.cwd());
        match request {
            UseRequest::Default => self.default_path(connector, options),
            UseRequest::Existing(name) => self.existing(connector, name, options),
            UseRequest::Session => {
                setup::confirm_session(self.instances, connector, &options.scopes, env, cwd)
                    .map(|instance| UseOutcome::Selected(Box::new(instance)))
            }
            UseRequest::Credential {
                credential,
                secret,
                profile,
            } => setup::set_credential(
                self.instances,
                connector,
                profile,
                credential,
                secret,
                &options.scopes,
                options.parameters.clone(),
                self.workspace,
            )
            .map(|instance| UseOutcome::Selected(Box::new(instance))),
            UseRequest::Hosted { credential, target } => {
                self.hosted_grant(connector, credential, target, options)
            }
        }
    }

    /// The authority sources the project holds: instances, local profiles,
    /// and the signed-in account's Connect grants.
    pub fn sources(&self) -> Result<AuthoritySources, String> {
        Ok(AuthoritySources {
            instances: self.instances.list().map_err(|error| error.to_string())?,
            profiles: list_local_credential_profiles(self.workspace)
                .map_err(|error| error.to_string())?,
            grants: self.grants(),
        })
    }

    /// Remove the one instance, profile or grant `name` names. A profile's
    /// removal also revokes the instances that hold it, and a grant's those
    /// that stand for it.
    pub fn revoke(&self, name: &str) -> Result<Revoked, String> {
        let instance = self
            .instances
            .get(name)
            .map_err(|error| error.to_string())?;
        let profile = self.profile(name)?;
        match (instance, profile) {
            (Some(_), Some(_)) => Err(format!(
                "{name} is both an instance id and a credential profile; nothing was revoked"
            )),
            (Some(_), None) => {
                self.instances
                    .revoke(name)
                    .map_err(|error| error.to_string())?;
                Ok(Revoked::Instance(name.to_owned()))
            }
            (None, Some(profile)) => {
                self.revoke_holding(|instance| {
                    profile_of(instance) == Some(profile.name.as_str())
                })?;
                remove_local_credential_profile(self.workspace, &profile.name)
                    .map_err(|error| error.to_string())?;
                Ok(Revoked::Profile(profile.name))
            }
            (None, None) => {
                self.hosted_connect(HostedConnectAction::Revoke { grant_id: name })
                    .map_err(|error| {
                        format!("{name} is no instance, profile or grant here: {error}")
                    })?;
                self.revoke_holding(|instance| grant_of(instance) == Some(name))?;
                Ok(Revoked::Grant(name.to_owned()))
            }
        }
    }

    fn default_path(
        &self,
        connector: &LoadedConnector,
        options: &UseOptions,
    ) -> Result<UseOutcome, String> {
        let confirmed = self
            .instances
            .list()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|record| {
                !record.revoked && record.instance.connector == connector.connector_ref
            })
            .map(|record| record.instance.instance_id.as_str().to_owned())
            .collect::<Vec<_>>();
        match confirmed.as_slice() {
            [only] => return self.existing(connector, only, options),
            [] => {}
            several => {
                return Err(format!(
                    "{} has several instances; name one: {}",
                    connector.connector_ref,
                    several
                        .iter()
                        .map(|id| format!("`runx connector use {} {id}`", connector.connector_ref))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        let paths = self.new_paths(connector)?;
        match paths.as_slice() {
            [Path::Public] => {
                setup::confirm_public(self.instances, connector, options.parameters.clone())
                    .map(|instance| UseOutcome::Selected(Box::new(instance)))
            }
            [Path::Session] => self.use_connector(connector, UseRequest::Session, options),
            [Path::Profile(name)] => self.existing(connector, name, options),
            [Path::Hosted] => self.use_connector(
                connector,
                UseRequest::Hosted {
                    credential: None,
                    target: None,
                },
                options,
            ),
            _ => Err(self.choose_message(connector, &paths)),
        }
    }

    /// The paths that confirm a new instance without anything only the
    /// operator can supply, such as a secret.
    fn new_paths(&self, connector: &LoadedConnector) -> Result<Vec<Path>, String> {
        let manifest = &connector.manifest;
        let local = manifest
            .placements
            .local
            .as_ref()
            .map(|local| local.auth.as_slice())
            .unwrap_or(&[]);
        let mut paths = Vec::new();
        if local.contains(&LocalAuth::None) {
            paths.push(Path::Public);
        }
        if local.contains(&LocalAuth::Session) {
            paths.push(Path::Session);
        }
        for profile in
            list_local_credential_profiles(self.workspace).map_err(|error| error.to_string())?
        {
            if profile.connector == connector.connector_ref {
                paths.push(Path::Profile(profile.name));
            }
        }
        if manifest.placements.hosted.is_some()
            && HostedApiEnvironment::resolve(None, self.workspace.env(), self.workspace.cwd())
                .is_ok_and(|environment| environment.require_token().is_ok())
        {
            paths.push(Path::Hosted);
        }
        Ok(paths)
    }

    fn choose_message(&self, connector: &LoadedConnector, paths: &[Path]) -> String {
        let reference = connector.connector_ref.as_str();
        let mut forms = paths
            .iter()
            .map(|path| match path {
                Path::Public => {
                    format!("`runx connector use {reference} --parameter <name=value>`")
                }
                Path::Session => format!("`runx connector use {reference} --session`"),
                Path::Profile(name) => format!("`runx connector use {reference} {name}`"),
                Path::Hosted => format!("`runx connector use {reference} --hosted`"),
            })
            .collect::<Vec<_>>();
        for credential in connector.manifest.auth.credentials.keys() {
            forms.push(format!(
                "`runx connector use {reference} --credential {credential}` with the secret on stdin"
            ));
        }
        if forms.is_empty() {
            return format!(
                "{reference} has no setup this project can complete; sign in with `runx login` for its hosted placement"
            );
        }
        format!(
            "{reference} can be set up more than one way; choose one: {}",
            forms.join(", ")
        )
    }

    /// The instance, profile, grant or pending Connect session `name` names.
    fn existing(
        &self,
        connector: &LoadedConnector,
        name: &str,
        options: &UseOptions,
    ) -> Result<UseOutcome, String> {
        let instance = self
            .instances
            .get(name)
            .map_err(|error| error.to_string())?;
        let profile = self.profile(name)?;
        match (instance, profile) {
            (Some(_), Some(_)) => Err(format!(
                "{name} is both an instance id and a credential profile; nothing was selected"
            )),
            (Some(record), None) => {
                if record.instance.connector != connector.connector_ref {
                    return Err(format!(
                        "instance {name} belongs to connector {}, not {}",
                        record.instance.connector, connector.connector_ref
                    ));
                }
                self.instances
                    .select(name)
                    .map(|instance| UseOutcome::Selected(Box::new(instance)))
                    .map_err(|error| error.to_string())
            }
            (None, Some(profile)) => setup::confirm_credential_profile(
                self.instances,
                connector,
                &profile.name,
                &options.scopes,
                options.parameters.clone(),
                self.workspace,
            )
            .map(|instance| UseOutcome::Selected(Box::new(instance))),
            (None, None) => self.hosted_existing(connector, name, options),
        }
    }

    /// An active grant by id, or a pending Connect session that has since
    /// produced one.
    fn hosted_existing(
        &self,
        connector: &LoadedConnector,
        name: &str,
        options: &UseOptions,
    ) -> Result<UseOutcome, String> {
        let (env, cwd) = (self.workspace.env(), self.workspace.cwd());
        let grant_error = match setup::confirm_grant_from(
            self.instances,
            connector,
            &self.hosted,
            name,
            env,
            cwd,
        ) {
            Ok(instance) => return Ok(UseOutcome::Selected(Box::new(instance))),
            Err(error) => error,
        };
        match self.hosted_connect(HostedConnectAction::Status { session_id: name }) {
            Ok(response) => self.settle(connector, name, response, options),
            Err(_) => Err(format!(
                "{name} is no instance, profile, grant or pending Connect session for {}: {grant_error}",
                connector.connector_ref
            )),
        }
    }

    fn hosted_grant(
        &self,
        connector: &LoadedConnector,
        credential: Option<(&CredentialName, &str)>,
        target: Option<&str>,
        options: &UseOptions,
    ) -> Result<UseOutcome, String> {
        let manifest = &connector.manifest;
        let hosted = manifest
            .placements
            .hosted
            .as_ref()
            .ok_or_else(|| format!("{} has no hosted placement", connector.connector_ref))?;
        let scopes = if options.scopes.is_empty() {
            let mut declared = manifest
                .operations
                .values()
                .flat_map(|operation| {
                    operation
                        .scopes
                        .iter()
                        .map(|scope| scope.as_str().to_owned())
                })
                .collect::<Vec<_>>();
            declared.sort();
            declared.dedup();
            declared
        } else {
            options.scopes.clone()
        };
        let (target_repo, target_locator) = match target {
            Some(target) if is_owner_repo(target) => (Some(target), None),
            Some(target) => (None, Some(target)),
            None => (None, None),
        };
        let started = self.hosted_connect(HostedConnectAction::Start(HostedConnectStart {
            provider: manifest.id.as_str(),
            scopes: &scopes,
            scope_family: None,
            authority_kind: None,
            target_repo,
            target_locator,
            binding_id: None,
            credential_grant_id: None,
        }))?;
        let started = match (status(&started), credential) {
            ("credential_required", Some((name, secret))) => {
                let session_id = field(&started, "session_id")?;
                let credentials = self.credential_fields(connector, name, secret, hosted)?;
                self.hosted_connect(HostedConnectAction::SubmitCredentials {
                    session_id: &session_id,
                    credentials: &credentials,
                })?
            }
            ("credential_required", None) => return Err(needs_credential(connector)),
            _ => started,
        };
        let session_id = field(&started, "session_id").unwrap_or_default();
        self.settle(connector, &session_id, started, options)
    }

    /// Bind the grant a Connect response carries, or wait for it within
    /// `options.wait`, then return what finishes it.
    fn settle(
        &self,
        connector: &LoadedConnector,
        session_id: &str,
        mut response: Value,
        options: &UseOptions,
    ) -> Result<UseOutcome, String> {
        let deadline = Instant::now() + options.wait;
        let authorization_url = response
            .get("authorization_url")
            .and_then(Value::as_str)
            .map(str::to_owned);
        loop {
            match status(&response) {
                "created" | "unchanged" => {
                    let grant_id = response
                        .get("grant")
                        .and_then(|grant| grant.get("grant_id"))
                        .and_then(Value::as_str)
                        .ok_or("the Connect response names no grant")?
                        .to_owned();
                    let (env, cwd) = (self.workspace.env(), self.workspace.cwd());
                    return setup::confirm_grant_from(
                        self.instances,
                        connector,
                        &self.hosted,
                        &grant_id,
                        env,
                        cwd,
                    )
                    .map(|instance| UseOutcome::Selected(Box::new(instance)));
                }
                "pending" | "pending_verification" => {}
                // A session holding an authorization link awaits the account holder.
                _ if response.get("authorization_url").is_some() => {}
                "credential_required" => return Err(needs_credential(connector)),
                other => {
                    return Err(match response.get("error").and_then(Value::as_str) {
                        Some(error) => format!("the Connect session is {other}: {error}"),
                        None => format!("the Connect session is {other}"),
                    });
                }
            }
            if session_id.is_empty() || Instant::now() >= deadline {
                return Ok(UseOutcome::Pending {
                    session_id: session_id.to_owned(),
                    authorization_url,
                    finish: format!(
                        "runx connector use {} {session_id}",
                        connector.connector_ref
                    ),
                });
            }
            // The session says when to ask again; never sooner than MIN_POLL.
            let pause = response
                .get("poll_after_ms")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_POLL, Duration::from_millis)
                .max(MIN_POLL);
            std::thread::sleep(pause.min(deadline.saturating_duration_since(Instant::now())));
            response = self.hosted_connect(HostedConnectAction::Status { session_id })?;
        }
    }

    fn credential_fields(
        &self,
        connector: &LoadedConnector,
        name: &CredentialName,
        secret: &str,
        hosted: &runx_contracts::connector::HostedPlacement,
    ) -> Result<Value, String> {
        if hosted
            .credential
            .as_ref()
            .is_some_and(|declared| declared != name)
        {
            return Err(format!(
                "{}'s hosted placement takes credential {}, not {name}",
                connector.connector_ref,
                hosted
                    .credential
                    .as_ref()
                    .map(CredentialName::as_str)
                    .unwrap_or_default()
            ));
        }
        let declaration = connector
            .manifest
            .auth
            .credentials
            .get(name)
            .ok_or_else(|| format!("{} declares no credential {name}", connector.connector_ref))?;
        let fields = super::operation::credentials::fields(declaration, secret)
            .map_err(|reason| format!("the secret {reason}"))?;
        Ok(Value::Object(
            fields
                .into_iter()
                .map(|(field, value)| (field, Value::String(value)))
                .collect(),
        ))
    }

    fn hosted_connect(&self, action: HostedConnectAction<'_>) -> Result<Value, String> {
        let (env, cwd) = (self.workspace.env(), self.workspace.cwd());
        let transport: Transport = match &self.transport {
            Some(transport) => Arc::clone(transport),
            None => Arc::new(
                crate::hosted_api_transport(crate::hosted_private_network_allowed(false, env))
                    .map_err(|error| error.to_string())?,
            ),
        };
        let environment = HostedApiEnvironment::resolve(None, env, cwd)
            .and_then(|environment| environment.authenticate(transport.as_ref()))
            .map_err(|error| error.to_string())?;
        execute_hosted_connect(transport.as_ref(), &environment, action)
            .map_err(|error| error.to_string())
    }

    fn grants(&self) -> Result<Vec<Value>, String> {
        let listed = self.hosted_connect(HostedConnectAction::List)?;
        Ok(listed
            .get("grants")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    fn profile(&self, name: &str) -> Result<Option<CredentialProfileSummary>, String> {
        Ok(list_local_credential_profiles(self.workspace)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|profile| profile.name == name))
    }

    fn revoke_holding(&self, holds: impl Fn(&ConnectorInstance) -> bool) -> Result<(), String> {
        for record in self.instances.list().map_err(|error| error.to_string())? {
            if !record.revoked && holds(&record.instance) {
                self.instances
                    .revoke(record.instance.instance_id.as_str())
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }
}

enum Path {
    Public,
    Session,
    Profile(String),
    Hosted,
}

fn needs_credential(connector: &LoadedConnector) -> String {
    format!(
        "{}'s hosted placement takes your own credential; run `runx connector use {} --hosted --credential <name>` with the secret on stdin",
        connector.connector_ref, connector.connector_ref
    )
}

fn status(response: &Value) -> &str {
    response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn field(response: &Value, name: &str) -> Result<String, String> {
    response
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("the Connect response names no {name}"))
}

/// `owner/repo`, the shape Connect narrows a repository grant by.
fn is_owner_repo(target: &str) -> bool {
    let mut parts = target.split('/');
    let valid = |part: &str| {
        !part.is_empty()
            && part.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
            })
    };
    matches!((parts.next(), parts.next(), parts.next()), (Some(owner), Some(repo), None) if valid(owner) && valid(repo))
}

fn profile_of(instance: &ConnectorInstance) -> Option<&str> {
    match &instance.authority {
        runx_contracts::InstanceAuthority::Credential { profile, .. } => Some(profile.as_str()),
        _ => None,
    }
}

fn grant_of(instance: &ConnectorInstance) -> Option<&str> {
    match &instance.authority {
        runx_contracts::InstanceAuthority::HostedGrant { grant_id } => Some(grant_id.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
