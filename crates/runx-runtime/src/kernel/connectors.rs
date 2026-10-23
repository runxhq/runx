//! The connector registry: where a requirement's connector comes from.
//!
//! A connector has one name, `stripe` or `google/calendar`, and it resolves
//! one way: a runtime capability, then the skill's own plugin, then a version
//! runx added to the project. The runtime capability connectors, part of runx
//! itself, are embedded in the binary and validated once per process. A skill
//! also sees the connectors of the plugin that holds it,
//! `<plugin root>/connectors/<name>/connector.yaml`. A bundled package, one
//! under a registry package bundle descriptor, is its own plugin root and
//! carries every connector it binds; in a source tree the plugin root is the
//! nearest directory at or above the skill holding `connectors/`, and a skill
//! runx added to the project directory has the project's plugin. Any other
//! connector is an added version, at `<project>/connectors/<name>/<version>/`,
//! which a skill reaches only by pinning its manifest digest; a plugin's own
//! connector of that name answers the pin instead (core-shape §6.5). A runtime
//! capability cannot be shadowed: a plugin copy with other bytes is refused,
//! and a bundled skill never sees its composer's plugin connectors. A
//! connector a turn loads is the project's, and its calls resolve there.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use runx_contracts::connector::{
    ConnectorManifest, ConnectorName, ConnectorOperation, Digest, OperationName, connector_digest,
};
use runx_parser::{ValidatedConnector, parse_connector_manifest, validate_connector_manifest};
use thiserror::Error;

use crate::paths::RunxPaths;

pub mod references;
mod runtime_capabilities;
mod tools;

pub use tools::ConnectorTool;

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// The manifest file of a connector directory.
pub(crate) const MANIFEST_FILE: &str = "connector.yaml";

/// A validated connector with its digests, computed once.
#[derive(Debug, PartialEq)]
pub struct LoadedConnector {
    /// The connector's name, its manifest's `id`.
    pub name: ConnectorName,
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
            connector: self.name.clone(),
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
    /// Embedded in the runtime: a runtime capability connector.
    RuntimeCapability,
    Plugin {
        root: PathBuf,
    },
    /// A version runx added to the project, from a registry or seeded from
    /// the release: its exact package, read from `directory`.
    Installed {
        directory: PathBuf,
    },
}

#[derive(Debug, Error)]
pub enum ConnectorRegistryError {
    #[error("runtime capability connector {id} is invalid: {message}")]
    InvalidRuntimeCapability { id: String, message: String },
    #[error("connector manifest {path} is invalid: {message}")]
    InvalidManifest { path: PathBuf, message: String },
    #[error("connector {id} at {path} conflicts with the runtime capability of the same name")]
    Shadowed { id: ConnectorName, path: PathBuf },
    #[error("connector {id} is not a runtime capability")]
    NotRuntimeCapability { id: String },
    #[error("connector {id} is not available in this scope")]
    Unavailable { id: String },
    #[error(
        "connector {connector} is not installed; install it with `runx registry install {connector}`"
    )]
    NotInstalledConnector { connector: ConnectorName },
    #[error(
        "connector {id} is neither a runtime capability nor one of the plugin at {}; a connector outside the skill's plugin pins its manifest_digest and is installed with `runx registry install {id}`",
        root.display()
    )]
    NotInPlugin { id: String, root: PathBuf },
    #[error(
        "connector {id} is not a runtime capability, and {} is in no plugin; a plugin's connectors are reached from inside the plugin, at or below the directory holding its connectors/, and any other connector pins its manifest_digest and is installed with `runx registry install {id}`",
        dir.display()
    )]
    NoPlugin { id: String, dir: PathBuf },
    #[error("{} cannot be inspected for a plugin: {message}", dir.display())]
    Uninspectable { dir: PathBuf, message: String },
    #[error("connector {connector} does not match the required manifest digest")]
    RequiredManifest { connector: ConnectorName },
    #[error(
        "connector {connector} at manifest {manifest_digest} is not installed; install the version that publishes it with `runx registry install {connector}@<version>`"
    )]
    NotInstalled {
        connector: ConnectorName,
        manifest_digest: Digest,
    },
    #[error(
        "connector {connector} has more than one installed version{}; name one as {connector}@<version>",
        if versions.is_empty() { String::new() } else { format!(" ({})", versions.join(", ")) }
    )]
    InstalledAmbiguous {
        connector: ConnectorName,
        versions: Vec<String>,
    },
    #[error("connector {connector} has no installed version {version}")]
    VersionNotInstalled {
        connector: ConnectorName,
        version: String,
    },
    #[error(
        "{} holds connector {found}, which is neither {connector} nor a surface under it",
        path.display()
    )]
    Misplaced {
        connector: ConnectorName,
        found: ConnectorName,
        path: PathBuf,
    },
    #[error(
        "{} holds connector {namespace} and surfaces under it; a provider namespace is a connector or a parent of surfaces, never both",
        path.display()
    )]
    LeafAndParent { namespace: String, path: PathBuf },
}

