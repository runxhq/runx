use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use url::Url;

use crate::registry::{RegistryManifestSourceAuthority, canonical_registry_url};

/// The registry a ref resolves in: a hosted catalog, or a directory.
#[derive(Clone, Debug)]
pub enum RegistryTarget {
    Remote {
        registry_url: String,
    },
    Local {
        registry_path: PathBuf,
        registry_url: Option<String>,
        source_kind: LocalRegistrySourceKind,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum LocalRegistrySourceKind {
    Local,
    File,
}

impl RegistryTarget {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Remote { .. } => "remote",
            Self::Local { source_kind, .. } => match source_kind {
                LocalRegistrySourceKind::Local => "local",
                LocalRegistrySourceKind::File => "file",
            },
        }
    }

    /// The registry source as one URI, `remote:`, `local:` or `file:` and
    /// where; a resolution names its source by this URI's digest.
    #[must_use]
    pub fn source_uri(&self) -> String {
        match self {
            Self::Remote { registry_url } => {
                format!("remote:{}", canonical_registry_url(registry_url))
            }
            Self::Local {
                registry_path,
                source_kind,
                ..
            } => {
                let absolute =
                    fs::canonicalize(registry_path).unwrap_or_else(|_| registry_path.to_path_buf());
                match source_kind {
                    LocalRegistrySourceKind::Local => format!("local:{}", absolute.display()),
                    LocalRegistrySourceKind::File => format!("file:{}", absolute.display()),
                }
            }
        }
    }

    #[must_use]
    pub fn manifest_source_authority(&self) -> RegistryManifestSourceAuthority {
        match self {
            Self::Remote { registry_url } => {
                crate::registry::registry_manifest_source_authority_from_registry_url(registry_url)
            }
            Self::Local {
                registry_path,
                source_kind,
                ..
            } => {
                let absolute =
                    fs::canonicalize(registry_path).unwrap_or_else(|_| registry_path.to_path_buf());
                RegistryManifestSourceAuthority::RegistrySource(match source_kind {
                    LocalRegistrySourceKind::Local => format!("local:{}", absolute.display()),
                    LocalRegistrySourceKind::File => format!("file:{}", absolute.display()),
                })
            }
        }
    }
}

