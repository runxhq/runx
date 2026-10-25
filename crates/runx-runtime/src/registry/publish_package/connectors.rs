//! A connector's package: its exact manifest, every tool its process
//! operations run, and its README and mark when present. A skill package
//! carries each external connector it declares at `connectors/<name>/`, with its exact
//! binding in the bundle descriptor. Runtime capabilities remain in the binary.
//! A connector published on its own is the
//! same files at its package root.

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::connector::{ConnectorManifest, ConnectorName, Digest, OperationName};
use runx_parser::mark::{MARK_FILE, validate_mark_svg};
use runx_parser::{parse_connector_manifest, validate_connector_manifest};
use serde::{Deserialize, Serialize};

use super::RegistryPublishPackageError;
use crate::LoadedSkillPackage;
use crate::kernel::connectors::{
    ConnectorScope, ConnectorTool, LoadedConnector, MANIFEST_FILE, OperationDigests,
    embedded_asset, embedded_manifest, load_connector_directory, operation_digest_entries,
    plugin_root, read_bounded,
};
use crate::registry::RegistryPackageFile;
use crate::registry::package_metadata::{RegistryConnectorMetadata, declared_connectors};

/// Where a skill package keeps the plugin connectors it ships, below its root.
pub(crate) const PACKAGE_CONNECTORS_DIR: &str = "connectors";
const README_FILE: &str = "README.md";

/// A connector packaged on its own: its files at the package root, and what
/// the registry records from them, as the runtime loaded them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistryConnectorPackage {
    pub connector: ConnectorName,
    pub version: String,
    /// The version's identity: the digest of every file it consists of,
    /// which the registry's signed manifest names.
    pub content_digest: Digest,
    pub manifest: ConnectorManifest,
    pub manifest_digest: Digest,
    /// The manifest and every tool its process operations run.
    pub connector_digest: Digest,
    /// Each operation's digest and its implementation's, as runx Connect binds them.
    pub digests: BTreeMap<OperationName, OperationDigests>,
    pub files: Vec<RegistryPackageFile>,
}

/// The connector in `directory`, packaged for publication on its own.
///
/// # Errors
///
/// Returns an error when the connector does not load as a plugin's would, a
/// file is not UTF-8 text, the mark fails the mark check, or the package
/// exceeds the package limits.
pub fn prepare_registry_connector_package(
    directory: &Path,
) -> Result<RegistryConnectorPackage, RegistryPublishPackageError> {
    let directory = directory.canonicalize().map_err(|error| {
        RegistryPublishPackageError::invalid(format!(
            "connector directory {} cannot be read: {error}",
            directory.display()
        ))
    })?;
    // A connector names itself; it loads as its plugin's, or as its own
    // package root when it is in none.
    let root = plugin_root(&directory)
        .map_err(|error| RegistryPublishPackageError::invalid(error.to_string()))?
        .unwrap_or_else(|| directory.clone());
    let connector = load_connector_directory(&directory, &root)
        .map_err(|error| RegistryPublishPackageError::invalid(error.to_string()))?;
    connector_package(&connector, Some(&directory))
}

/// The connectors `loaded` declares, in canonical order, with the files of
/// each plugin or added connector added to `files` below its package root:
/// each the connector that runs in the project `paths` resolves, so a
/// requirement pinned to another publisher's manifest packages the project's
/// own copy, or else the added version that publishes it.
pub(super) fn package_connectors(
    loaded: &LoadedSkillPackage,
    files: &mut BTreeMap<String, RegistryPackageFile>,
    paths: &crate::RunxPaths,
) -> Result<Vec<RegistryConnectorMetadata>, RegistryPublishPackageError> {
    let declared = declared_connectors(loaded.manifest());
    if declared.is_empty() {
        return Ok(Vec::new());
    }
    let scope = ConnectorScope::in_project(&loaded.package_root, paths)
        .map_err(|error| RegistryPublishPackageError::invalid(error.to_string()))?;
    let fixed = loaded
        .manifest()
        .into_iter()
        .flat_map(|manifest| &manifest.connectors)
        .filter(|(_, requirement)| requirement.connector_from.is_none())
        .collect::<BTreeMap<_, _>>();
    let projected = declared
        .into_iter()
        .map(|(id, operations)| {
            let connector = match fixed.get(&id) {
                Some(requirement) => scope.requirement(&id, requirement),
                None => scope.connector(&id),
            }
            .map_err(|error| RegistryPublishPackageError::invalid(error.to_string()))?;
            if let Some(missing) = operations.iter().find(|operation| {
                !connector
                    .manifest
                    .operations
                    .contains_key(operation.as_str())
            }) {
                return Err(RegistryPublishPackageError::invalid(format!(
                    "connector {id} declares no operation {missing}"
                )));
            }
            let runtime_capability = connector
                .is_runtime_capability()
                .map_err(|error| RegistryPublishPackageError::invalid(error.to_string()))?;
            let directory = if runtime_capability {
                None
            } else {
                scope.connector_directory(&connector)
            };
            let package = connector_package(&connector, directory.as_deref())?;
            for file in package.files.into_iter().filter(|_| !runtime_capability) {
                super::files::insert_source_file(
                    files,
                    &format!("{PACKAGE_CONNECTORS_DIR}/{}/{}", connector.name, file.path),
                    file.content.as_bytes(),
                )?;
            }
            Ok(RegistryConnectorMetadata {
                connector_digest: connector.connector_digest.clone(),
                manifest_digest: connector.manifest_digest.clone(),
                runtime_capability,
                connector: connector.name.clone(),
                version: package.version,
                operations,
            })
        })
        .collect::<Result<Vec<_>, RegistryPublishPackageError>>()?;
    let mut canonical = BTreeMap::<ConnectorName, RegistryConnectorMetadata>::new();
    for metadata in projected {
        if let Some(existing) = canonical.get_mut(&metadata.connector) {
            for operation in metadata.operations {
                if !existing.operations.contains(&operation) {
                    existing.operations.push(operation);
                }
            }
        } else {
            canonical.insert(metadata.connector.clone(), metadata);
        }
    }
    Ok(canonical.into_values().collect())
}