struct RuntimeCapabilities {
    connectors: BTreeMap<ConnectorName, Arc<LoadedConnector>>,
}

fn runtime_capabilities() -> Result<&'static RuntimeCapabilities, ConnectorRegistryError> {
    static RUNTIME_CAPABILITIES: OnceLock<Result<RuntimeCapabilities, ConnectorRegistryError>> =
        OnceLock::new();
    RUNTIME_CAPABILITIES
        .get_or_init(load_runtime_capabilities)
        .as_ref()
        .map_err(|error| ConnectorRegistryError::InvalidRuntimeCapability {
            id: match error {
                ConnectorRegistryError::InvalidRuntimeCapability { id, .. } => id.clone(),
                _ => String::new(),
            },
            message: error.to_string(),
        })
}

fn load_runtime_capabilities() -> Result<RuntimeCapabilities, ConnectorRegistryError> {
    let invalid = |id: &str, message: String| ConnectorRegistryError::InvalidRuntimeCapability {
        id: id.to_owned(),
        message,
    };
    let mut connectors = BTreeMap::new();
    for (id, source) in runtime_capabilities::CONNECTORS {
        let manifest =
            parse_connector_manifest(source).map_err(|error| invalid(id, error.to_string()))?;
        let validated = validate_connector_manifest(manifest)
            .map_err(|errors| invalid(id, format!("{errors:?}")))?;
        if !validated.manifest.runtime_capability {
            return Err(invalid(
                id,
                "an embedded connector must declare runtime_capability: true".to_owned(),
            ));
        }
        // A runtime capability is implemented by the runtime: it ships no tool
        // package for a process operation to run.
        if let Some(tool) = process_tools(&validated.manifest).next() {
            return Err(invalid(
                id,
                format!("a runtime capability ships no tool package, yet names {tool}"),
            ));
        }
        connectors.insert(
            validated.manifest.id.clone(),
            Arc::new(
                loaded(
                    validated,
                    ConnectorOrigin::RuntimeCapability,
                    BTreeMap::new(),
                )
                .map_err(|message| invalid(id, message))?,
            ),
        );
    }
    Ok(RuntimeCapabilities { connectors })
}

