use std::collections::BTreeMap;
use std::path::Path;

use runx_runtime::registry::{
    HostedAdminSkillPublishRequest, HostedSkillPublishRequest, HostedSkillPublishResult,
    RegistryPublishHarnessReport, RegistryPublishPackage, publish_hosted_admin_skill,
    publish_hosted_skill,
};

use super::{RegistryCliError, RegistryPlan, internal_error, usage_error};

pub(super) fn publish_remote_skill_package(
    registry_url: &str,
    plan: &RegistryPlan,
    package: &RegistryPublishPackage,
    harness: &RegistryPublishHarnessReport,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<HostedSkillPublishResult, RegistryCliError> {
    if plan.trust_tier.is_some() {
        return Err(usage_error(
            "remote registry publish derives trust from hosted verification; --trust-tier is local-only",
        ));
    }
    // An operator holding the registry's publish credential publishes into
    // the namespace it names; everyone else publishes as their login.
    if let Some(token) = admin_publish_token(env) {
        let owner = plan.publisher.clone().ok_or_else(|| {
            usage_error(
                "an operator publish names the namespace it publishes into with --publisher",
            )
        })?;
        let transport = runx_runtime::hosted_api_transport(
            runx_runtime::hosted_private_network_allowed(false, env),
        )
        .map_err(|error| internal_error(error.to_string()))?;
        return publish_hosted_admin_skill(
            &transport,
            registry_url,
            &token,
            &HostedAdminSkillPublishRequest {
                owner: &owner,
                markdown: package.markdown(),
                profile_document: package.profile_document(),
                version: plan.version.as_deref(),
                package_files: package.package_files(),
                harness,
            },
        )
        .map_err(|error| internal_error(error.to_string()));
    }
    let environment =
        runx_runtime::HostedApiEnvironment::resolve_publish(Some(registry_url), env, cwd)
            .map_err(|error| usage_error(error.to_string()))?;
    let transport = runx_runtime::hosted_api_transport(
        runx_runtime::hosted_private_network_allowed(false, env),
    )
    .map_err(|error| internal_error(error.to_string()))?;
    let authenticated = environment
        .authenticate(&transport)
        .map_err(|error| usage_error(error.to_string()))?;
    publish_hosted_skill(
        &transport,
        authenticated.base_url(),
        authenticated.token(),
        &HostedSkillPublishRequest {
            markdown: package.markdown(),
            profile_document: package.profile_document(),
            version: plan.version.as_deref(),
            publisher: plan.publisher.as_deref(),
            package_files: package.package_files(),
        },
    )
    .map_err(|error| internal_error(error.to_string()))
}

fn admin_publish_token(env: &BTreeMap<String, String>) -> Option<String> {
    [
        "RUNX_HOSTED_REGISTRY_PUBLISH_TOKEN",
        "RUNX_HOSTED_API_ADMIN_TOKEN",
    ]
    .iter()
    .find_map(|name| {
        env.get(*name)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}
