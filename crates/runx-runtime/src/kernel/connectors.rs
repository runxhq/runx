//! The connector registry: where a requirement's connector comes from.
//!
//! Official connectors are embedded in the binary and validated once per
//! process. A skill also sees the connectors of the plugin that holds it,
//! `<plugin root>/connectors/<id>/connector.yaml`. A bundled package, one under
//! a registry package bundle descriptor, is its own plugin root; in a source
//! tree the plugin root is the nearest directory at or above the skill holding
//! `connectors/`. Within one resolution an id names exactly one manifest, in
//! the directory named by its id: a plugin connector with an official id is
//! refused unless it is the same manifest, and a bundled skill never sees its
//! composer's plugin connectors.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use runx_contracts::connector::{
    ConnectorId, ConnectorManifest, ConnectorOperation, ConnectorRef, Digest, OperationName,
    PublisherNamespace, connector_digest,
};
use runx_parser::{ValidatedConnector, parse_connector_manifest, validate_connector_manifest};
use thiserror::Error;

mod official;
pub mod references;
mod tools;

pub use tools::ConnectorTool;

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// The manifest file of a connector directory.
pub(crate) const MANIFEST_FILE: &str = "connector.yaml";

/// A validated connector with its digests, computed once.
#[derive(Debug, PartialEq)]
pub struct LoadedConnector {
    pub connector_ref: ConnectorRef,
    pub manifest: ConnectorManifest,
    pub manifest_digest: Digest,
    pub connector_digest: Digest,
    pub operation_digests: BTreeMap<OperationName, Digest>,
    pub origin: ConnectorOrigin,
    /// The tool packages its process operations run, by tool name.
    pub tools: BTreeMap<String, ConnectorTool>,
}

impl LoadedConnector {
    #[must_use]
    pub fn binding(&self) -> runx_contracts::connector::ConnectorBinding {
        runx_contracts::connector::ConnectorBinding {
            connector: self.connector_ref.clone(),
            manifest_digest: self.manifest_digest.clone(),
            connector_digest: self.connector_digest.clone(),
        }
    }
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
    Plugin {
        root: PathBuf,
    },
    /// A version installed from a registry into the runx home: its exact
    /// package, read from `directory`.
    Installed {
        directory: PathBuf,
    },
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
    #[error("connector {id} is not available in this scope")]
    Unavailable { id: String },
    #[error("connector {id} names more than one publisher; use its qualified reference")]
    Ambiguous { id: String },
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
    #[error("connector {connector} does not match the required manifest digest")]
    RequiredManifest { connector: ConnectorRef },
    #[error(
        "connector {connector} at manifest {manifest_digest} is not installed; install the version that publishes it with `runx registry connector install {connector}@<version>`"
    )]
    NotInstalled {
        connector: ConnectorRef,
        manifest_digest: Digest,
    },
    #[error(
        "connector {connector} has more than one installed version{}; name one as {connector}@<version>",
        if versions.is_empty() { String::new() } else { format!(" ({})", versions.join(", ")) }
    )]
    InstalledAmbiguous {
        connector: ConnectorRef,
        versions: Vec<String>,
    },
    #[error("connector {connector} has no installed version {version}")]
    VersionNotInstalled {
        connector: ConnectorRef,
        version: String,
    },
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
    let namespace = PublisherNamespace::new("runx")
        .ok_or_else(|| invalid("runx", "invalid embedded namespace".to_owned()))?;
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
            Arc::new(
                loaded(&namespace, validated, ConnectorOrigin::Official, tools)
                    .map_err(|message| invalid(id, message))?,
            ),
        );
    }
    Ok(Official { connectors })
}