fn loaded(
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
        name: validated.manifest.id.clone(),
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
        .chain(
            manifest
                .auth
                .session
                .as_ref()
                .and_then(|session| match session {
                    runx_contracts::connector::SessionAuth::HostBrowser { process } => {
                        Some(process.tool.as_str())
                    }
                    _ => None,
                }),
        )
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

/// The connectors one skill may use: the runtime capabilities, its plugin's,
/// and the project's installed versions it pins by manifest digest.
#[derive(Clone)]
pub struct ConnectorScope {
    plugin: Option<Arc<PluginConnectors>>,
    /// The directory the plugin was looked for from, so a miss says where.
    /// None for the runtime capabilities alone.
    searched: Option<PathBuf>,
    /// The connectors runx added to the project (`RunxPaths::added_connectors`),
    /// whose versions (`<name>/<version>/`) are read only for a connector named
    /// exactly: by a requirement's manifest pin, an operator's `name@version`,
    /// or an operator naming a connector of which one version is installed. A
    /// runtime capability is seeded into it and read back from it (core-shape
    /// §6.5). None for a bundle.
    installed: Option<PathBuf>,
}

/// A plugin's connectors, by name. A source plugin's are each read when
/// first reached, so a run reads only the connectors it reaches and one that
/// cannot be read fails only what reaches it; a bundle's are read and held to
/// their exact bindings when its scope is made.
struct PluginConnectors {
    root: PathBuf,
    isolated: bool,
    connectors: BTreeMap<ConnectorName, PluginConnector>,
}

/// One connector of a plugin: the directory it lives in, and what it loaded
/// as once reached.
struct PluginConnector {
    directory: PathBuf,
    loaded: OnceLock<Arc<LoadedConnector>>,
}

impl PluginConnectors {
    /// The connector the plugin holds by `name`, loaded; `None` when it holds
    /// none, or when its copy is a runtime capability's own source, which is
    /// the embedded connector.
    fn get(&self, name: &str) -> Result<Option<Arc<LoadedConnector>>, ConnectorRegistryError> {
        match self.connectors.get_key_value(name) {
            Some((name, connector)) => Ok(self.reached(connector.load(name, &self.root)?)),
            None => Ok(None),
        }
    }

    /// Every connector the plugin holds, each loaded: what needs all of them
    /// fails on any that cannot be read.
    fn all(&self) -> Result<Vec<Arc<LoadedConnector>>, ConnectorRegistryError> {
        let mut all = Vec::new();
        for (name, connector) in &self.connectors {
            all.extend(self.reached(connector.load(name, &self.root)?));
        }
        Ok(all)
    }

    /// A source plugin's copy of a runtime capability is reached as the
    /// capability itself; a bundle's snapshot of one is its own.
    fn reached(&self, connector: Arc<LoadedConnector>) -> Option<Arc<LoadedConnector>> {
        (self.isolated || connector.origin != ConnectorOrigin::RuntimeCapability)
            .then_some(connector)
    }
}

impl PluginConnector {
    /// What the connector `name` loads as from its directory, read once.
    fn load(
        &self,
        name: &ConnectorName,
        root: &Path,
    ) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        if let Some(loaded) = self.loaded.get() {
            return Ok(Arc::clone(loaded));
        }
        let connector = load_connector_directory(&self.directory, root)?;
        if &connector.name != name {
            return Err(ConnectorRegistryError::InvalidManifest {
                path: self.directory.join(MANIFEST_FILE),
                message: format!(
                    "connector {} lives in connectors/{}/, named by its name",
                    connector.name, connector.name
                ),
            });
        }
        Ok(Arc::clone(self.loaded.get_or_init(|| connector)))
    }
}

impl ConnectorScope {
    /// The connectors usable from `dir`, a skill's directory or an operator's
    /// workspace: the runtime capabilities, plus those of the plugin `dir` is in.
    pub fn at(dir: &Path) -> Result<Self, ConnectorRegistryError> {
        Self::scoped(dir, None)
    }

    /// The connectors usable from `dir` in the project `paths` resolves
    /// (core-shape §6.5): those of [`Self::at`], and the versions runx added
    /// to the project, into which it seeds the runtime capabilities it
    /// resolves. A skill runx added to the project directory has no plugin of
    /// its own: its plugin is the project's, the user's connectors. A bundled
    /// package stays isolated: it carries every connector it binds.
    pub fn in_project(dir: &Path, paths: &RunxPaths) -> Result<Self, ConnectorRegistryError> {
        let mut scope = Self::scoped(dir, Some(paths))?;
        if scope.isolated() {
            return Ok(scope);
        }
        scope.installed = Some(paths.added_connectors());
        Ok(scope)
    }

    /// The connectors an operation step of the skill at `graph_dir` resolves
    /// in: the project's for a connector a load of the turn bound, where the
    /// load found it, so a bundled session reaches what it loads; the skill's
    /// own otherwise.
    pub fn of_operation(
        target: &runx_parser::OperationStepTarget,
        graph_dir: &Path,
        paths: &RunxPaths,
    ) -> Result<Self, ConnectorRegistryError> {
        let dir = if target.loaded {
            paths.workspace()
        } else {
            graph_dir
        };
        Self::in_project(dir, paths)
    }