/// The registry `registry` names (`--registry`), else the environment's,
/// else the hosted catalog.
#[must_use]
pub fn resolve_registry_target(
    registry: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> RegistryTarget {
    let configured_registry = registry.or_else(|| env.get("RUNX_REGISTRY_URL").map(String::as_str));
    if let Some(target) = explicit_registry_target(registry, env, cwd) {
        return target;
    }
    if let Some(target) = env_registry_dir_target(configured_registry, env, cwd) {
        return target;
    }
    if let Some(registry) = configured_registry.filter(|value| is_remote_registry_url(value)) {
        return RegistryTarget::Remote {
            registry_url: registry.to_owned(),
        };
    }
    // Nothing names a registry, so the hosted catalog serves it: a skill or
    // connector arrives with no setup, and a publish goes where others fetch
    // it, as the signed-in publisher. A local registry is always named, and an
    // empty RUNX_REGISTRY_URL names none and keeps it.
    if configured_registry.is_none() {
        return RegistryTarget::Remote {
            registry_url: hosted_registry_url(env),
        };
    }
    default_local_registry_target(configured_registry, env, cwd)
}

/// The hosted catalog: the runx API this runtime talks to.
fn hosted_registry_url(env: &BTreeMap<String, String>) -> String {
    env.get(crate::HOSTED_API_BASE_URL_ENV)
        .map(|value| value.trim())
        .filter(|value| is_remote_registry_url(value))
        .unwrap_or(crate::DEFAULT_HOSTED_API_BASE_URL)
        .to_owned()
}

fn explicit_registry_target(
    registry: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<RegistryTarget> {
    let registry = registry?;
    if is_remote_registry_url(registry) {
        return Some(RegistryTarget::Remote {
            registry_url: registry.to_owned(),
        });
    }
    Some(RegistryTarget::Local {
        registry_path: registry_path_from_value(registry, env, cwd),
        registry_url: env
            .get("RUNX_REGISTRY_URL")
            .filter(|value| is_remote_registry_url(value))
            .cloned(),
        source_kind: registry_source_kind(registry),
    })
}

fn env_registry_dir_target(
    configured_registry: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<RegistryTarget> {
    let paths = crate::RunxPaths::resolve(env, cwd);
    Some(local_registry_target(
        paths.registry_dir()?.to_path_buf(),
        remote_registry_url(configured_registry),
    ))
}

fn default_local_registry_target(
    configured_registry: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> RegistryTarget {
    local_registry_target(
        crate::RunxPaths::resolve(env, cwd).registry_cache(),
        configured_registry,
    )
}

fn local_registry_target(
    registry_path: PathBuf,
    configured_registry: Option<&str>,
) -> RegistryTarget {
    RegistryTarget::Local {
        registry_path,
        registry_url: configured_registry.map(ToOwned::to_owned),
        source_kind: LocalRegistrySourceKind::Local,
    }
}

#[must_use]
pub fn registry_source_description(target: &RegistryTarget) -> String {
    match target {
        RegistryTarget::Remote { registry_url } => {
            format!("remote {}", canonical_registry_url(registry_url))
        }
        RegistryTarget::Local {
            registry_path,
            source_kind,
            ..
        } => match source_kind {
            LocalRegistrySourceKind::Local => format!("local {}", registry_path.display()),
            LocalRegistrySourceKind::File => format!("file {}", registry_path.display()),
        },
    }
}

fn registry_path_from_value(value: &str, env: &BTreeMap<String, String>, cwd: &Path) -> PathBuf {
    if let Some(path) = file_url_path(value) {
        return path;
    }
    crate::RunxPaths::resolve(env, cwd).in_workspace(value)
}

fn registry_source_kind(registry: &str) -> LocalRegistrySourceKind {
    if file_url_path(registry).is_some() {
        LocalRegistrySourceKind::File
    } else {
        LocalRegistrySourceKind::Local
    }
}

fn remote_registry_url(value: Option<&str>) -> Option<&str> {
    value.filter(|entry| is_remote_registry_url(entry))
}

fn file_url_path(value: &str) -> Option<PathBuf> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

fn is_remote_registry_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(registry: Option<&str>, env: &[(&str, &str)]) -> RegistryTarget {
        let env = env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .chain([("RUNX_HOME".to_owned(), "/home/runx".to_owned())])
            .collect();
        resolve_registry_target(registry, &env, Path::new("/work"))
    }

    fn remote_url(target: &RegistryTarget) -> Option<&str> {
        match target {
            RegistryTarget::Remote { registry_url } => Some(registry_url),
            RegistryTarget::Local { .. } => None,
        }
    }

    #[test]
    fn the_hosted_catalog_serves_when_nothing_names_a_registry() {
        assert_eq!(remote_url(&target(None, &[])), Some("https://api.runx.ai"));
    }

    #[test]
    fn the_hosted_catalog_follows_the_runtime_api_base() {
        assert_eq!(
            remote_url(&target(
                None,
                &[("RUNX_PUBLIC_API_BASE_URL", "https://api.staging.runx.test")],
            )),
            Some("https://api.staging.runx.test")
        );
    }

    #[test]
    fn a_named_local_registry_or_an_empty_registry_url_stays_local() {
        let local = [
            target(Some("./registry"), &[]),
            target(None, &[("RUNX_REGISTRY_DIR", "/srv/registry")]),
            target(None, &[("RUNX_REGISTRY_URL", "")]),
        ];
        for target in &local {
            assert!(remote_url(target).is_none(), "{target:?}");
        }
    }
}
