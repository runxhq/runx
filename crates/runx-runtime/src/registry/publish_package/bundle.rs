//! Registry publication projects the complete local execution closure into one
//! digest-bound package without rewriting the source execution profile.

use std::collections::BTreeMap;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use super::RegistryPublishPackageError;
use super::files::CollectedPackage;
use crate::registry::RegistryPackageFile;
use crate::registry::package_bundle::{
    PACKAGE_BUNDLE_FILE, PackageBundleConnector, PackageBundleDependency, encode_package_bundle,
};
use crate::skill_package::{LoadedSkillPackage, LocalExecutionClosure};

const DEPENDENCY_ROOT: &str = "dependencies";

pub(super) fn collect_bundle_files(
    loaded: &LoadedSkillPackage,
    closure: &LocalExecutionClosure,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<CollectedPackage, RegistryPublishPackageError> {
    let root = canonical_directory(&loaded.package_root, "publish package root")?;
    let no_packet_ids = BTreeSet::new();
    let root_packet_ids = match closure.packages.get(&root) {
        Some(packet_ids) => packet_ids,
        None if loaded.manifest().is_none() && closure.packages.is_empty() => &no_packet_ids,
        None => {
            return Err(RegistryPublishPackageError::invalid(
                "publish execution closure omitted its root package",
            ));
        }
    };
    let CollectedPackage {
        files: root_files,
        connectors,
    } = super::files::collect_publish_package_files(loaded, env, cwd, root_packet_ids)?;
    // Every package carries its descriptor, dependencies or none: installed,
    // it marks the package root, the boundary its connector lookup and skill
    // edges never cross.
    let files = if let Some(descriptor) = loaded.package.source.files.get(PACKAGE_BUNDLE_FILE) {
        collect_materialized_bundle_files(
            closure,
            env,
            cwd,
            &root,
            root_files,
            &connectors,
            descriptor,
        )?
    } else {
        collect_source_bundle_files(closure, env, cwd, &root, root_files, &connectors)?
    };
    Ok(CollectedPackage { files, connectors })
}

/// A source package: each local skill dependency's package below
/// `dependencies/`, and the descriptor binding every edge to it.
fn collect_source_bundle_files(
    closure: &LocalExecutionClosure,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    root: &Path,
    root_files: Vec<RegistryPackageFile>,
    root_connectors: &[crate::registry::RegistryConnectorMetadata],
) -> Result<Vec<RegistryPackageFile>, RegistryPublishPackageError> {
    let mut connectors = connector_bindings(".", root_connectors);
    if closure.packages.len() <= 1 {
        let mut files = root_files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let descriptor = encode_package_bundle(BTreeSet::new(), connectors)
            .map_err(RegistryPublishPackageError::invalid)?;
        super::files::insert_source_file(&mut files, PACKAGE_BUNDLE_FILE, descriptor.as_bytes())?;
        super::files::validate_package_limits(&files)?;
        return Ok(files.into_values().collect());
    }
    let package_paths = portable_package_paths(root, closure)?;
    let mut files = root_files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    for (package_root, packet_ids) in &closure.packages {
        if package_root == root {
            continue;
        }
        let metadata = append_dependency_package(
            &mut files,
            package_path(&package_paths, package_root)?,
            package_root,
            packet_ids,
            env,
            cwd,
        )?;
        connectors.extend(connector_bindings(
            package_path(&package_paths, package_root)?,
            &metadata,
        ));
    }

    let dependencies = closure
        .skill_edges
        .iter()
        .map(|edge| {
            let from = bundled_graph_directory(
                &package_paths,
                &edge.source_package_root,
                &edge.graph_directory,
            )?;
            let target = package_path(&package_paths, &edge.target_package_root)?.to_owned();
            Ok(PackageBundleDependency {
                from,
                reference: edge.reference.clone(),
                path: target,
            })
        })
        .collect::<Result<_, RegistryPublishPackageError>>()?;
    let descriptor = encode_package_bundle(dependencies, connectors)
        .map_err(RegistryPublishPackageError::invalid)?;
    super::files::insert_source_file(&mut files, PACKAGE_BUNDLE_FILE, descriptor.as_bytes())?;
    super::files::validate_package_limits(&files)?;
    Ok(files.into_values().collect())
}

fn collect_materialized_bundle_files(
    closure: &LocalExecutionClosure,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    root: &Path,
    root_files: Vec<RegistryPackageFile>,
    root_connectors: &[crate::registry::RegistryConnectorMetadata],
    descriptor: &[u8],
) -> Result<Vec<RegistryPackageFile>, RegistryPublishPackageError> {
    if closure.packages.len() <= 1 {
        let empty =
            encode_package_bundle(BTreeSet::new(), connector_bindings(".", root_connectors))
                .map_err(RegistryPublishPackageError::invalid)?;
        if descriptor != empty.as_bytes() {
            return Err(RegistryPublishPackageError::invalid(format!(
                "{PACKAGE_BUNDLE_FILE} names dependencies this package does not use"
            )));
        }
    }
    let mut files = root_files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    for (package_root, packet_ids) in &closure.packages {
        if package_root == root {
            continue;
        }
        let relative = package_root.strip_prefix(root).map_err(|_| {
            RegistryPublishPackageError::invalid(format!(
                "materialized package dependency {} is outside bundle {}",
                package_root.display(),
                root.display()
            ))
        })?;
        let prefix = relative.to_string_lossy().replace('\\', "/");
        if prefix.is_empty() {
            return Err(RegistryPublishPackageError::invalid(
                "materialized package dependency path cannot be empty",
            ));
        }
        let dependency = crate::load_validated_skill_package(package_root)?;
        insert_prefixed(
            &mut files,
            &prefix,
            "SKILL.md",
            dependency.package.manual_markdown.as_bytes(),
        )?;
        if let Some(profile_path) = dependency.profile_path.as_deref()
            && let Some(profile_document) = dependency.package.file_text(profile_path)
        {
            insert_prefixed(
                &mut files,
                &prefix,
                runx_parser::PROFILE_FILE,
                profile_document.as_bytes(),
            )?;
        }
        for file in
            super::files::collect_publish_package_files(&dependency, env, cwd, packet_ids)?.files
        {
            insert_prefixed(&mut files, &prefix, &file.path, file.content.as_bytes())?;
        }
    }
    super::files::insert_source_file(&mut files, PACKAGE_BUNDLE_FILE, descriptor)?;
    super::files::validate_package_limits(&files)?;
    Ok(files.into_values().collect())
}

fn bundled_graph_directory(
    package_paths: &BTreeMap<PathBuf, String>,
    source_package_root: &Path,
    graph_directory: &Path,
) -> Result<String, RegistryPublishPackageError> {
    let relative = graph_directory
        .strip_prefix(source_package_root)
        .map_err(|_| {
            RegistryPublishPackageError::invalid(format!(
                "graph directory {} is outside source package {}",
                graph_directory.display(),
                source_package_root.display()
            ))
        })?;
    let base = package_path(package_paths, source_package_root)?;
    let bundled = if relative.as_os_str().is_empty() {
        PathBuf::from(base)
    } else if base == "." {
        relative.to_path_buf()
    } else {
        PathBuf::from(base).join(relative)
    };
    let bundled = bundled.to_string_lossy().replace('\\', "/");
    Ok(if bundled.is_empty() {
        ".".to_owned()
    } else {
        bundled
    })
}

fn append_dependency_package(
    files: &mut BTreeMap<String, RegistryPackageFile>,
    prefix: &str,
    package_root: &Path,
    packet_ids: &std::collections::BTreeSet<String>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<crate::registry::RegistryConnectorMetadata>, RegistryPublishPackageError> {
    let dependency = crate::load_validated_skill_package(package_root)?;
    insert_prefixed(
        files,
        prefix,
        "SKILL.md",
        dependency.package.manual_markdown.as_bytes(),
    )?;
    if let Some(profile_path) = dependency.profile_path.as_deref()
        && let Some(profile_document) = dependency.package.file_text(profile_path)
    {
        insert_prefixed(
            files,
            prefix,
            runx_parser::PROFILE_FILE,
            profile_document.as_bytes(),
        )?;
    }
    let collected = super::files::collect_publish_package_files(&dependency, env, cwd, packet_ids)?;
    for file in collected.files {
        insert_prefixed(files, prefix, &file.path, file.content.as_bytes())?;
    }
    Ok(collected.connectors)
}

fn connector_bindings(
    from: &str,
    connectors: &[crate::registry::RegistryConnectorMetadata],
) -> BTreeSet<PackageBundleConnector> {
    connectors
        .iter()
        .map(|connector| PackageBundleConnector {
            from: from.to_owned(),
            connector: connector.name.clone(),
            manifest_digest: connector.manifest_digest.clone(),
            connector_digest: connector.connector_digest.clone(),
        })
        .collect()
}

fn insert_prefixed(
    files: &mut BTreeMap<String, RegistryPackageFile>,
    prefix: &str,
    relative: &str,
    contents: &[u8],
) -> Result<(), RegistryPublishPackageError> {
    super::files::insert_source_file(files, &format!("{prefix}/{relative}"), contents)
}

fn package_path<'a>(
    package_paths: &'a BTreeMap<PathBuf, String>,
    package_root: &Path,
) -> Result<&'a str, RegistryPublishPackageError> {
    package_paths
        .get(package_root)
        .map(String::as_str)
        .ok_or_else(|| {
            RegistryPublishPackageError::invalid(format!(
                "publish dependency {} is absent from the execution closure",
                package_root.display()
            ))
        })
}

