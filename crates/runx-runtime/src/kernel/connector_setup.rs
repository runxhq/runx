//! One place a connector's authority is set up, listed and removed:
//! `runx connector use`, `runx connector list` and `runx connector revoke`.
//!
//! Five authority sources exist: an instance confirmed earlier, a local
//! command session (a signed-in tool such as `gh`), the variable a
//! credential's declaration delivers it in, a local secret profile, and a
//! runx Connect grant. Setup composes the confirmations in [`super::setup`];
//! it never picks between accounts for the operator, and a request it cannot
//! resolve to one source is refused with the exact forms that would.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use runx_contracts::connector::{CredentialName, EnvName, HostedAuth, LocalAuth, SecretReference};
use runx_contracts::{ConnectorInstance, InstanceAuthority, JsonObject};
use serde_json::Value;

use super::connectors::LoadedConnector;
use super::instances::{InstanceRecord, Instances};
use super::operation::hosted::{HostedClient, HostedSession};
use super::setup;
use crate::credential_profiles::{
    CredentialProfileSummary, list_local_credential_profiles, remove_local_credential_profile,
};
use crate::hosted_api::{
    ConnectSession, HostedApiEnvironment, HostedConnectAction, HostedConnectStart,
};
use crate::http::RuntimeHttpTransport;
use crate::provider_operations::HostedProviderGrant;
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
    /// The connector's `credential`: bound to the variable its declaration
    /// delivers it in when that is set, else read from the operator and
    /// stored in `profile`, the connector's own name when none is given.
    Credential {
        credential: &'a CredentialName,
        profile: Option<&'a str>,
    },
    /// The connector's `credential`, or its only one, read on every call
    /// from the secret `reference` names in a secrets manager.
    Reference {
        credential: Option<&'a CredentialName>,
        reference: &'a SecretReference,
    },
    /// A new runx Connect grant: delegated, or holding the one credential the
    /// connector's hosted placement names, read from `from` when given, else
    /// from its variable when that is set, else from the operator.
    Hosted {
        target: Option<&'a str>,
        from: Option<&'a SecretReference>,
    },
}

/// Asks the operator for a credential whose variable is not set, given what
/// to ask for: the CLI asks on a terminal and reads stdin otherwise.
pub type SecretReader<'a> = &'a dyn Fn(&str) -> Result<String, String>;

/// Shows the operator where to authorize a new Connect grant, given the
/// connector and the link, as soon as Connect returns it: the wait for the
/// grant is only useful once the person has the link.
pub type LinkShower<'a> = &'a dyn Fn(&str, &str);

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
    pub instances: Vec<InstanceRecord>,
    pub profiles: Vec<CredentialProfileSummary>,
    /// The Connect grants the signed-in account holds, or why they could not
    /// be read (an unavailable account is never an empty list).
    pub grants: Result<Vec<HostedProviderGrant>, String>,
}

impl AuthoritySources {
    /// The instance `connector` runs under, if one is selected.
    #[must_use]
    pub fn selected(&self, connector: &LoadedConnector) -> Option<&InstanceRecord> {
        self.instances.iter().find(|record| {
            record.selected && !record.revoked && record.instance.connector == connector.name
        })
    }
}

/// What `runx connector revoke` removed.
#[derive(Clone, Debug)]
pub enum Revoked {
    Instance(String),
    Profile(String),
    Grant(String),
}

/// One command that sets a connector up on a placement it declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupForm {
    pub command: String,
    /// The variable the command reads the credential from; when it is not
    /// set, the command asks for the credential.
    pub variable: Option<EnvName>,
    /// The program a session form runs for its token, as the manifest
    /// declares it.
    pub runs: Option<String>,
}

impl fmt::Display for SetupForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.command)?;
        if let Some(variable) = &self.variable {
            write!(formatter, " (reads {variable}, else asks for it)")?;
        }
        if let Some(runs) = &self.runs {
            write!(formatter, " (runs `{runs}`)")?;
        }
        Ok(())
    }
}

impl SetupForm {
    /// The form in a refusal: the command quoted so it can be run as given.
    fn quoted(&self) -> String {
        let mut quoted = format!("`{}`", self.command);
        if let Some(variable) = &self.variable {
            quoted.push_str(&format!(" (reads {variable}, else asks for it)"));
        }
        if let Some(runs) = &self.runs {
            quoted.push_str(&format!(" (runs `{runs}`)"));
        }
        quoted
    }
}