    /// `dir`'s runtime capabilities, and a bundle's own snapshots or the
    /// plugin `dir` is in: for a skill under `project`'s directory, the
    /// workspace's.
    fn scoped(dir: &Path, project: Option<&RunxPaths>) -> Result<Self, ConnectorRegistryError> {
        runtime_capabilities()?;
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
                let connector = load_directory(
                    &directory,
                    ConnectorOrigin::Plugin { root: root.clone() },
                    false,
                )?;
                if connector.name != binding.connector
                    || connector.manifest_digest != binding.manifest_digest
                    || connector.connector_digest != binding.connector_digest
                {
                    return Err(ConnectorRegistryError::InvalidManifest {
                        path: directory.join(MANIFEST_FILE),
                        message: "connector snapshot does not match its exact bundle binding"
                            .to_owned(),
                    });
                }
                connectors.insert(
                    connector.name.clone(),
                    PluginConnector {
                        directory,
                        loaded: OnceLock::from(connector),
                    },
                );
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
        // A skill runx added to the project directory has the project's plugin.
        let from = match project {
            Some(paths)
                if paths
                    .project()
                    .canonicalize()
                    .is_ok_and(|project| dir.starts_with(project)) =>
            {
                inspected(paths.workspace())?
            }
            _ => dir.clone(),
        };
        let plugin = match plugin_root(&from)? {
            Some(root) => Some(Arc::new(plugin_index(&root)?)),
            None => None,
        };
        Ok(Self {
            plugin,
            searched: Some(dir),
            installed: None,
        })
    }

