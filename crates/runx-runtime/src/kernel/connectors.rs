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
mod tools;

pub use tools::ConnectorTool;

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// A validated connector with its digests, computed once.
#[derive(Debug, PartialEq)]
pub struct LoadedConnector {
    pub manifest: ConnectorManifest,
    pub manifest_digest: Digest,
    pub operation_digests: BTreeMap<OperationName, Digest>,
    pub origin: ConnectorOrigin,
    /// The tool packages its process operations run, by tool name.
    pub tools: BTreeMap<String, ConnectorTool>,
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
    #[error("connector {id} is not an official connector")]
    NotOfficial { id: String },
    #[error(
        "connector {id} is neither an official connector nor one of the plugin at {}",
        root.display()
    )]
    NotInPlugin { id: String, root: PathBuf },
    #[error(
        "connector {id} is not an official connector, and {} is in no plugin; a plugin's connectors are reached from inside the plugin, at or below the directory holding its connectors/",
        dir.display()
    )]
    NoPlugin { id: String, dir: PathBuf },
    #[error("{} cannot be inspected for a plugin: {message}", dir.display())]
    Uninspectable { dir: PathBuf, message: String },
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
        let embedded = official::TOOLS
            .iter()
            .filter(|(connector, _, _)| connector == id)
            .map(|(_, path, contents)| (*path, *contents))
            .collect::<Vec<_>>();
        let tools = process_tools(&validated.manifest)
            .map(|tool| {
                ConnectorTool::official(&embedded, tool).map(|loaded| (tool.to_owned(), loaded))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|message| invalid(id, message))?;
        connectors.insert(
            validated.manifest.id.clone(),
            Arc::new(loaded(validated, ConnectorOrigin::Official, tools)),
        );
    }
    Ok(Official { connectors })
}

fn loaded(
    validated: ValidatedConnector,
    origin: ConnectorOrigin,
    tools: BTreeMap<String, ConnectorTool>,
) -> LoadedConnector {
    LoadedConnector {
        manifest: validated.manifest,
        manifest_digest: validated.manifest_digest,
        operation_digests: validated.operation_digests,
        origin,
        tools,
    }
}