fn loaded(
    namespace: &PublisherNamespace,
    validated: ValidatedConnector,
    origin: ConnectorOrigin,
    tools: BTreeMap<String, ConnectorTool>,
) -> Result<LoadedConnector, String> {
    let connector_digest = connector_digest(
        &validated.manifest_digest,
        tools
            .iter()
            .map(|(name, tool)| (name.as_str(), &tool.digest)),
    )
    .map_err(|error| error.to_string())?;
    Ok(LoadedConnector {
        connector_ref: ConnectorRef::qualified(namespace, &validated.manifest.id),
        manifest: validated.manifest,
        manifest_digest: validated.manifest_digest,
        connector_digest,
        operation_digests: validated.operation_digests,
        origin,
        tools,
    })
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

/// One operation's digests, in the shape runx Connect binds: its operation
/// digest and, when it declares an implementation, that implementation's.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OperationDigests {
    pub operation: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<Digest>,
}

/// Every operation's digests: the validated operation digest, and the
/// implementation digest over its local MCP server or the digest of the tool
/// package its process operation runs (`tool_digest`).
pub(crate) fn operation_digest_entries<'a>(
    manifest: &ConnectorManifest,
    operation_digests: &BTreeMap<OperationName, Digest>,
    tool_digest: impl Fn(&str) -> Option<&'a Digest>,
) -> Result<BTreeMap<OperationName, OperationDigests>, String> {
    let server = manifest
        .placements
        .local
        .as_ref()
        .and_then(|local| local.mcp_server.as_ref());
    manifest
        .operations
        .iter()
        .map(|(name, operation)| {
            let implementation = operation
                .implementation
                .as_ref()
                .map(|implementation| {
                    let tool = match implementation {
                        runx_contracts::connector::Implementation::Process(process) => {
                            tool_digest(process.tool.as_str())
                        }
                        _ => None,
                    };
                    runx_contracts::connector::implementation_digest(implementation, server, tool)
                        .map_err(|error| error.to_string())
                })
                .transpose()?;
            let operation = operation_digests
                .get(name)
                .cloned()
                .ok_or_else(|| format!("operation {name} has no operation digest"))?;
            Ok((
                name.clone(),
                OperationDigests {
                    operation,
                    implementation,
                },
            ))
        })
        .collect()
}