    /// `connector`, a runtime capability, as this project holds it: the
    /// release's copy, written once among the project's added connectors and
    /// read back from there. A copy whose bytes are not the release's is
    /// refused.
    fn seeded(
        &self,
        connector: &Arc<LoadedConnector>,
    ) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        let Some(root) = &self.installed else {
            return Ok(Arc::clone(connector));
        };
        let directory =
            crate::registry::write_connector_version(root, connector, None).map_err(|message| {
                ConnectorRegistryError::Uninspectable {
                    dir: root.clone(),
                    message,
                }
            })?;
        let held = load_installed(&directory)?;
        if held.connector_digest != connector.connector_digest {
            return Err(ConnectorRegistryError::InvalidManifest {
                path: directory.join(MANIFEST_FILE),
                message: format!(
                    "this copy of {} is not the one this runx release carries; remove {} to restore it",
                    connector.name,
                    directory.display()
                ),
            });
        }
        Ok(held)
    }

    fn isolated(&self) -> bool {
        self.plugin.as_ref().is_some_and(|plugin| plugin.isolated)
    }

    /// Whether this scope is a bundled package's: it carries every connector
    /// it binds, and sees no other.
    #[must_use]
    pub fn is_bundle(&self) -> bool {
        self.isolated()
    }

    /// The connector a skill's requirement keyed `key` binds. A runtime
    /// capability or a connector of the skill's plugin needs no pin, and the
    /// plugin's answers whatever a pin names; any other connector is the
    /// version runx added publishing the exact manifest the requirement pins,
    /// and a requirement without a pin cannot reach one.
    pub fn requirement(
        &self,
        key: &str,
        requirement: &runx_parser::ConnectorRequirement,
    ) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        let Some(pin) = requirement.manifest_digest.as_ref() else {
            return self.in_reach(key);
        };
        match self.in_reach(key) {
            Ok(connector) if &connector.manifest_digest == pin => Ok(connector),
            // The project's own connector is used whatever the pin names, so a
            // local copy composes (core-shape §6.5); its instance pins its own
            // digests, so it holds no credential confirmed for another.
            Ok(connector)
                if !self.isolated()
                    && matches!(connector.origin, ConnectorOrigin::Plugin { .. }) =>
            {
                Ok(connector)
            }
            Ok(connector) if self.installed.is_none() => {
                Err(ConnectorRegistryError::RequiredManifest {
                    connector: connector.name.clone(),
                })
            }
            Ok(connector) => self.installed_pinned(&connector.name, pin),
            Err(
                ConnectorRegistryError::NotInPlugin { .. }
                | ConnectorRegistryError::NoPlugin { .. }
                | ConnectorRegistryError::NotRuntimeCapability { .. },
            ) if self.installed.is_some() => {
                let name = ConnectorName::new(key)
                    .ok_or_else(|| ConnectorRegistryError::Unavailable { id: key.to_owned() })?;
                self.installed_pinned(&name, pin)
            }
            Err(error) => Err(error),
        }
    }

    /// The installed version of `connector` publishing manifest `pin`.
    fn installed_pinned(
        &self,
        connector: &ConnectorName,
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

    /// The connector an operator names on the command line: anything
    /// [`Self::connector`] resolves, or `name@version` for an installed version.
    pub fn named(&self, name: &str) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        if let Some((reference, version)) = name.split_once('@') {
            let connector = ConnectorName::new(reference).ok_or_else(|| {
                ConnectorRegistryError::Unavailable {
                    id: name.to_owned(),
                }
            })?;
            return self.installed(&connector, Some(version));
        }
        self.connector(name)
    }

    /// The installed version named: `name@version`, or a name no connector in
    /// reach answers when exactly one version of it is installed.
    pub fn installed(
        &self,
        connector: &ConnectorName,
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
                Err(versions) if versions.is_empty() && self.installed.is_some() => {
                    Err(ConnectorRegistryError::NotInstalledConnector {
                        connector: connector.clone(),
                    })
                }
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

    /// Every installed version of `connector`, by version label: the
    /// directories under `connectors/<name>/` holding that connector. The same
    /// directory may hold the project's own copy (its `connector.yaml` and
    /// tools) and, under a parent namespace, its surfaces; neither is a
    /// version. Each version directory is complete or absent; a dot-entry is
    /// an install in progress.
    fn installed_versions(
        &self,
        connector: &ConnectorName,
    ) -> Result<Vec<(String, Arc<LoadedConnector>)>, ConnectorRegistryError> {
        let Some(root) = &self.installed else {
            return Ok(Vec::new());
        };
        let mut versions = Vec::new();
        for (version, directory) in subdirectories(&name_directory(root, connector))? {
            if !directory.join(MANIFEST_FILE).is_file() {
                continue;
            }
            let held = manifest_name(&directory)?;
            if &held == connector {
                versions.push((version, load_installed(&directory)?));
            } else if held.as_str() != format!("{connector}/{version}") {
                return Err(ConnectorRegistryError::Misplaced {
                    connector: connector.clone(),
                    found: held,
                    path: directory,
                });
            }
        }
        Ok(versions)
    }

    /// Every connector in the scope's reach without naming a version, the
    /// runtime capabilities first.
    pub fn connectors(&self) -> Result<Vec<Arc<LoadedConnector>>, ConnectorRegistryError> {
        let plugin = match &self.plugin {
            Some(plugin) => plugin.all()?,
            None => Vec::new(),
        };
        if self.isolated() {
            return Ok(plugin);
        }
        Ok(runtime_capabilities()?
            .connectors
            .values()
            .map(Arc::clone)
            .chain(plugin)
            .collect())
    }

    /// Every connector the scope can resolve: [`Self::connectors`], then each
    /// version installed into the project, once per content. What a run could
    /// bind, so credentials, secret shapes and locators are known for all of it.
    pub fn reachable(&self) -> Result<Vec<Arc<LoadedConnector>>, ConnectorRegistryError> {
        let mut connectors = self.connectors()?;
        if self.isolated() {
            return Ok(connectors);
        }
        for name in self.installed_names()? {
            for (_, installed) in self.installed_versions(&name)? {
                if !connectors.iter().any(|known| {
                    known.name == installed.name
                        && known.connector_digest == installed.connector_digest
                }) {
                    connectors.push(installed);
                }
            }
        }
        Ok(connectors)
    }

    /// Every name the project's connectors directory could hold versions of:
    /// each `<namespace>` and `<namespace>/<surface>` directory with a valid
    /// name. [`Self::installed_versions`] reads which of them hold any.
    fn installed_names(&self) -> Result<Vec<ConnectorName>, ConnectorRegistryError> {
        let Some(root) = &self.installed else {
            return Ok(Vec::new());
        };
        let mut names = Vec::new();
        for (namespace, directory) in subdirectories(root)? {
            names.extend(ConnectorName::new(namespace.as_str()));
            for (surface, _) in subdirectories(&directory)? {
                names.extend(ConnectorName::new(format!("{namespace}/{surface}")));
            }
        }
        Ok(names)
    }

    /// Only the runtime capabilities.
    pub fn runtime_capabilities_only() -> Result<Self, ConnectorRegistryError> {
        runtime_capabilities()?;
        Ok(Self {
            plugin: None,
            searched: None,
            installed: None,
        })
    }

    /// The connector `name` names: one in reach ([`Self::connectors`]), or one
    /// no connector in reach answers, its one installed version. What an
    /// operator names; a skill's requirement resolves through
    /// [`Self::requirement`].
    pub fn connector(&self, name: &str) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        match (self.in_reach(name), ConnectorName::new(name)) {
            (
                Err(
                    miss @ (ConnectorRegistryError::NotInPlugin { .. }
                    | ConnectorRegistryError::NoPlugin { .. }
                    | ConnectorRegistryError::NotRuntimeCapability { .. }),
                ),
                Some(connector),
            ) if self.installed.is_some() => match self.installed(&connector, None) {
                // Nothing installed: the miss says where it looked.
                Err(ConnectorRegistryError::NotInstalledConnector { .. }) => Err(miss),
                found => found,
            },
            (result, _) => result,
        }
    }

    /// The connector `id` names among those in reach, never an installed one:
    /// a runtime capability, then the skill's plugin. A bundle reaches only
    /// its own snapshots, a capability's included.
    fn in_reach(&self, id: &str) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
        // A plugin copy of a runtime capability loads as the capability
        // itself, or is refused as shadowing it.
        let plugin = match &self.plugin {
            Some(plugin) => plugin.get(id)?,
            None => None,
        };
        if self.isolated() {
            return plugin.ok_or_else(|| ConnectorRegistryError::Unavailable { id: id.to_owned() });
        }
        if let Some(connector) = runtime_capabilities()?.connectors.get(id) {
            return self.seeded(connector);
        }
        if let Some(connector) = plugin {
            return Ok(connector);
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
            (None, None) => Err(ConnectorRegistryError::NotRuntimeCapability { id }),
        }
    }

    /// Every runtime capability, for inspection.
    pub fn runtime_capabilities() -> Result<Vec<Arc<LoadedConnector>>, ConnectorRegistryError> {
        Ok(runtime_capabilities()?
            .connectors
            .values()
            .map(Arc::clone)
            .collect())
    }

    /// The variable every reachable connector delivers a credential in. A
    /// project install that does not load is skipped: nothing can bind it.
    pub fn credential_variables(&self) -> Result<BTreeSet<String>, ConnectorRegistryError> {
        let mut connectors = self.connectors()?;
        if !self.isolated() {
            for name in self.installed_names()? {
                connectors.extend(
                    self.installed_versions(&name)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(_, installed)| installed),
                );
            }
        }
        Ok(connectors
            .iter()
            .flat_map(|connector| connector.manifest.auth.credentials.values())
            .filter_map(|declaration| declaration.env().map(|env| env.as_str().to_owned()))
            .collect())
    }

    /// Every secret prefix a reachable connector declares for its credentials.
    pub fn credential_prefixes(&self) -> Result<BTreeSet<String>, ConnectorRegistryError> {
        Ok(self
            .reachable()?
            .iter()
            .flat_map(|connector| connector.manifest.auth.credentials.values())
            .flat_map(|declaration| declaration.fields.iter())
            .flat_map(|field| &field.prefixes)
            .map(|prefix| prefix.as_str().to_owned())
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
            let directory = name_directory(&plugin.root.join("connectors"), &connector.name);
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

/// The source plugin at `root`: each connector its `connectors/` holds, by
/// the name its manifest declares, read when first reached.
fn plugin_index(root: &Path) -> Result<PluginConnectors, ConnectorRegistryError> {
    let connectors = connector_directories(&root.join("connectors"))?
        .into_iter()
        .map(|(name, directory)| {
            let connector = PluginConnector {
                directory,
                loaded: OnceLock::new(),
            };
            (name, connector)
        })
        .collect();
    Ok(PluginConnectors {
        root: root.to_path_buf(),
        isolated: false,
        connectors,
    })
}

/// Every connector a `connectors/` directory holds, by name: a leaf at
/// `<name>/connector.yaml`, a surface at `<namespace>/<surface>/connector.yaml`.
/// What lies below a connector (its tools, its installed versions) is not a
/// connector of the directory, and a namespace holding both a connector and
/// surfaces is refused. A directory without `connectors/` holds none; one
/// that cannot be read is an error.
///
/// A manifest this runx cannot read, a broken one or one written for a newer
/// runx, is indexed where it lives and its error waits for a run that reaches
/// it, so it never fails a run that names another connector (#557).
fn connector_directories(
    directory: &Path,
) -> Result<Vec<(ConnectorName, PathBuf)>, ConnectorRegistryError> {
    let mut found = Vec::new();
    for (namespace, path) in subdirectories(directory)? {
        if path.join(MANIFEST_FILE).is_file() {
            let name = match manifest_name(&path) {
                Ok(name) => name,
                Err(error) => ConnectorName::new(namespace.as_str()).ok_or(error)?,
            };
            if name.as_str() != namespace {
                return Err(ConnectorRegistryError::InvalidManifest {
                    path: path.join(MANIFEST_FILE),
                    message: format!(
                        "connector {name} lives in connectors/{name}/, named by its name"
                    ),
                });
            }
            for (surface, below) in subdirectories(&path)? {
                if below.join(MANIFEST_FILE).is_file()
                    && manifest_name(&below)
                        .is_ok_and(|held| held.as_str() == format!("{namespace}/{surface}"))
                {
                    return Err(ConnectorRegistryError::LeafAndParent { namespace, path });
                }
            }
            found.push((name, path));
            continue;
        }
        for (surface, below) in subdirectories(&path)? {
            if !below.join(MANIFEST_FILE).is_file() {
                continue;
            }
            let held = match manifest_name(&below) {
                Ok(held) => held,
                // Where it lives names it a surface, or nothing: an installed
                // version's directory is no connector name.
                Err(_) => match ConnectorName::new(format!("{namespace}/{surface}")) {
                    Some(name) => name,
                    None => continue,
                },
            };
            // An installed version of the connector `namespace`, not a surface.
            if held.as_str() == namespace {
                continue;
            }
            if held.as_str() != format!("{namespace}/{surface}") {
                return Err(ConnectorRegistryError::InvalidManifest {
                    path: below.join(MANIFEST_FILE),
                    message: format!(
                        "connector {held} lives in connectors/{held}/, named by its name"
                    ),
                });
            }
            found.push((held, below));
        }
    }
    Ok(found)
}

/// The directories directly under `directory`, by name, without dot-entries;
/// none when it does not exist.
fn subdirectories(directory: &Path) -> Result<Vec<(String, PathBuf)>, ConnectorRegistryError> {
    let uninspectable = |error: std::io::Error| ConnectorRegistryError::Uninspectable {
        dir: directory.to_path_buf(),
        message: error.to_string(),
    };
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(uninspectable(error)),
    };
    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(uninspectable)?;
        if let Some(name) = entry.file_name().to_str()
            && !name.starts_with('.')
            && entry.path().is_dir()
        {
            directories.push((name.to_owned(), entry.path()));
        }
    }
    directories.sort();
    Ok(directories)
}