/// The commands that set `connector` up, one per way each placement it
/// declares authorizes it: what `runx connector inspect` lists and what an
/// ambiguous `runx connector use` offers. A public API needs none: it runs
/// under its public instance until another is selected.
#[must_use]
pub fn setup_forms(connector: &LoadedConnector) -> Vec<SetupForm> {
    let manifest = &connector.manifest;
    let use_ = format!("runx connector use {}", connector.name);
    let credential = |name: &CredentialName| SetupForm {
        command: format!("{use_} --credential {name}"),
        variable: manifest
            .auth
            .credentials
            .get(name)
            .map(|declaration| declaration.delivery.env.clone()),
        runs: None,
    };
    let mut forms = Vec::new();
    if let Some(local) = &manifest.placements.local {
        for auth in local.auth.iter() {
            match auth {
                LocalAuth::None => {}
                LocalAuth::Session => {
                    if let Some(session) = super::session::CommandSession::of(manifest) {
                        forms.push(SetupForm {
                            command: format!("{use_} --session"),
                            variable: None,
                            runs: Some(session.command_line()),
                        });
                    }
                }
                LocalAuth::Credentials => {
                    forms.extend(manifest.auth.credentials.keys().map(credential))
                }
            }
        }
    }
    if let Some(hosted) = &manifest.placements.hosted {
        forms.push(SetupForm {
            command: format!("{use_} --hosted"),
            variable: hosted
                .credential
                .as_ref()
                .and_then(|name| manifest.auth.credentials.get(name))
                .map(|declaration| declaration.delivery.env.clone()),
            runs: None,
        });
    }
    forms
}

/// One connector setup over a project's instances and its hosted account.
pub struct ConnectorSetup<'a> {
    instances: &'a Instances,
    workspace: &'a WorkspaceEnv,
    /// The hosted account, authenticated once per setup.
    hosted: HostedClient,
    /// The wire a local command session's account is read over, when an
    /// embedder gives one.
    transport: Option<Transport>,
    /// Asks for a credential whose variable is not set, when the embedder
    /// can ask anyone.
    secrets: Option<SecretReader<'a>>,
    /// Shows a new grant's authorization link, when the embedder can show
    /// anyone.
    links: Option<LinkShower<'a>>,
}

impl<'a> ConnectorSetup<'a> {
    #[must_use]
    pub fn new(instances: &'a Instances, workspace: &'a WorkspaceEnv) -> Self {
        Self {
            instances,
            workspace,
            hosted: HostedClient::default(),
            transport: None,
            secrets: None,
            links: None,
        }
    }

    /// This setup, asking `reader` for a credential whose variable is not
    /// set. Without one, such a credential is refused with its variable.
    #[must_use]
    pub fn reading_secrets(self, reader: SecretReader<'a>) -> Self {
        Self {
            secrets: Some(reader),
            ..self
        }
    }

    /// This setup, showing a new grant's authorization link through `shower`
    /// before it waits. Without one, the link comes back only with a grant
    /// still pending.
    #[must_use]
    pub fn showing_links(self, shower: LinkShower<'a>) -> Self {
        Self {
            links: Some(shower),
            ..self
        }
    }