/// The official connectors as runx Connect reads them: each manifest under
/// `connectors_dir`, validated, beside its manifest digest and each
/// operation's operation and implementation digests, the tool packages its
/// process operations run read the way the runtime reads a plugin's.
/// `scripts/generate-official-connectors.mjs` writes this into
/// `@runxhq/contracts`; digests are computed in Rust only.
pub fn official_connectors_contract(connectors_dir: &Path) -> Result<serde_json::Value, String> {
    let namespace = plugin_namespace(
        connectors_dir
            .parent()
            .ok_or("connector source has no plugin root")?,
    )?;
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
        let tools = process_tools(manifest)
            .map(|tool| {
                ConnectorTool::in_plugin(&directory, tool).map(|loaded| (tool, loaded.digest))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|message| format!("connectors/{id}: {message}"))?;
        let digests = operation_digest_entries(manifest, &validated.operation_digests, |tool| {
            tools.get(tool)
        })
        .map_err(|message| format!("connectors/{id}: {message}"))?;
        connectors.push(serde_json::json!({
            "connector_binding": runx_contracts::connector::ConnectorBinding {
                connector: ConnectorRef::qualified(&namespace, &manifest.id),
                manifest_digest: validated.manifest_digest.clone(),
                connector_digest: connector_digest(&validated.manifest_digest, tools.iter().map(|(name, digest)| (*name, digest))).map_err(|error| error.to_string())?,
            },
            "manifest_digest": validated.manifest_digest.as_str(),
            "digests": serde_json::to_value(&digests).map_err(|error| error.to_string())?,
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
    /// The runx home's installed connector versions, read only for a
    /// qualified connector named exactly: by a requirement's manifest pin or
    /// an operator's `ref@version`. Never an ambient search root.
    installed: Option<PathBuf>,
}

struct PluginConnectors {
    root: PathBuf,
    isolated: bool,
    connectors: BTreeMap<ConnectorRef, Arc<LoadedConnector>>,
}

impl ConnectorScope {
    /// The connectors usable from `dir`, a skill's directory or an operator's
    /// workspace: official, plus those of the plugin `dir` is in.
    pub fn at(dir: &Path) -> Result<Self, ConnectorRegistryError> {
        official()?;
        let dir = inspected(dir)?;
        if let Some(bindings) =
            crate::registry::package_bundle::bundled_connectors(&dir).map_err(|message| {
                ConnectorRegistryError::Uninspectable {
                    dir: dir.clone(),
                    message,
                }
            })?
        {
            let root = crate::skill_package::find_owning_package_root(&dir).ok_or_else(|| {
                ConnectorRegistryError::Uninspectable {
                    dir: dir.clone(),
                    message: "bundled directory has no owning package".to_owned(),
                }
            })?;
            let mut connectors = BTreeMap::new();
            for (binding, directory) in bindings {
                let namespace =
                    PublisherNamespace::new(binding.connector.namespace()).ok_or_else(|| {
                        ConnectorRegistryError::Unavailable {
                            id: binding.connector.to_string(),
                        }
                    })?;
                let connector = load_directory(
                    &directory,
                    ConnectorOrigin::Plugin { root: root.clone() },
                    &namespace,
                    false,
                )?;
                if connector.connector_ref != binding.connector
                    || connector.manifest_digest != binding.manifest_digest
                    || connector.connector_digest != binding.connector_digest
                {
                    return Err(ConnectorRegistryError::InvalidManifest {
                        path: directory.join(MANIFEST_FILE),
                        message: "connector snapshot does not match its exact bundle binding"
                            .to_owned(),
                    });
                }
                connectors.insert(connector.connector_ref.clone(), connector);
            }
            return Ok(Self {
                plugin: Some(Arc::new(PluginConnectors {
                    root,
                    isolated: true,
                    connectors,
                })),
                searched: Some(dir),
                installed: None,
            });
        }
        let plugin = match plugin_root(&dir)? {
            Some(root) => Some(Arc::new(load_plugin(&root)?)),
            None => None,
        };
        Ok(Self {
            plugin,
            searched: Some(dir),
            installed: None,
        })
    }

    /// This scope, able to read the connector versions installed under
    /// `root` (`RunxPaths::installed_connectors`). A bundled package stays
    /// isolated: it carries every connector it binds.
    #[must_use]
    pub fn with_installed(mut self, root: PathBuf) -> Self {
        if !self.isolated() {
            self.installed = Some(root);
        }
        self
    }

    fn isolated(&self) -> bool {
        self.plugin.as_ref().is_some_and(|plugin| plugin.isolated)
    }

    /// The connector a skill's requirement keyed `key` binds. A qualified key
    /// pinned to a manifest digest that no connector in reach carries
    /// resolves to the installed version publishing that exact manifest.
    pub fn requirement(
        &self,
        key: &str,
        requirement: &runx_parser::ConnectorRequirement,
    ) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        let Some(pin) = requirement.manifest_digest.as_ref() else {
            return self.connector(key);
        };
        let reference = ConnectorRef::new(key);
        match (self.connector(key), reference) {
            (Ok(connector), _) if &connector.manifest_digest == pin => Ok(connector),
            (Ok(connector), _) if self.installed.is_none() => {
                Err(ConnectorRegistryError::RequiredManifest {
                    connector: connector.connector_ref.clone(),
                })
            }
            (Ok(connector), _) => self.installed_pinned(&connector.connector_ref, pin),
            (Err(ConnectorRegistryError::Unavailable { .. }), Some(reference))
                if self.installed.is_some() =>
            {
                self.installed_pinned(&reference, pin)
            }
            (Err(error), _) => Err(error),
        }
    }

    /// The installed version of `connector` publishing manifest `pin`.
    fn installed_pinned(
        &self,
        connector: &ConnectorRef,
        pin: &Digest,
    ) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        let mut found: Option<Arc<LoadedConnector>> = None;
        for (_, installed) in self.installed_versions(connector)? {
            if &installed.manifest_digest != pin {
                continue;
            }
            if let Some(previous) = &found
                && previous.connector_digest != installed.connector_digest
            {
                return Err(ConnectorRegistryError::InstalledAmbiguous {
                    connector: connector.clone(),
                    versions: Vec::new(),
                });
            }
            found.get_or_insert(installed);
        }
        found.ok_or_else(|| ConnectorRegistryError::NotInstalled {
            connector: connector.clone(),
            manifest_digest: pin.clone(),
        })
    }

    /// The connector an operator names on the command line: an id or
    /// qualified ref in reach, `ref@version` for an installed version, or a
    /// bare qualified ref of which exactly one version is installed.
    pub fn named(&self, name: &str) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        if let Some((reference, version)) = name.split_once('@') {
            let connector = ConnectorRef::new(reference).ok_or_else(|| {
                ConnectorRegistryError::Unavailable {
                    id: name.to_owned(),
                }
            })?;
            return self.installed(&connector, Some(version));
        }
        match (self.connector(name), ConnectorRef::new(name)) {
            (Err(ConnectorRegistryError::Unavailable { .. }), Some(reference)) => {
                self.installed(&reference, None)
            }
            (result, _) => result,
        }
    }

    /// The installed version an operator names: `ref@version`, or a bare
    /// qualified ref no connector in reach answers when exactly one version
    /// of it is installed.
    pub fn installed(
        &self,
        connector: &ConnectorRef,
        version: Option<&str>,
    ) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        let versions = self.installed_versions(connector)?;
        match version {
            Some(version) => versions
                .into_iter()
                .find(|(installed, _)| installed == version)
                .map(|(_, loaded)| loaded)
                .ok_or_else(|| ConnectorRegistryError::VersionNotInstalled {
                    connector: connector.clone(),
                    version: version.to_owned(),
                }),
            None => match <[_; 1]>::try_from(versions) {
                Ok([(_, loaded)]) => Ok(loaded),
                Err(versions) if versions.is_empty() => Err(ConnectorRegistryError::Unavailable {
                    id: connector.to_string(),
                }),
                Err(versions) => Err(ConnectorRegistryError::InstalledAmbiguous {
                    connector: connector.clone(),
                    versions: versions.into_iter().map(|(version, _)| version).collect(),
                }),
            },
        }
    }

    /// Every installed version of `connector`, by version label. Each version
    /// directory is complete or absent; a dot-entry is an install in progress.
    fn installed_versions(
        &self,
        connector: &ConnectorRef,
    ) -> Result<Vec<(String, Arc<LoadedConnector>)>, ConnectorRegistryError> {
        let Some(root) = &self.installed else {
            return Ok(Vec::new());
        };
        let directory = root.join(connector.namespace()).join(connector.id());
        let uninspectable = |error: std::io::Error| ConnectorRegistryError::Uninspectable {
            dir: directory.clone(),
            message: error.to_string(),
        };
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(uninspectable(error)),
        };
        let mut versions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(uninspectable)?;
            let Some(version) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if version.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            let loaded = load_installed(&entry.path())?;
            if &loaded.connector_ref != connector {
                return Err(ConnectorRegistryError::InvalidManifest {
                    path: entry.path().join(MANIFEST_FILE),
                    message: format!(
                        "the installed package is {}, not {connector}",
                        loaded.connector_ref
                    ),
                });
            }
            versions.push((version, loaded));
        }
        versions.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(versions)
    }

    /// Every connector in the scope, official first.
    pub fn connectors(&self) -> Result<Vec<Arc<LoadedConnector>>, ConnectorRegistryError> {
        if let Some(plugin) = self.plugin.as_ref().filter(|plugin| plugin.isolated) {
            return Ok(plugin.connectors.values().map(Arc::clone).collect());
        }
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
            installed: None,
        })
    }

    pub fn connector(&self, id: &str) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        if let Some(reference) = ConnectorRef::new(id) {
            // A bundle's exact snapshot takes precedence over the runtime's
            // current copy of the same qualified connector.
            if let Some(connector) = self
                .plugin
                .as_ref()
                .and_then(|plugin| plugin.connectors.get(&reference))
            {
                return Ok(Arc::clone(connector));
            }
            if self.plugin.as_ref().is_some_and(|plugin| plugin.isolated) {
                return Err(ConnectorRegistryError::Unavailable { id: id.to_owned() });
            }
            let found = official()?
                .connectors
                .values()
                .chain(
                    self.plugin
                        .iter()
                        .flat_map(|plugin| plugin.connectors.values()),
                )
                .find(|connector| connector.connector_ref == reference);
            return found
                .map(Arc::clone)
                .ok_or_else(|| ConnectorRegistryError::Unavailable { id: id.to_owned() });
        }
        let plugin = self
            .plugin
            .iter()
            .flat_map(|plugin| plugin.connectors.values())
            .filter(|connector| connector.manifest.id.as_str() == id)
            .collect::<Vec<_>>();
        if plugin.len() > 1
            || (plugin.first().is_some_and(|connector| {
                official()
                    .ok()
                    .and_then(|official| official.connectors.get(id))
                    .is_some_and(|official| official.connector_ref != connector.connector_ref)
            }))
        {
            return Err(ConnectorRegistryError::Ambiguous { id: id.to_owned() });
        }
        if let Some(connector) = plugin.first() {
            return Ok(Arc::clone(connector));
        }
        if self.plugin.as_ref().is_some_and(|plugin| plugin.isolated) {
            return Err(ConnectorRegistryError::Unavailable { id: id.to_owned() });
        }
        if let Some(connector) = official()?.connectors.get(id) {
            return Ok(Arc::clone(connector));
        }
        let id = id.to_owned();
        match (&self.plugin, &self.searched) {
            (Some(plugin), _) => Err(ConnectorRegistryError::NotInPlugin {
                id,
                root: plugin.root.clone(),
            }),
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

    /// Source bytes for a bound snapshot, when present on disk. Embedded
    /// connectors have their own source bytes in the runtime.
    pub(crate) fn connector_directory(&self, connector: &LoadedConnector) -> Option<PathBuf> {
        if let ConnectorOrigin::Installed { directory } = &connector.origin {
            return Some(directory.clone());
        }
        if let Some(plugin) = self.plugin.as_ref() {
            let directory = plugin.root.join("connectors").join(if plugin.isolated {
                connector.connector_ref.as_str()
            } else {
                connector.manifest.id.as_str()
            });
            if directory.join(MANIFEST_FILE).is_file() {
                return Some(directory);
            }
        }
        None
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

/// The plugin a resolved `dir` is in. A bundled package is its own plugin, so
/// a dependency never sees its composer's connectors nor anything above the
/// bundle; in a source tree it is the nearest directory at or above `dir`
/// holding `connectors/`.
pub(crate) fn plugin_root(dir: &Path) -> Result<Option<PathBuf>, ConnectorRegistryError> {
    let uninspectable = |message: String| ConnectorRegistryError::Uninspectable {
        dir: dir.to_path_buf(),
        message,
    };
    if crate::registry::package_bundle::package_bundle_root(dir)
        .map_err(uninspectable)?
        .is_some()
    {
        return crate::skill_package::find_owning_package_root(dir)
            .map(Some)
            .ok_or_else(|| {
                uninspectable("a bundled directory with no owning SKILL.md".to_owned())
            });
    }
    Ok(dir
        .ancestors()
        .find(|ancestor| ancestor.join("connectors").is_dir())
        .map(Path::to_path_buf))
}

fn load_plugin(root: &Path) -> Result<PluginConnectors, ConnectorRegistryError> {
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
                isolated: false,
                connectors,
            });
        }
        Err(error) => return Err(uninspectable(error)),
    };
    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(uninspectable)?;
        let path = entry.path();
        if path
            .join(MANIFEST_FILE)
            .try_exists()
            .map_err(uninspectable)?
        {
            directories.push((entry.file_name(), path));
        }
    }
    directories.sort();
    for (named, directory) in directories {
        let connector = load_connector_directory(&directory, root)?;
        if named.to_str() != Some(connector.manifest.id.as_str()) {
            return Err(ConnectorRegistryError::InvalidManifest {
                path: directory.join(MANIFEST_FILE),
                message: format!(
                    "connector {} lives in connectors/{}/, named by its id",
                    connector.manifest.id, connector.manifest.id
                ),
            });
        }
        // The official plugin's own source tree is the official connector.
        if connector.origin == ConnectorOrigin::Official {
            continue;
        }
        connectors.insert(connector.connector_ref.clone(), connector);
    }
    Ok(PluginConnectors {
        root: root.to_path_buf(),
        isolated: false,
        connectors,
    })
}