/// Where a connector named `name` lives under a `connectors/` directory:
/// `<root>/stripe`, `<root>/google/calendar`.
pub(crate) fn name_directory(root: &Path, name: &ConnectorName) -> PathBuf {
    name.as_str()
        .split('/')
        .fold(root.to_path_buf(), |path, segment| path.join(segment))
}

/// The name the manifest in `directory` declares, read without validating the
/// rest of it.
fn manifest_name(directory: &Path) -> Result<ConnectorName, ConnectorRegistryError> {
    /// What a manifest declares before the rest of it is read: its name.
    #[derive(serde::Deserialize)]
    struct Declared {
        id: ConnectorName,
    }
    let path = directory.join(MANIFEST_FILE);
    let document =
        read_bounded(&path).map_err(|message| ConnectorRegistryError::InvalidManifest {
            path: path.clone(),
            message,
        })?;
    runx_parser::parse_yaml_document::<Declared>(&document)
        .map(|declared| declared.id)
        .map_err(|error| ConnectorRegistryError::InvalidManifest {
            path,
            message: error.to_string(),
        })
}

/// The connector whose manifest is `directory/connector.yaml`, loaded as a
/// plugin's with its plugin at `root`. A runtime capability's own source tree
/// resolves to the embedded connector; any other source copy of it is refused.
pub(crate) fn load_connector_directory(
    directory: &Path,
    root: &Path,
) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
    load_directory(
        directory,
        ConnectorOrigin::Plugin {
            root: root.to_path_buf(),
        },
        true,
    )
}

