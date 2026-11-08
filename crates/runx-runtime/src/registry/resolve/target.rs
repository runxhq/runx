use std::collections::BTreeMap;

use super::ResolveError;
use crate::registry::{RegistryManifestSourceAuthority, canonical_registry_url};

/// The registry a ref resolves in: a hosted registry, named by its URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryTarget {
    pub registry_url: String,
}

impl RegistryTarget {
    /// The registry source as one URI, `remote:` and its credential-free
    /// URL; a resolution names its source by this URI's digest.
    #[must_use]
    pub fn source_uri(&self) -> String {
        format!("remote:{}", canonical_registry_url(&self.registry_url))
    }

    /// The registry as an operator reads it: `remote <url>`.
    #[must_use]
    pub fn description(&self) -> String {
        format!("remote {}", canonical_registry_url(&self.registry_url))
    }

    #[must_use]
    pub fn manifest_source_authority(&self) -> RegistryManifestSourceAuthority {
        crate::registry::registry_manifest_source_authority_from_registry_url(&self.registry_url)
    }
}

/// The registry `registry` names (`--registry`), else `RUNX_REGISTRY_URL`,
/// else the hosted catalog: the runx API this runtime talks to. A registry
/// is a hosted service, named by its URL; a private registry is another
/// hosted registry, with its own key.
///
/// # Errors
///
/// Returns a usage error when the registry named is not an `http(s)` URL.
pub fn resolve_registry_target(
    registry: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<RegistryTarget, ResolveError> {
    let (named, origin) = match (registry, env.get("RUNX_REGISTRY_URL")) {
        (Some(registry), _) => (registry, "--registry"),
        (None, Some(configured)) => (configured.as_str(), "RUNX_REGISTRY_URL"),
        (None, None) => {
            return Ok(RegistryTarget {
                registry_url: hosted_registry_url(env),
            });
        }
    };
    let named = named.trim();
    if !is_registry_url(named) {
        return Err(ResolveError::Usage(format!(
            "{origin} names a registry by its URL, http:// or https://; '{named}' is not one"
        )));
    }
    Ok(RegistryTarget {
        registry_url: named.to_owned(),
    })
}

/// The hosted catalog: the runx API this runtime talks to.
fn hosted_registry_url(env: &BTreeMap<String, String>) -> String {
    env.get(crate::HOSTED_API_BASE_URL_ENV)
        .map(|value| value.trim())
        .filter(|value| is_registry_url(value))
        .unwrap_or(crate::DEFAULT_HOSTED_API_BASE_URL)
        .to_owned()
}

fn is_registry_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(registry: Option<&str>, env: &[(&str, &str)]) -> Result<RegistryTarget, String> {
        let env = env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        resolve_registry_target(registry, &env).map_err(ResolveError::into_message)
    }

    #[test]
    fn the_hosted_catalog_serves_when_nothing_names_a_registry() -> Result<(), String> {
        assert_eq!(target(None, &[])?.registry_url, "https://api.runx.ai");
        Ok(())
    }

    #[test]
    fn the_hosted_catalog_follows_the_runtime_api_base() -> Result<(), String> {
        assert_eq!(
            target(
                None,
                &[("RUNX_PUBLIC_API_BASE_URL", "https://api.staging.runx.test")],
            )?
            .registry_url,
            "https://api.staging.runx.test"
        );
        Ok(())
    }

    #[test]
    fn a_named_registry_wins_over_the_environment() -> Result<(), String> {
        assert_eq!(
            target(
                Some("https://registry.acme.test"),
                &[("RUNX_REGISTRY_URL", "https://registry.other.test")],
            )?
            .registry_url,
            "https://registry.acme.test"
        );
        assert_eq!(
            target(
                None,
                &[("RUNX_REGISTRY_URL", "https://registry.other.test")]
            )?
            .registry_url,
            "https://registry.other.test"
        );
        Ok(())
    }

    /// A registry is a hosted service: a directory, a file URL or an empty
    /// RUNX_REGISTRY_URL names none.
    #[test]
    fn a_registry_that_is_not_a_url_is_a_usage_error() {
        for (registry, env) in [
            (Some("./registry"), &[][..]),
            (Some("file:///srv/registry"), &[][..]),
            (None, &[("RUNX_REGISTRY_URL", "")][..]),
            (None, &[("RUNX_REGISTRY_URL", "/srv/registry")][..]),
        ] {
            let named = env
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect();
            assert!(
                matches!(
                    resolve_registry_target(registry, &named),
                    Err(ResolveError::Usage(message)) if message.contains("names a registry by its URL")
                ),
                "{registry:?} {env:?}"
            );
        }
    }
}