    /// A setup reaching the hosted account and local sessions through
    /// `transport`: tests and embedders verify the production path without
    /// live traffic.
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
            secrets: None,
            links: None,
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
        let selected = |instance| UseOutcome::Selected(Box::new(instance));
        match request {
            UseRequest::Default => self.default_path(connector, options),
            UseRequest::Existing(name) => self.existing(connector, name, options),
            UseRequest::Session => {
                no_parameters(options, "a session")?;
                match &self.transport {
                    Some(transport) => setup::confirm_session_over(
                        self.instances,
                        connector,
                        &options.scopes,
                        transport.as_ref(),
                        env,
                        cwd,
                    ),
                    None => {
                        setup::confirm_session(self.instances, connector, &options.scopes, env, cwd)
                    }
                }
                .map(selected)
            }
            UseRequest::Credential {
                credential,
                profile,
            } => self
                .credential(connector, credential, profile, options)
                .map(selected),
            UseRequest::Reference {
                credential,
                reference,
            } => {
                let credential = match credential {
                    Some(credential) => credential.clone(),
                    None => only_credential(connector)?,
                };
                setup::bind_reference(
                    self.instances,
                    connector,
                    &credential,
                    reference,
                    &options.scopes,
                    options.parameters.clone(),
                    self.workspace,
                )
                .map(selected)
            }
            UseRequest::Hosted { target, from } => {
                self.hosted_grant(connector, target, from, options)
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
            grants: self
                .hosted
                .list_grants(self.workspace.env(), self.workspace.cwd()),
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
                self.session()?
                    .connect(HostedConnectAction::Revoke { grant_id: name })
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
            .filter(|record| !record.revoked && record.instance.connector == connector.name)
            .map(|record| record.instance.instance_id.as_str().to_owned())
            .collect::<Vec<_>>();
        match confirmed.as_slice() {
            [only] => return self.existing(connector, only, options),
            [] => {}
            several => {
                return Err(format!(
                    "{} has several instances; name one: {}",
                    connector.name,
                    several
                        .iter()
                        .map(|id| format!("`runx connector use {} {id}`", connector.name))
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
            [Path::Credential(credential)] => self
                .credential(connector, credential, None, options)
                .map(|instance| UseOutcome::Selected(Box::new(instance))),
            [Path::Profile(name)] => self.existing(connector, name, options),
            [Path::Hosted] => self.use_connector(
                connector,
                UseRequest::Hosted {
                    target: None,
                    from: None,
                },
                options,
            ),
            _ => Err(self.choose_message(connector, &paths)),
        }
    }

    /// The paths that confirm a new instance from what is already here: a
    /// credential's variable, a stored profile, a Connect account. A public
    /// API is the floor, taken only when no account is available; past it, a
    /// connector that takes one credential asks for that credential. A
    /// declared command session is a path that is never taken by default: its
    /// command comes from the manifest, so it runs only once the operator
    /// names it, and nothing runs it to find out whether it would work.
    fn new_paths(&self, connector: &LoadedConnector) -> Result<Vec<Path>, String> {
        let manifest = &connector.manifest;
        let local = manifest
            .placements
            .local
            .as_ref()
            .map(|local| local.auth.as_slice())
            .unwrap_or(&[]);
        let mut paths = Vec::new();
        if local.contains(&LocalAuth::Session) && command_session(connector) {
            paths.push(Path::Session);
        }
        if local.contains(&LocalAuth::Credentials) {
            for (name, declaration) in &manifest.auth.credentials {
                if self
                    .variable(connector, name, &declaration.delivery.env)?
                    .is_some()
                {
                    paths.push(Path::Credential(name.clone()));
                }
            }
            for profile in
                list_local_credential_profiles(self.workspace).map_err(|error| error.to_string())?
            {
                if profile.connector == connector.name {
                    paths.push(Path::Profile(profile.name));
                }
            }
        }
        // A delegated grant is the account holder's to authorize; the
        // operator's own credential goes to Connect only when it has no local
        // home.
        let hosted = manifest
            .placements
            .hosted
            .as_ref()
            .is_some_and(|hosted| match hosted.auth {
                HostedAuth::Delegated => true,
                HostedAuth::Credential => !local.contains(&LocalAuth::Credentials),
            });
        if hosted && self.signed_in()? {
            paths.push(Path::Hosted);
        }
        if paths.is_empty() && local.contains(&LocalAuth::None) {
            paths.push(Path::Public);
        }
        if paths.is_empty()
            && local.contains(&LocalAuth::Credentials)
            && let [only] = manifest
                .auth
                .credentials
                .keys()
                .collect::<Vec<_>>()
                .as_slice()
        {
            paths.push(Path::Credential((*only).clone()));
        }
        Ok(paths)
    }

    /// The connector's `credential`, bound to its variable when that is set;
    /// otherwise read from the operator and stored in `profile`.
    fn credential(
        &self,
        connector: &LoadedConnector,
        credential: &CredentialName,
        profile: Option<&str>,
        options: &UseOptions,
    ) -> Result<ConnectorInstance, String> {
        let variable = &declared(connector, credential)?.delivery.env;
        if self.variable(connector, credential, variable)?.is_some() {
            if profile.is_some() {
                return Err(format!(
                    "{}'s {credential} credential is read from {variable}, which is set; --profile stores a credential only when its variable is not",
                    connector.name
                ));
            }
            return setup::bind_environment(
                self.instances,
                connector,
                credential,
                &options.scopes,
                options.parameters.clone(),
                self.workspace,
            );
        }
        let secret = self.secret(connector, credential)?;
        setup::set_credential(
            self.instances,
            connector,
            profile.unwrap_or(connector.name.as_str()),
            credential,
            &secret,
            &options.scopes,
            options.parameters.clone(),
            self.workspace,
        )
    }

    /// `credential`'s value for a new profile or hosted grant: its variable
    /// when that is set, else what the operator gives.
    fn secret(
        &self,
        connector: &LoadedConnector,
        credential: &CredentialName,
    ) -> Result<String, String> {
        let declaration = declared(connector, credential)?;
        let variable = &declaration.delivery.env;
        if let Some(value) = self.variable(connector, credential, variable)? {
            return Ok(value.to_owned());
        }
        let read = self.secrets.ok_or_else(|| {
            format!(
                "{} needs its {credential} credential: set {variable}, or run `runx connector use` where it can ask for it",
                connector.name
            )
        })?;
        let wanted = match declaration.fields.as_slice() {
            [_] => format!("{} {credential}", connector.name),
            fields => format!(
                "{} {credential}, a JSON object of {}",
                connector.name,
                fields
                    .iter()
                    .map(|field| field.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        read(&wanted).map_err(|error| format!("{error} (or set {variable})"))
    }

    /// The workspace's value for `credential`'s `variable`, when it holds one;
    /// refused when the environment and the `.env` disagree on it, whatever
    /// either holds.
    fn variable(
        &self,
        connector: &LoadedConnector,
        credential: &CredentialName,
        variable: &EnvName,
    ) -> Result<Option<&str>, String> {
        super::operation::credentials::one_value(
            self.workspace.env(),
            connector,
            credential,
            variable,
        )?;
        Ok(self
            .workspace
            .env()
            .get(variable.as_str())
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty()))
    }

    /// Whether the runx account is signed in. Only a missing token is "not
    /// signed in": two tokens that differ, or a stored one that cannot be
    /// read, are refused rather than read as signed out.
    fn signed_in(&self) -> Result<bool, String> {
        let environment =
            HostedApiEnvironment::resolve(None, self.workspace.env(), self.workspace.cwd())
                .map_err(|error| error.to_string())?;
        match environment.require_token() {
            Ok(_) => Ok(true),
            Err(crate::hosted_api::HostedApiError::MissingToken) => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    fn choose_message(&self, connector: &LoadedConnector, paths: &[Path]) -> String {
        let reference = connector.name.as_str();
        let mut forms = paths
            .iter()
            .filter_map(|path| match path {
                Path::Profile(name) => Some(format!("`runx connector use {reference} {name}`")),
                _ => None,
            })
            .collect::<Vec<_>>();
        forms.extend(setup_forms(connector).iter().map(SetupForm::quoted));
        let login = match connector.manifest.placements.hosted.is_some()
            && matches!(self.signed_in(), Ok(false))
        {
            true => "; a hosted form needs `runx login` first",
            false => "",
        };
        // A declared session is offered, never already set up.
        match paths
            .iter()
            .filter(|path| !matches!(path, Path::Session))
            .count()
        {
            0 => format!(
                "{reference} needs setup; run one of: {}{login}",
                forms.join(", ")
            ),
            _ => format!(
                "{reference} can be set up more than one way; choose one: {}{login}",
                forms.join(", ")
            ),
        }
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
                if record.instance.connector != connector.name {
                    return Err(format!(
                        "instance {name} belongs to connector {}, not {}",
                        record.instance.connector, connector.name
                    ));
                }
                fixed(options, &format!("instance {name}"))?;
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
            (None, None) => {
                fixed(options, name)?;
                self.hosted_existing(connector, name, options)
            }
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
        if self
            .hosted
            .list_grants(env, cwd)?
            .iter()
            .any(|grant| grant.grant_id == name)
        {
            return setup::confirm_grant_from(
                self.instances,
                connector,
                &self.hosted,
                name,
                env,
                cwd,
            )
            .map(|instance| UseOutcome::Selected(Box::new(instance)));
        }
        let session = self
            .session()?
            .connect(HostedConnectAction::Status { session_id: name })
            .map_err(|error| {
                format!(
                    "{name} is no instance, profile, grant or pending Connect session for {}: {error}",
                    connector.name
                )
            })?;
        self.settle(connector, ConnectSession::from_response(session)?, options)
    }

    fn hosted_grant(
        &self,
        connector: &LoadedConnector,
        target: Option<&str>,
        from: Option<&SecretReference>,
        options: &UseOptions,
    ) -> Result<UseOutcome, String> {
        no_parameters(options, "a hosted grant")?;
        let manifest = &connector.manifest;
        let hosted = manifest
            .placements
            .hosted
            .as_ref()
            .ok_or_else(|| format!("{} has no hosted placement", connector.name))?;
        let credentials = match (hosted.auth, &hosted.credential) {
            (HostedAuth::Delegated, _) if from.is_some() => {
                return Err(format!(
                    "{} signs in to its provider through Connect; --from seeds only a hosted credential",
                    connector.name
                ));
            }
            (HostedAuth::Delegated, _) => None,
            (HostedAuth::Credential, Some(name)) => {
                Some(self.credential_fields(connector, name, from)?)
            }
            (HostedAuth::Credential, None) => {
                return Err(format!(
                    "{}'s hosted placement names no credential",
                    connector.name
                ));
            }
        };
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
        let session = self.session()?;
        let mut opened = ConnectSession::from_response(
            session
                .connect(HostedConnectAction::Start(HostedConnectStart {
                    connector_binding: &connector.binding(),
                    scopes: &scopes,
                    target,
                }))
                .map_err(|error| error.to_string())?,
        )?;
        if let (ConnectSession::CredentialRequired { session_id }, Some(credentials)) =
            (&opened, &credentials)
        {
            opened = ConnectSession::from_response(
                session
                    .connect(HostedConnectAction::SubmitCredentials {
                        session_id,
                        credentials,
                    })
                    .map_err(|error| error.to_string())?,
            )?;
        }
        self.settle(connector, opened, options)
    }

    /// Bind the grant a Connect session made, or wait for it within
    /// `options.wait`, then return what finishes it.
    fn settle(
        &self,
        connector: &LoadedConnector,
        mut current: ConnectSession,
        options: &UseOptions,
    ) -> Result<UseOutcome, String> {
        let deadline = Instant::now() + options.wait;
        let mut authorization_url = None;
        loop {
            let (session_id, poll_after_ms) = match current {
                ConnectSession::Created { grant } | ConnectSession::Unchanged { grant } => {
                    let (env, cwd) = (self.workspace.env(), self.workspace.cwd());
                    // The grant is new: list again so it is found.
                    self.hosted.list_grants(env, cwd)?;
                    return setup::confirm_grant_from(
                        self.instances,
                        connector,
                        &self.hosted,
                        &grant.grant_id,
                        env,
                        cwd,
                    )
                    .map(|instance| UseOutcome::Selected(Box::new(instance)));
                }
                ConnectSession::Pending {
                    session_id,
                    authorization_url: url,
                    poll_after_ms,
                } => {
                    // Shown once, the moment it is known, so the wait is
                    // spent with the person holding the link.
                    if authorization_url.is_none()
                        && let (Some(url), Some(show)) = (url.as_deref(), self.links)
                    {
                        show(connector.name.as_str(), url);
                    }
                    authorization_url = url.or(authorization_url);
                    (session_id, poll_after_ms)
                }
                ConnectSession::PendingVerification {
                    session_id,
                    poll_after_ms,
                } => (session_id, poll_after_ms),
                ConnectSession::CredentialRequired { .. } => {
                    return Err(needs_credential(connector));
                }
                ConnectSession::Failed { error }
                | ConnectSession::VerificationFailed { error }
                | ConnectSession::ConnectUnavailable { error }
                | ConnectSession::Unsupported { error } => {
                    return Err(format!(
                        "runx Connect could not grant {}: {}",
                        connector.name,
                        error.as_deref().unwrap_or("no reason given")
                    ));
                }
            };
            if Instant::now() >= deadline {
                return Ok(UseOutcome::Pending {
                    finish: format!("runx connector use {} {session_id}", connector.name),
                    session_id,
                    authorization_url,
                });
            }
            // The session says when to ask again; never sooner than MIN_POLL.
            let pause = poll_after_ms
                .map_or(DEFAULT_POLL, Duration::from_millis)
                .max(MIN_POLL);
            std::thread::sleep(pause.min(deadline.saturating_duration_since(Instant::now())));
            current = ConnectSession::from_response(
                self.session()?
                    .connect(HostedConnectAction::Status {
                        session_id: &session_id,
                    })
                    .map_err(|error| error.to_string())?,
            )?;
        }
    }

    fn credential_fields(
        &self,
        connector: &LoadedConnector,
        name: &CredentialName,
        from: Option<&SecretReference>,
    ) -> Result<Value, String> {
        let declaration = declared(connector, name)?;
        let fields = match from {
            Some(reference) => {
                let read = crate::secret_refs::read(
                    self.workspace.env(),
                    self.workspace.cwd(),
                    reference,
                    None,
                    crate::secret_refs::Purpose::Bind,
                )?;
                super::operation::credentials::reference_fields(
                    declaration,
                    reference,
                    &read.value,
                )?
            }
            None => {
                super::operation::credentials::fields(declaration, &self.secret(connector, name)?)
                    .map_err(|reason| format!("the credential {reason}"))?
            }
        };
        Ok(Value::Object(
            fields
                .into_iter()
                .map(|(field, value)| (field, Value::String(value)))
                .collect(),
        ))
    }

    /// The signed-in hosted account, authenticated once per setup.
    fn session(&self) -> Result<HostedSession, String> {
        self.hosted
            .session(self.workspace.env(), self.workspace.cwd())
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
    /// A declared command session: offered, never taken by default.
    Session,
    /// A credential, from its variable when that is set, else asked for.
    Credential(CredentialName),
    Profile(String),
    Hosted,
}

fn declared<'a>(
    connector: &'a LoadedConnector,
    credential: &CredentialName,
) -> Result<&'a runx_contracts::connector::CredentialDeclaration, String> {
    connector
        .manifest
        .auth
        .credentials
        .get(credential)
        .ok_or_else(|| format!("{} declares no credential {credential}", connector.name))
}

/// A session the runtime confirms by running the command that prints its
/// token; a host browser session is the host's to confirm.
fn command_session(connector: &LoadedConnector) -> bool {
    connector
        .manifest
        .auth
        .session
        .as_ref()
        .is_some_and(|session| session.kind() == runx_contracts::connector::SessionKind::Command)
}

/// A Connect session left waiting for a credential: a new one sends it.
fn needs_credential(connector: &LoadedConnector) -> String {
    let forms = setup_forms(connector)
        .iter()
        .filter(|form| form.command.ends_with(" --hosted"))
        .map(SetupForm::quoted)
        .collect::<Vec<_>>();
    format!(
        "{}'s Connect session waits for a credential; run {}",
        connector.name,
        forms.join(" or ")
    )
}

/// An instance, grant or session keeps the scopes and parameters it was
/// confirmed with; naming new ones is refused rather than ignored.
fn fixed(options: &UseOptions, what: &str) -> Result<(), String> {
    if options.scopes.is_empty() && options.parameters.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{what} keeps the scopes and parameters it was confirmed with; --scope and --parameter apply to a new instance"
    ))
}

/// The connector's one credential, for a reference that names none.
fn only_credential(connector: &LoadedConnector) -> Result<CredentialName, String> {
    let names = connector
        .manifest
        .auth
        .credentials
        .keys()
        .collect::<Vec<_>>();
    match names.as_slice() {
        [name] => Ok((*name).clone()),
        [] => Err(format!("{} declares no credential", connector.name)),
        names => Err(format!(
            "{} declares {}; name one with --credential",
            connector.name,
            names
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(" and ")
        )),
    }
}

fn no_parameters(options: &UseOptions, what: &str) -> Result<(), String> {
    if options.parameters.is_empty() {
        return Ok(());
    }
    Err(format!("{what} takes no --parameter"))
}

fn profile_of(instance: &ConnectorInstance) -> Option<&str> {
    match &instance.authority {
        InstanceAuthority::Credential { profile, .. } => Some(profile.as_str()),
        _ => None,
    }
}

fn grant_of(instance: &ConnectorInstance) -> Option<&str> {
    match &instance.authority {
        InstanceAuthority::HostedGrant { grant_id } => Some(grant_id.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