/// The connector whose manifest is `directory/connector.yaml`, loaded as a
/// plugin's with its plugin at `root`. An official connector's own source
/// tree resolves to the official connector; any other connector may not take
/// an official id or declare references.
pub(crate) fn load_connector_directory(
    directory: &Path,
    root: &Path,
) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
    let namespace =
        plugin_namespace(root).map_err(|message| ConnectorRegistryError::InvalidManifest {
            path: root.join(PLUGIN_METADATA_FILE),
            message,
        })?;
    load_directory(
        directory,
        ConnectorOrigin::Plugin {
            root: root.to_path_buf(),
        },
        &namespace,
        true,
    )
}

/// The connector version installed at `directory`, its own plugin root: the
/// exact package a registry published, named by the namespace it carries.
/// It may be another version of an official connector.
pub(crate) fn load_installed(
    directory: &Path,
) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
    let namespace =
        plugin_namespace(directory).map_err(|message| ConnectorRegistryError::InvalidManifest {
            path: directory.join(PLUGIN_METADATA_FILE),
            message,
        })?;
    load_directory(
        directory,
        ConnectorOrigin::Installed {
            directory: directory.to_path_buf(),
        },
        &namespace,
        false,
    )
}

fn load_directory(
    directory: &Path,
    origin: ConnectorOrigin,
    namespace: &PublisherNamespace,
    source: bool,
) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
    let path = directory.join(MANIFEST_FILE);
    let invalid = |message: String| ConnectorRegistryError::InvalidManifest {
        path: path.clone(),
        message,
    };
    let document = read_bounded(&path).map_err(&invalid)?;
    let manifest =
        parse_connector_manifest(&document).map_err(|error| invalid(error.to_string()))?;
    let validated =
        validate_connector_manifest(manifest).map_err(|errors| invalid(format!("{errors:?}")))?;
    let tools = process_tools(&validated.manifest)
        .map(|tool| {
            ConnectorTool::in_plugin(directory, tool).map(|loaded| (tool.to_owned(), loaded))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(&invalid)?;
    let connector = loaded(namespace, validated, origin, tools).map_err(&invalid)?;
    if let Some(official) = official()?.connectors.get(&connector.manifest.id)
        && official.connector_ref == connector.connector_ref
    {
        if official.connector_digest == connector.connector_digest {
            return Ok(Arc::clone(official));
        }
        if source {
            return Err(ConnectorRegistryError::Shadowed {
                id: connector.manifest.id,
                path,
            });
        }
    }
    Ok(Arc::new(connector))
}

pub(crate) fn embedded_manifest(connector: &LoadedConnector) -> Option<&'static str> {
    official::CONNECTORS
        .iter()
        .find(|(id, _)| *id == connector.manifest.id.as_str())
        .map(|(_, document)| *document)
}

pub(crate) fn embedded_asset(connector: &LoadedConnector, path: &str) -> Option<&'static [u8]> {
    official::ASSETS
        .iter()
        .find(|(id, file, _)| *id == connector.manifest.id.as_str() && *file == path)
        .map(|(_, _, document)| document.as_bytes())
}

/// Plugin metadata names a namespace only. It never grants publication or
/// execution authority; neither paths nor identity labels supply a default.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct PluginMetadata {
    pub namespace: PublisherNamespace,
}

pub(crate) const PLUGIN_METADATA_FILE: &str = "plugin.json";

pub(crate) fn plugin_namespace(root: &Path) -> Result<PublisherNamespace, String> {
    let source = read_bounded(&root.join(PLUGIN_METADATA_FILE))?;
    serde_json::from_str::<PluginMetadata>(&source)
        .map(|metadata| metadata.namespace)
        .map_err(|error| format!("plugin metadata needs a declared namespace: {error}"))
}

/// A manifest's text, refused past 256 KiB.
pub(crate) fn read_bounded(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds 256 KiB".to_owned());
    }
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