/// The package of a loaded connector: its files as the runtime admitted them
/// from `directory` (the embedded table when `None`).
pub(crate) fn connector_package(
    connector: &LoadedConnector,
    directory: Option<&Path>,
) -> Result<RegistryConnectorPackage, RegistryPublishPackageError> {
    let mut files = BTreeMap::new();
    ship_connector(directory, connector, &mut files)?;
    super::files::validate_package_limits(&files)?;
    let files = files.into_values().collect::<Vec<_>>();
    let content_digest = runx_contracts::registry_content_digest(files.iter().map(|file| {
        runx_contracts::RegistryContentFile {
            path: &file.path,
            content: &file.content,
        }
    }))
    .ok()
    .and_then(Digest::new)
    .ok_or_else(|| {
        RegistryPublishPackageError::invalid("connector package has no content digest")
    })?;
    Ok(RegistryConnectorPackage {
        connector: connector.name.clone(),
        version: format!("sha-{}", &content_digest.as_str()[7..19]),
        content_digest,
        connector_digest: connector.connector_digest.clone(),
        manifest_digest: connector.manifest_digest.clone(),
        digests: operation_digest_entries(
            &connector.manifest,
            &connector.operation_digests,
            |tool| connector.tools.get(tool).map(|loaded| &loaded.digest),
        )
        .map_err(RegistryPublishPackageError::invalid)?,
        manifest: connector.manifest.clone(),
        files,
    })
}

/// Adds the connector in `directory` to `files`, each path behind `prefix`,
/// exactly as the runtime loaded it.
fn ship_connector(
    directory: Option<&Path>,
    connector: &LoadedConnector,
    files: &mut BTreeMap<String, RegistryPackageFile>,
) -> Result<(), RegistryPublishPackageError> {
    let id = connector.manifest.id.as_str();
    let invalid = |message: String| {
        RegistryPublishPackageError::invalid(format!("connector {id}: {message}"))
    };
    let manifest_path = directory.map(|directory| directory.join(MANIFEST_FILE));

    // The bytes shipped are the bytes the runtime admitted: read again, they
    // must still be that manifest.
    let manifest = match &manifest_path {
        Some(path) => read_bounded(path).map_err(&invalid)?,
        None => embedded_manifest(connector)
            .ok_or_else(|| invalid("connector source is unavailable".to_owned()))?
            .to_owned(),
    };
    let parsed = parse_connector_manifest(&manifest)
        .map_err(|error| invalid(format!("manifest is invalid: {error}")))?;
    let validated = validate_connector_manifest(parsed)
        .map_err(|errors| invalid(format!("manifest is invalid: {errors:?}")))?;
    if validated.manifest_digest != connector.manifest_digest {
        return Err(invalid(
            "manifest changed after the connector was loaded".to_owned(),
        ));
    }
    super::files::insert_source_file(files, MANIFEST_FILE, manifest.as_bytes())?;

    for (name, tool) in &connector.tools {
        let tool_path = ConnectorTool::package_path(name);
        for (path, bytes) in tool.files(name).map_err(&invalid)? {
            super::files::insert_source_file(files, &format!("{tool_path}/{path}"), &bytes)?;
        }
    }

    if let Some(readme) = package_asset(connector, directory, README_FILE).map_err(&invalid)? {
        super::files::insert_source_file(files, README_FILE, &readme)?;
    }
    if let Some(mark) = package_asset(connector, directory, MARK_FILE).map_err(&invalid)? {
        validate_mark_svg(&mark).map_err(|error| invalid(format!("{MARK_FILE}: {error}")))?;
        super::files::insert_source_file(files, MARK_FILE, &mark)?;
    }
    Ok(())
}

fn package_asset(
    connector: &LoadedConnector,
    directory: Option<&Path>,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    match directory {
        Some(directory) => read_optional(&directory.join(path)),
        None => Ok(embedded_asset(connector, path).map(<[u8]>::to_vec)),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}
