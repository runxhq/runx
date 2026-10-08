//! Signed registry package bundles keep local skill composition portable.
//!
//! A published root package keeps its source `run.yaml` unchanged. Local
//! dependencies are stored below `dependencies/`, and this descriptor binds
//! only the root package edges that would otherwise escape the installed
//! package directory. The descriptor is part of `package_files`, so the
//! registry package digest commits both the mapping and every dependency file.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const PACKAGE_BUNDLE_FILE: &str = "runx.package.json";
const PACKAGE_BUNDLE_SCHEMA: &str = "runx.registry.package_bundle.v1";
const MAX_DESCRIPTOR_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackageBundleDependency {
    pub from: String,
    pub reference: String,
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageBundleDescriptor {
    schema: String,
    dependencies: Vec<PackageBundleDependency>,
}

#[cfg(any(feature = "process-tool", test))]
pub(crate) fn encode_package_bundle(
    dependencies: BTreeSet<PackageBundleDependency>,
) -> Result<String, String> {
    let descriptor = PackageBundleDescriptor {
        schema: PACKAGE_BUNDLE_SCHEMA.to_owned(),
        dependencies: dependencies.into_iter().collect(),
    };
    validate_descriptor(&descriptor)?;
    serde_json::to_string_pretty(&descriptor)
        .map(|document| format!("{document}\n"))
        .map_err(|error| format!("failed to encode registry package bundle: {error}"))
}

pub(crate) fn resolve_bundled_skill(
    graph_directory: &Path,
    reference: &str,
) -> Result<Option<PathBuf>, String> {
    let Some((bundle_root, descriptor)) = find_bundle(graph_directory)? else {
        return Ok(None);
    };
    let graph_directory = graph_directory.canonicalize().map_err(|error| {
        format!(
            "failed to resolve graph directory {} for registry package bundle: {error}",
            graph_directory.display()
        )
    })?;
    let source_directory = graph_directory
        .strip_prefix(&bundle_root)
        .map_err(|_| {
            format!(
                "graph directory {} is outside registry package bundle {}",
                graph_directory.display(),
                bundle_root.display()
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let source_directory = if source_directory.is_empty() {
        ".".to_owned()
    } else {
        source_directory
    };
    if let Some(dependency) = descriptor
        .dependencies
        .iter()
        .find(|dependency| dependency.from == source_directory && dependency.reference == reference)
    {
        let target = bundle_root.join(&dependency.path);
        let target = target.canonicalize().map_err(|error| {
            format!(
                "registry package dependency {} is unavailable: {error}",
                target.display()
            )
        })?;
        if !target.starts_with(&bundle_root) {
            return Err(format!(
                "registry package dependency {} escapes bundle {}",
                target.display(),
                bundle_root.display()
            ));
        }
        return Ok(Some(target));
    }
    if reference_escapes_bundle(&bundle_root, &graph_directory, reference) {
        return Err(format!(
            "registry package bundle has no dependency mapping for {source_directory}:{reference}"
        ));
    }
    Ok(None)
}

pub(crate) fn package_bundle_root(directory: &Path) -> Result<Option<PathBuf>, String> {
    find_bundle(directory).map(|bundle| bundle.map(|(root, _descriptor)| root))
}

fn find_bundle(
    graph_directory: &Path,
) -> Result<Option<(PathBuf, PackageBundleDescriptor)>, String> {
    for ancestor in graph_directory.ancestors() {
        let path = ancestor.join(PACKAGE_BUNDLE_FILE);
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "failed to inspect registry package bundle {}: {error}",
                    path.display()
                ));
            }
        };
        if metadata.len() > MAX_DESCRIPTOR_BYTES {
            return Err(format!(
                "registry package bundle {} exceeds {MAX_DESCRIPTOR_BYTES} bytes",
                path.display()
            ));
        }
        let document = fs::read_to_string(&path).map_err(|error| {
            format!(
                "failed to read registry package bundle {}: {error}",
                path.display()
            )
        })?;
        let descriptor: PackageBundleDescriptor =
            serde_json::from_str(&document).map_err(|error| {
                format!(
                    "registry package bundle {} is invalid: {error}",
                    path.display()
                )
            })?;
        validate_descriptor(&descriptor)?;
        let root = ancestor.canonicalize().map_err(|error| {
            format!(
                "failed to resolve registry package bundle root {}: {error}",
                ancestor.display()
            )
        })?;
        return Ok(Some((root, descriptor)));
    }
    Ok(None)
}

fn validate_descriptor(descriptor: &PackageBundleDescriptor) -> Result<(), String> {
    if descriptor.schema != PACKAGE_BUNDLE_SCHEMA {
        return Err(format!(
            "unsupported registry package bundle schema {:?}",
            descriptor.schema
        ));
    }
    let mut keys = BTreeSet::new();
    for dependency in &descriptor.dependencies {
        if dependency.reference.trim() != dependency.reference
            || dependency.reference.is_empty()
            || Path::new(&dependency.reference).is_absolute()
        {
            return Err("registry package bundle contains an invalid skill reference".to_owned());
        }
        validate_bundle_relative_path(&dependency.from, true)?;
        validate_bundle_relative_path(&dependency.path, true)?;
        if !keys.insert((&dependency.from, &dependency.reference)) {
            return Err(format!(
                "registry package bundle duplicates {}:{}",
                dependency.from, dependency.reference
            ));
        }
    }
    Ok(())
}

fn validate_bundle_relative_path(path: &str, allow_dot: bool) -> Result<(), String> {
    if allow_dot && path == "." {
        return Ok(());
    }
    if path.is_empty() || path.contains('\\') || Path::new(path).is_absolute() {
        return Err(format!("registry package bundle path {path:?} is invalid"));
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(format!("registry package bundle path {path:?} is unsafe"));
    }
    Ok(())
}

fn reference_escapes_bundle(bundle_root: &Path, graph_directory: &Path, reference: &str) -> bool {
    let Ok(relative_directory) = graph_directory.strip_prefix(bundle_root) else {
        return true;
    };
    let mut depth = relative_directory
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count();
    for component in Path::new(reference).components() {
        match component {
            Component::Normal(_) => depth = depth.saturating_add(1),
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return true,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_is_deterministic_and_rejects_duplicate_edges() -> Result<(), String> {
        let dependencies = BTreeSet::from([
            PackageBundleDependency {
                from: ".".to_owned(),
                reference: "../beta".to_owned(),
                path: "dependencies/skills/beta".to_owned(),
            },
            PackageBundleDependency {
                from: ".".to_owned(),
                reference: "../alpha".to_owned(),
                path: "dependencies/skills/alpha".to_owned(),
            },
        ]);
        let document = encode_package_bundle(dependencies)?;
        assert!(document.find("../alpha") < document.find("../beta"));
        let descriptor: PackageBundleDescriptor =
            serde_json::from_str(&document).map_err(|error| error.to_string())?;
        validate_descriptor(&descriptor)?;
        Ok(())
    }

    #[test]
    fn parent_traversal_is_classified_against_bundle_root() {
        let root = Path::new("/tmp/bundle");
        assert!(!reference_escapes_bundle(
            root,
            &root.join("graph/plan"),
            "../../internal"
        ));
        assert!(reference_escapes_bundle(
            root,
            &root.join("graph"),
            "../../external"
        ));
    }
}
