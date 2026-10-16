//! The connector half of `runx registry package|publish|install`. Packaging
//! reads a connector directory with the runtime's own loader. Publish and
//! install use a hosted registry, the only registry that holds published
//! connectors; install verifies the version and writes it to the project's
//! connectors (`RunxPaths::connectors()`).

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::ConnectorName;
use runx_runtime::registry::resolve::install_connector;
use runx_runtime::registry::{
    HostedConnectorPublishRequest, prepare_registry_connector_package, publish_hosted_connector,
};

use super::output::{self, RegistryEnvelope, RegistryPayload};
use super::{
    RegistryCliError, RegistryCliOutput, RegistryPlan, RegistryTarget, internal_error, usage_error,
};

// Function rationale: a standalone connector's package is the exact artifact
// its publisher submits, read by the same loader the runtime uses.
pub(super) fn package(
    plan: &RegistryPlan,
    directory: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let connector = prepare_registry_connector_package(directory)
        .map_err(|error| internal_error(error.to_string()))?;
    output::write_output(
        plan.json,
        &RegistryEnvelope {
            status: "success",
            registry: RegistryPayload::ConnectorPackage {
                connector: Box::new(connector),
            },
        },
        || "\n  registry package ready\n\n".to_owned(),
    )
}

pub(super) fn publish(
    plan: &RegistryPlan,
    directory: &Path,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let package = prepare_registry_connector_package(directory)
        .map_err(|error| internal_error(error.to_string()))?;
    let registry_url = hosted_registry(plan, env, cwd)?;
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
    if published.name != package.name || published.content_digest != package.content_digest {
        return Err(internal_error(format!(
            "the registry recorded {} with content {}, but the package is {} with content {}",
            published.name, published.content_digest, package.name, package.content_digest
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

/// Installs connector `name` at `version`, or its current version.
pub(super) fn install(
    plan: &RegistryPlan,
    name: &ConnectorName,
    registry_url: &str,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryCliOutput, RegistryCliError> {
    let installed = install_connector(
        name,
        plan.version.as_deref(),
        plan.expected_digest.as_deref(),
        registry_url,
        env,
        cwd,
    )?
    .ok_or_else(|| not_found(&plan.requested_ref()))?;
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

/// Connectors are published only to a hosted registry.
fn hosted_registry(
    plan: &RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<String, RegistryCliError> {
    match plan.target(env, cwd) {
        RegistryTarget::Remote { registry_url } => Ok(registry_url),
        RegistryTarget::Local { .. } => Err(usage_error(
            "a connector publishes only to a hosted registry: pass --registry https://api.runx.ai or set RUNX_REGISTRY_URL",
        )),
    }
}

/// Neither kind is published under the name and version.
pub(super) fn not_found(registry_ref: &str) -> RegistryCliError {
    internal_error(format!(
        "nothing is published as {registry_ref}: no skill or connector has that name and version"
    ))
}
