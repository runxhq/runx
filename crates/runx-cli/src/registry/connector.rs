//! `runx registry connector package|publish|read|install`. Packaging reads a
//! connector directory with the runtime's own loader. Publish, read and
//! install use a hosted registry, the only registry that holds published
//! connectors; install verifies the version and writes it to the local
//! package cache (`RunxPaths::installed_connectors()`).

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::ConnectorRef;
use runx_runtime::registry::{
    HostedConnectorPublishRequest, InstallRegistryConnectorOptions, install_registry_connector,
    prepare_registry_connector_package, publish_hosted_connector,
};

use super::output::{self, RegistryEnvelope, RegistryPayload};
use super::{
    RegistryCliError, RegistryCliOutput, RegistryPlan, RegistryTarget, internal_error,
    remote_registry_client, resolve_registry_target, trusted_manifest_keys_from_env_for_source,
    usage_error,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectorAction {
    /// `package <dir>`: a connector packaged on its own.
    Package,
    /// `publish <dir>`: that package, published to a hosted registry.
    Publish,
    /// `read <publisher>/<id>[@version]`.
    Read,
    /// `install <publisher>/<id>[@version]`: into the local package cache.
    Install,
}

impl ConnectorAction {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "package" => Ok(Self::Package),
            "publish" => Ok(Self::Publish),
            "read" => Ok(Self::Read),
            "install" => Ok(Self::Install),
            other => Err(format!("unknown registry connector subcommand {other}")),
        }
    }
}

/// `<publisher>/<id>` and the version after `@`, when one is named.
pub(crate) fn parse_connector_subject(
    value: &str,
) -> Result<(ConnectorRef, Option<String>), String> {
    let (connector_ref, version) = match value.split_once('@') {
        Some((connector_ref, version)) => (connector_ref, Some(version)),
        None => (value, None),
    };
    let connector_ref = ConnectorRef::new(connector_ref).ok_or_else(|| {
        format!("{value} is not a connector ref; expected <publisher>/<id>[@version]")
    })?;
    if let Some(version) = version
        && !runx_contracts::is_registry_version(version)
    {
        return Err(format!("{version} is not a registry version label"));
    }
    Ok((connector_ref, version.map(str::to_owned)))
}

pub(super) fn run_connector(
    action: ConnectorAction,
    plan: RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    match action {
        ConnectorAction::Package => package(plan, env, cwd),
        ConnectorAction::Publish => publish(plan, env, cwd),
        ConnectorAction::Read => read(plan, env, cwd),
        ConnectorAction::Install => install(plan, env, cwd),
    }
}

// Function rationale: a standalone connector's package is the exact artifact
// its publisher submits, read by the same loader the runtime uses.
fn package(
    plan: RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let directory = runx_runtime::RunxPaths::resolve(env, cwd).existing_input(&plan.subject);
    let connector = prepare_registry_connector_package(&directory)
        .map_err(|error| internal_error(error.to_string()))?;
    output::write_output(
        plan.json,
        &RegistryEnvelope {
            status: "success",
            registry: RegistryPayload::ConnectorPackage {
                connector: Box::new(connector),
            },
        },
        || "\n  registry connector package  ready\n\n".to_owned(),
    )
}

fn publish(
    plan: RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let directory = runx_runtime::RunxPaths::resolve(env, cwd).existing_input(&plan.subject);
    let package = prepare_registry_connector_package(&directory)
        .map_err(|error| internal_error(error.to_string()))?;
    let registry_url = hosted_registry(&plan, env, cwd, "publish")?;
    let environment =
        runx_runtime::HostedApiEnvironment::resolve_publish(Some(&registry_url), env, cwd)
            .map_err(|error| usage_error(error.to_string()))?;
    let transport = runx_runtime::hosted_api_transport(
        runx_runtime::hosted_private_network_allowed(false, env),
    )
    .map_err(|error| internal_error(error.to_string()))?;
    let authenticated = environment
        .authenticate(&transport)
        .map_err(|error| usage_error(error.to_string()))?;
    let published = publish_hosted_connector(
        &transport,
        authenticated.base_url(),
        authenticated.token(),
        &HostedConnectorPublishRequest {
            version: plan.version.as_deref(),
            files: &package.files,
        },
    )
    .map_err(|error| internal_error(error.to_string()))?;
    // The registry reads the files natively; it must have recorded these.
    if published.connector_ref != package.connector_ref
        || published.content_digest != package.content_digest
    {
        return Err(internal_error(format!(
            "the registry recorded {} with content {}, but the package is {} with content {}",
            published.connector_ref,
            published.content_digest,
            package.connector_ref,
            package.content_digest
        )));
    }
    let human = output::render_connector_publish(&published);
    output::write_output(
        plan.json,
        &RegistryEnvelope {
            status: "success",
            registry: RegistryPayload::ConnectorPublish {
                publish: Box::new(published),
            },
        },
        || human,
    )
}

