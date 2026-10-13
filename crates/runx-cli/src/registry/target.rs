use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_runtime::registry::{RegistryManifestSourceAuthority, canonical_registry_url};
use url::Url;

use super::RegistryPlan;

#[derive(Clone, Debug)]
pub(crate) enum RegistryTarget {
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
pub(crate) enum LocalRegistrySourceKind {
    Local,
    File,
}

impl RegistryTarget {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Remote { .. } => "remote",
            Self::Local { source_kind, .. } => match source_kind {
                LocalRegistrySourceKind::Local => "local",
                LocalRegistrySourceKind::File => "file",
            },
        }
    }

    pub(crate) fn fingerprint_source(&self) -> String {
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

    pub(crate) fn manifest_source_authority(&self) -> RegistryManifestSourceAuthority {
        match self {
            Self::Remote { registry_url } => {
                runx_runtime::registry::registry_manifest_source_authority_from_registry_url(
                    registry_url,
                )
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

pub(crate) fn resolve_registry_target(
    plan: &RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> RegistryTarget {
    let configured_registry = plan
        .registry
        .as_deref()
        .or_else(|| env.get("RUNX_REGISTRY_URL").map(String::as_str));
    if let Some(target) = explicit_registry_target(plan, env, cwd) {
        return target;
    }
    if let Some(target) = explicit_registry_dir_target(plan, configured_registry, env, cwd) {
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
    default_local_registry_target(configured_registry, env, cwd)
}

fn explicit_registry_target(
    plan: &RegistryPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<RegistryTarget> {
    let registry = plan.registry.as_ref()?;
    if is_remote_registry_url(registry) {
        return Some(RegistryTarget::Remote {
            registry_url: registry.clone(),
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

fn explicit_registry_dir_target(
    plan: &RegistryPlan,
    configured_registry: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<RegistryTarget> {
    let registry_dir = plan.registry_dir.as_ref()?;
    Some(local_registry_target(
        super::resolve_path(registry_dir, env, cwd, false),
        remote_registry_url(configured_registry),
    ))
}

fn env_registry_dir_target(
    configured_registry: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<RegistryTarget> {
    let paths = runx_runtime::RunxPaths::resolve(env, cwd);
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
        runx_runtime::RunxPaths::resolve(env, cwd).registry_cache(),
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

pub(crate) fn registry_source_description(target: &RegistryTarget) -> String {
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
    runx_runtime::RunxPaths::resolve(env, cwd).in_workspace(value)
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