/// The connector version installed at `directory`, its own plugin root: the
/// exact package a registry published. It may be another version of a
/// runtime capability.
pub(crate) fn load_installed(
    directory: &Path,
) -> Result<Arc<LoadedConnector>, ConnectorRegistryError> {
    load_directory(
        directory,
        ConnectorOrigin::Installed {
            directory: directory.to_path_buf(),
        },
        false,
    )
}

fn load_directory(
    directory: &Path,
    origin: ConnectorOrigin,
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
    let connector = loaded(validated, origin, tools).map_err(&invalid)?;
    if let Some(capability) = runtime_capabilities()?.connectors.get(&connector.name) {
        if capability.connector_digest == connector.connector_digest {
            return Ok(Arc::clone(capability));
        }
        if source {
            return Err(ConnectorRegistryError::Shadowed {
                id: connector.name.clone(),
                path,
            });
        }
    }
    Ok(Arc::new(connector))
}

pub(crate) fn embedded_manifest(connector: &LoadedConnector) -> Option<&'static str> {
    runtime_capabilities::CONNECTORS
        .iter()
        .find(|(id, _)| *id == connector.manifest.id.as_str())
        .map(|(_, document)| *document)
}

pub(crate) fn embedded_asset(connector: &LoadedConnector, path: &str) -> Option<&'static [u8]> {
    runtime_capabilities::ASSETS
        .iter()
        .find(|(id, file, _)| *id == connector.manifest.id.as_str() && *file == path)
        .map(|(_, _, document)| document.as_bytes())
}

/// A manifest's text, refused past 256 KiB.
pub(crate) fn read_bounded(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds 256 KiB".to_owned());
    }
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

/// The kernel tests' own plugin, whose `fixture/*` connectors carry each
/// connector shape the tests exercise. No test reaches a live provider connector.
#[cfg(test)]
fn fixture_plugin() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/plugins/kernel")
}

/// The kernel test plugin's scope, where every fixture connector is in reach
/// as a plugin connector.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) fn fixture_scope() -> ConnectorScope {
    ConnectorScope::at(&fixture_plugin()).expect("the kernel fixture plugin's scope")
}

/// Makes `root` a plugin holding copies of the fixture connectors `ids`, such
/// as `fixture/chat`: how a project reaches connectors without a registry.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) fn plugin_with_fixture_connectors(root: &Path, ids: &[&str]) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("connector directory");
        for entry in std::fs::read_dir(from).expect("source connector") {
            let entry = entry.expect("entry");
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).expect("connector file");
            }
        }
    }
    let source = fixture_plugin().join("connectors");
    for id in ids {
        copy(&source.join(id), &root.join("connectors").join(id));
    }
}

#[cfg(test)]
mod tests;