fn read(
    plan: RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let (connector_ref, version) = subject(&plan)?;
    let registry_url = hosted_registry(&plan, env, cwd, "read")?;
    let connector = remote_registry_client(&registry_url, env)?
        .read_connector(&connector_ref, version.as_deref())?
        .ok_or_else(|| not_found(&plan.subject))?;
    let human = output::render_connector_read(&connector);
    output::write_output(
        plan.json,
        &RegistryEnvelope {
            status: "success",
            registry: RegistryPayload::ConnectorRead {
                source: "remote",
                connector: Box::new(connector),
            },
        },
        || human,
    )
}

fn install(
    plan: RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let (connector_ref, version) = subject(&plan)?;
    let registry_url = hosted_registry(&plan, env, cwd, "install")?;
    let source_authority = RegistryTarget::Remote {
        registry_url: registry_url.clone(),
    }
    .manifest_source_authority();
    let client = remote_registry_client(&registry_url, env)?;
    let version = match version {
        Some(version) => version,
        None => {
            client
                .read_connector(&connector_ref, None)?
                .ok_or_else(|| not_found(&plan.subject))?
                .version
        }
    };
    let published = client
        .connector_package(&connector_ref, &version)?
        .ok_or_else(|| not_found(&format!("{connector_ref}@{version}")))?;
    if published.connector_ref != connector_ref || published.version != version {
        return Err(internal_error(format!(
            "the registry served {}@{} for {connector_ref}@{version}",
            published.connector_ref, published.version
        )));
    }
    let installed = install_registry_connector(
        &published,
        &InstallRegistryConnectorOptions {
            connectors_root: runx_runtime::RunxPaths::resolve(env, cwd).installed_connectors(),
            expected_content_digest: plan.expected_digest.clone(),
            trusted_manifest_keys: trusted_manifest_keys_from_env_for_source(
                env,
                source_authority.clone(),
            )?,
            manifest_source_authority: source_authority,
        },
    )
    .map_err(|error| internal_error(format!("registry connector install: {error}")))?;
    let human = output::render_connector_install(&installed);
    output::write_output(
        plan.json,
        &RegistryEnvelope {
            status: "success",
            registry: RegistryPayload::ConnectorInstall {
                source: "remote",
                install: Box::new(installed),
            },
        },
        || human,
    )
}

/// The ref the plan names, with the version from `@` or `--version`.
fn subject(plan: &RegistryPlan) -> Result<(ConnectorRef, Option<String>), RegistryCliError> {
    let (connector_ref, named) = parse_connector_subject(&plan.subject).map_err(usage_error)?;
    match (named, plan.version.clone()) {
        (Some(named), Some(flag)) if named != flag => Err(usage_error(format!(
            "{} names version {named} but --version is {flag}",
            plan.subject
        ))),
        (named, flag) => Ok((connector_ref, named.or(flag))),
    }
}

/// Connectors are published only to a hosted registry.
fn hosted_registry(
    plan: &RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    command: &str,
) -> Result<String, RegistryCliError> {
    match resolve_registry_target(plan, env, cwd) {
        RegistryTarget::Remote { registry_url } => Ok(registry_url),
        RegistryTarget::Local { .. } => Err(usage_error(format!(
            "runx registry connector {command} needs a hosted registry: pass --registry https://api.runx.ai or set RUNX_REGISTRY_URL"
        ))),
    }
}

fn not_found(subject: &str) -> RegistryCliError {
    internal_error(format!("registry connector not found: {subject}"))
}
