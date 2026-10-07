//! The connector registry: where a requirement's connector comes from.
//!
//! Official connectors are embedded in the binary and validated once per
//! process. A skill also sees the connectors of the plugin that holds it,
//! `<plugin root>/connectors/<id>/connector.yaml`, where the plugin root is the
//! directory whose `skills/` contains the skill. Within one resolution an id
//! names exactly one manifest: a plugin connector with an official id is
//! refused unless it is the same manifest, and a composed skill never sees
//! its composer's plugin connectors.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use runx_contracts::connector::{
    ConnectorId, ConnectorManifest, ConnectorOperation, Digest, OperationName,
};
use runx_parser::{ValidatedConnector, parse_connector_manifest, validate_connector_manifest};
use thiserror::Error;

pub mod messages;
mod official;
pub mod references;

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// A validated connector with its digests, computed once.
#[derive(Debug, PartialEq)]
pub struct LoadedConnector {
    pub manifest: ConnectorManifest,
    pub manifest_digest: Digest,
    pub operation_digests: BTreeMap<OperationName, Digest>,
    pub origin: ConnectorOrigin,
}

impl LoadedConnector {
    /// The operation and its digest.
    #[must_use]
    pub fn operation(&self, name: &str) -> Option<(&OperationName, &ConnectorOperation, &Digest)> {
        let (name, operation) = self.manifest.operations.get_key_value(name)?;
        Some((name, operation, self.operation_digests.get(name)?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectorOrigin {
    Official,
    Plugin { root: PathBuf },
}

#[derive(Debug, Error)]
pub enum ConnectorRegistryError {
    #[error("official connector {id} is invalid: {message}")]
    InvalidOfficial { id: String, message: String },
    #[error("connector manifest {path} is invalid: {message}")]
    InvalidManifest { path: PathBuf, message: String },
    #[error("connector {id} at {path} conflicts with the official connector of the same id")]
    Shadowed { id: ConnectorId, path: PathBuf },
    #[error("connector {0} is not an official connector or a connector of this skill's plugin")]
    Unknown(String),
}

struct Official {
    connectors: BTreeMap<ConnectorId, Arc<LoadedConnector>>,
}

fn official() -> Result<&'static Official, ConnectorRegistryError> {
    static OFFICIAL: OnceLock<Result<Official, ConnectorRegistryError>> = OnceLock::new();
    OFFICIAL
        .get_or_init(load_official)
        .as_ref()
        .map_err(|error| ConnectorRegistryError::InvalidOfficial {
            id: match error {
                ConnectorRegistryError::InvalidOfficial { id, .. } => id.clone(),
                _ => String::new(),
            },
            message: error.to_string(),
        })
}

fn load_official() -> Result<Official, ConnectorRegistryError> {
    let invalid = |id: &str, message: String| ConnectorRegistryError::InvalidOfficial {
        id: id.to_owned(),
        message,
    };
    let mut connectors = BTreeMap::new();
    for (id, source) in official::CONNECTORS {
        let manifest =
            parse_connector_manifest(source).map_err(|error| invalid(id, error.to_string()))?;
        let validated = validate_connector_manifest(manifest)
            .map_err(|errors| invalid(id, format!("{errors:?}")))?;
        connectors.insert(
            validated.manifest.id.clone(),
            Arc::new(loaded(validated, ConnectorOrigin::Official)),
        );
    }
    Ok(Official { connectors })
}

fn loaded(validated: ValidatedConnector, origin: ConnectorOrigin) -> LoadedConnector {
    LoadedConnector {
        manifest: validated.manifest,
        manifest_digest: validated.manifest_digest,
        operation_digests: validated.operation_digests,
        origin,
    }
}

/// The connectors one skill may use: official, plus its plugin's.
#[derive(Clone)]
pub struct ConnectorScope {
    plugin: Option<Arc<PluginConnectors>>,
}

struct PluginConnectors {
    root: PathBuf,
    connectors: BTreeMap<ConnectorId, Arc<LoadedConnector>>,
}

impl ConnectorScope {
    /// The scope of the skill in `skill_dir`.
    pub fn for_skill(skill_dir: &Path) -> Result<Self, ConnectorRegistryError> {
        official()?;
        let plugin = match plugin_root(skill_dir) {
            Some(root) => Some(plugin_connectors(&root)?),
            None => None,
        };
        Ok(Self { plugin })
    }

    /// The scope of an operator working in `dir`: official, plus the
    /// connectors of the plugin `dir` is in, the nearest directory at or
    /// above it holding a `connectors` directory.
    pub fn for_workspace(dir: &Path) -> Result<Self, ConnectorRegistryError> {
        official()?;
        let root = dir.canonicalize().ok().and_then(|dir| {
            dir.ancestors()
                .find(|ancestor| ancestor.join("connectors").is_dir())
                .map(Path::to_path_buf)
        });
        let plugin = match root {
            Some(root) => Some(plugin_connectors(&root)?),
            None => None,
        };
        Ok(Self { plugin })
    }

    /// Every connector in the scope, official first.
    pub fn connectors(&self) -> Result<Vec<Arc<LoadedConnector>>, ConnectorRegistryError> {
        let plugin = self
            .plugin
            .iter()
            .flat_map(|plugin| plugin.connectors.values());
        Ok(official()?
            .connectors
            .values()
            .chain(plugin)
            .map(Arc::clone)
            .collect())
    }

    /// Only the official connectors.
    pub fn official_only() -> Result<Self, ConnectorRegistryError> {
        official()?;
        Ok(Self { plugin: None })
    }

    pub fn connector(&self, id: &str) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        if let Some(connector) = official()?.connectors.get(id) {
            return Ok(Arc::clone(connector));
        }
        self.plugin
            .as_ref()
            .and_then(|plugin| plugin.connectors.get(id))
            .map(Arc::clone)
            .ok_or_else(|| ConnectorRegistryError::Unknown(id.to_owned()))
    }

    /// Every official connector, for catalogs and inspection.
    pub fn official_connectors() -> Result<Vec<Arc<LoadedConnector>>, ConnectorRegistryError> {
        Ok(official()?.connectors.values().map(Arc::clone).collect())
    }

    /// The variable every connector in reach delivers a credential in.
    pub fn credential_variables(&self) -> Result<BTreeSet<String>, ConnectorRegistryError> {
        let plugin = self
            .plugin
            .iter()
            .flat_map(|plugin| plugin.connectors.values());
        Ok(official()?
            .connectors
            .values()
            .chain(plugin)
            .flat_map(|connector| connector.manifest.auth.credentials.values())
            .map(|declaration| declaration.delivery.env.as_str().to_owned())
            .collect())
    }

    #[must_use]
    pub fn plugin_root(&self) -> Option<&Path> {
        self.plugin.as_ref().map(|plugin| plugin.root.as_path())
    }
}

/// The nearest ancestor whose `skills/` directory contains `skill_dir`.
fn plugin_root(skill_dir: &Path) -> Option<PathBuf> {
    let skill_dir = skill_dir.canonicalize().ok()?;
    skill_dir
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "skills"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

fn plugin_connectors(root: &Path) -> Result<Arc<PluginConnectors>, ConnectorRegistryError> {
    static PLUGINS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<PluginConnectors>>>> = OnceLock::new();
    let plugins = PLUGINS.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(plugin) = plugins
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(root)
    {
        return Ok(Arc::clone(plugin));
    }
    let plugin = Arc::new(load_plugin(root)?);
    plugins
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(root.to_path_buf(), Arc::clone(&plugin));
    Ok(plugin)
}

fn load_plugin(root: &Path) -> Result<PluginConnectors, ConnectorRegistryError> {
    let official = official()?;
    let mut connectors = BTreeMap::new();
    let directory = root.join("connectors");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(PluginConnectors {
            root: root.to_path_buf(),
            connectors,
        });
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("connector.yaml"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let invalid = |message: String| ConnectorRegistryError::InvalidManifest {
            path: path.clone(),
            message,
        };
        let source = read_bounded(&path).map_err(&invalid)?;
        let manifest =
            parse_connector_manifest(&source).map_err(|error| invalid(error.to_string()))?;
        let validated = validate_connector_manifest(manifest)
            .map_err(|errors| invalid(format!("{errors:?}")))?;
        if let Some(official) = official.connectors.get(&validated.manifest.id) {
            // The official plugin's own source tree is the same manifest.
            if official.manifest_digest == validated.manifest_digest {
                continue;
            }
            return Err(ConnectorRegistryError::Shadowed {
                id: validated.manifest.id,
                path,
            });
        }
        // Receipts classify locators by the official connectors'
        // declarations alone, so a plugin's would never be read.
        if !validated.manifest.references.is_empty() {
            return Err(invalid(
                "a plugin connector declares no references; receipts classify locators by official connectors only"
                    .to_owned(),
            ));
        }
        connectors.insert(
            validated.manifest.id.clone(),
            Arc::new(loaded(
                validated,
                ConnectorOrigin::Plugin {
                    root: root.to_path_buf(),
                },
            )),
        );
    }
    Ok(PluginConnectors {
        root: root.to_path_buf(),
        connectors,
    })
}

fn read_bounded(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds 256 KiB".to_owned());
    }
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