/// Assign slots by walking declared graph edges from the root. Filesystem
/// paths never determine published paths, and distinct source bindings remain
/// distinct even when their package bytes are identical. The existing digest
/// commits all files and the complete edge descriptor.
fn portable_package_paths(
    root: &Path,
    closure: &LocalExecutionClosure,
) -> Result<BTreeMap<PathBuf, String>, RegistryPublishPackageError> {
    let mut paths = BTreeMap::from([(root.to_path_buf(), ".".to_owned())]);
    let mut pending = VecDeque::from([root.to_path_buf()]);
    while let Some(source) = pending.pop_front() {
        let mut edges = closure
            .skill_edges
            .iter()
            .filter(|edge| edge.source_package_root == source)
            .map(|edge| {
                let graph = edge.graph_directory.strip_prefix(&source).map_err(|_| {
                    RegistryPublishPackageError::invalid(
                        "publish graph directory is outside its source package",
                    )
                })?;
                Ok((
                    graph.to_string_lossy().replace('\\', "/"),
                    &edge.reference,
                    edge,
                ))
            })
            .collect::<Result<Vec<_>, RegistryPublishPackageError>>()?;
        edges.sort_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
        for (_, _, edge) in edges {
            if !paths.contains_key(&edge.target_package_root) {
                let path = format!("{DEPENDENCY_ROOT}/{}", paths.len());
                paths.insert(edge.target_package_root.clone(), path);
                pending.push_back(edge.target_package_root.clone());
            }
        }
    }
    if paths.len() != closure.packages.len() {
        return Err(RegistryPublishPackageError::invalid(
            "publish execution closure contains unreachable packages",
        ));
    }
    Ok(paths)
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, RegistryPublishPackageError> {
    path.canonicalize().map_err(|error| {
        RegistryPublishPackageError::invalid(format!(
            "failed to resolve {label} {}: {error}",
            path.display()
        ))
    })
}