/// Every tool the manifest's process operations name, once each.
fn process_tools(manifest: &ConnectorManifest) -> impl Iterator<Item = &str> {
    manifest
        .operations
        .values()
        .filter_map(|operation| match &operation.implementation {
            Some(runx_contracts::connector::Implementation::Process(process)) => {
                Some(process.tool.as_str())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
}

/// The official connectors as runx Connect reads them: each manifest under
/// `connectors_dir`, validated, beside its manifest digest and each
/// operation's operation and implementation digests, the tool packages its
/// process operations run read the way the runtime reads a plugin's.
/// `scripts/generate-official-connectors.mjs` writes this into
/// `@runxhq/contracts`; digests are computed in Rust only.
pub fn official_connectors_contract(connectors_dir: &Path) -> Result<serde_json::Value, String> {
    let mut ids = std::fs::read_dir(connectors_dir)
        .map_err(|error| format!("reading {}: {error}", connectors_dir.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    ids.sort();
    let mut connectors = Vec::with_capacity(ids.len());
    for id in ids {
        let directory = connectors_dir.join(&id);
        let source = read_bounded(&directory.join("connector.yaml"))?;
        let validated = parse_connector_manifest(&source)
            .map_err(|error| error.to_string())
            .and_then(|manifest| {
                validate_connector_manifest(manifest).map_err(|errors| format!("{errors:?}"))
            })
            .map_err(|message| format!("connectors/{id}: {message}"))?;
        let manifest = &validated.manifest;
        let server = manifest
            .placements
            .local
            .as_ref()
            .and_then(|local| local.mcp_server.as_ref());
        let tools = process_tools(manifest)
            .map(|tool| {
                ConnectorTool::in_plugin(&directory, tool).map(|loaded| (tool, loaded.digest))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|message| format!("connectors/{id}: {message}"))?;
        let mut digests = serde_json::Map::new();
        for (name, operation) in &manifest.operations {
            let mut entry = serde_json::Map::from_iter([(
                "operation".to_owned(),
                serde_json::Value::from(validated.operation_digests[name].as_str()),
            )]);
            if let Some(implementation) = &operation.implementation {
                let tool = match implementation {
                    runx_contracts::connector::Implementation::Process(process) => {
                        tools.get(process.tool.as_str())
                    }
                    _ => None,
                };
                let digest =
                    runx_contracts::connector::implementation_digest(implementation, server, tool)
                        .map_err(|error| error.to_string())?;
                entry.insert(
                    "implementation".to_owned(),
                    serde_json::Value::from(digest.as_str()),
                );
            }
            digests.insert(name.as_str().to_owned(), serde_json::Value::Object(entry));
        }
        connectors.push(serde_json::json!({
            "manifest_digest": validated.manifest_digest.as_str(),
            "digests": digests,
            "manifest": serde_json::to_value(manifest).map_err(|error| error.to_string())?,
        }));
    }
    Ok(serde_json::Value::Array(connectors))
}

/// The connectors one skill may use: official, plus its plugin's.
#[derive(Clone)]
pub struct ConnectorScope {
    plugin: Option<Arc<PluginConnectors>>,
    /// The directory the plugin was looked for from, so a miss says where.
    /// None for the official connectors alone.
    searched: Option<PathBuf>,
}

struct PluginConnectors {
    root: PathBuf,
    connectors: BTreeMap<ConnectorId, Arc<LoadedConnector>>,
}

impl ConnectorScope {
    /// The scope of the skill in `skill_dir`.
    pub fn for_skill(skill_dir: &Path) -> Result<Self, ConnectorRegistryError> {
        official()?;
        let skill_dir = inspected(skill_dir)?;
        let plugin = match plugin_root(&skill_dir) {
            Some(root) => Some(plugin_connectors(&root)?),
            None => None,
        };
        Ok(Self {
            plugin,
            searched: Some(skill_dir),
        })
    }

    /// The scope of an operator working in `dir`: official, plus the
    /// connectors of the plugin `dir` is in, the nearest directory at or
    /// above it holding a `connectors` directory.
    pub fn for_workspace(dir: &Path) -> Result<Self, ConnectorRegistryError> {
        official()?;
        let dir = inspected(dir)?;
        let root = dir
            .ancestors()
            .find(|ancestor| ancestor.join("connectors").is_dir())
            .map(Path::to_path_buf);
        let plugin = match root {
            Some(root) => Some(plugin_connectors(&root)?),
            None => None,
        };
        Ok(Self {
            plugin,
            searched: Some(dir),
        })
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
        Ok(Self {
            plugin: None,
            searched: None,
        })
    }

    pub fn connector(&self, id: &str) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        if let Some(connector) = official()?.connectors.get(id) {
            return Ok(Arc::clone(connector));
        }
        let id = id.to_owned();
        match (&self.plugin, &self.searched) {
            (Some(plugin), _) => plugin.connectors.get(id.as_str()).map(Arc::clone).ok_or(
                ConnectorRegistryError::NotInPlugin {
                    id,
                    root: plugin.root.clone(),
                },
            ),
            (None, Some(dir)) => Err(ConnectorRegistryError::NoPlugin {
                id,
                dir: dir.clone(),
            }),
            (None, None) => Err(ConnectorRegistryError::NotOfficial { id }),
        }
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

/// `dir` as the filesystem resolves it.
fn inspected(dir: &Path) -> Result<PathBuf, ConnectorRegistryError> {
    dir.canonicalize()
        .map_err(|error| ConnectorRegistryError::Uninspectable {
            dir: dir.to_path_buf(),
            message: error.to_string(),
        })
}

/// The nearest ancestor of a resolved `skill_dir` named `skills`, whose
/// parent is the plugin.
fn plugin_root(skill_dir: &Path) -> Option<PathBuf> {
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
    // A plugin without connectors/ has none; one whose connectors cannot be
    // read is an error, never a plugin without connectors.
    let uninspectable = |error: std::io::Error| ConnectorRegistryError::Uninspectable {
        dir: directory.clone(),
        message: error.to_string(),
    };
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PluginConnectors {
                root: root.to_path_buf(),
                connectors,
            });
        }
        Err(error) => return Err(uninspectable(error)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.map_err(uninspectable)?.path().join("connector.yaml");
        if path.try_exists().map_err(uninspectable)? {
            paths.push(path);
        }
    }
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
        let directory = path.parent().unwrap_or(root);
        let tools = process_tools(&validated.manifest)
            .map(|tool| {
                ConnectorTool::in_plugin(directory, tool).map(|loaded| (tool.to_owned(), loaded))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(&invalid)?;
        connectors.insert(
            validated.manifest.id.clone(),
            Arc::new(loaded(
                validated,
                ConnectorOrigin::Plugin {
                    root: root.to_path_buf(),
                },
                tools,
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
