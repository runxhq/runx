//! Registry publication projects the complete local execution closure into one
//! digest-bound package without rewriting the source execution profile.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::RegistryPublishPackageError;
use crate::registry::RegistryPackageFile;
use crate::registry::package_bundle::{
    PACKAGE_BUNDLE_FILE, PackageBundleDependency, encode_package_bundle,
};
use crate::skill_package::{LoadedSkillPackage, LocalExecutionClosure};

const DEPENDENCY_ROOT: &str = "dependencies";

pub(super) fn collect_bundle_files(
    loaded: &LoadedSkillPackage,
    closure: &LocalExecutionClosure,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<RegistryPackageFile>, RegistryPublishPackageError> {
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
    let root_files =
        super::files::collect_publish_package_files(loaded, env, cwd, root_packet_ids)?;
    if let Some(descriptor) = loaded.package.source.files.get(PACKAGE_BUNDLE_FILE) {
        return collect_materialized_bundle_files(closure, env, cwd, &root, root_files, descriptor);
    }
    if closure.packages.len() <= 1 {
        return Ok(root_files);
    }

    let source_root = common_source_root(&root, closure.packages.keys())?;
    let mut files = root_files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    for (package_root, packet_ids) in &closure.packages {
        if package_root == &root {
            continue;
        }
        append_dependency_package(&mut files, &source_root, package_root, packet_ids, env, cwd)?;
    }

    let dependencies = closure
        .skill_edges
        .iter()
        .map(|edge| {
            let from = bundled_graph_directory(
                &root,
                &source_root,
                &edge.source_package_root,
                &edge.graph_directory,
            )?;
            let target = bundled_package_path(&root, &source_root, &edge.target_package_root)?;
            Ok(PackageBundleDependency {
                from,
                reference: edge.reference.clone(),
                path: target,
            })
        })
        .collect::<Result<_, RegistryPublishPackageError>>()?;
    let descriptor =
        encode_package_bundle(dependencies).map_err(RegistryPublishPackageError::invalid)?;
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
    descriptor: &[u8],
) -> Result<Vec<RegistryPackageFile>, RegistryPublishPackageError> {
    if closure.packages.len() <= 1 {
        return Err(RegistryPublishPackageError::invalid(format!(
            "{PACKAGE_BUNDLE_FILE} does not bind a package dependency"
        )));
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
        for file in super::files::collect_publish_package_files(&dependency, env, cwd, packet_ids)?
        {
            insert_prefixed(&mut files, &prefix, &file.path, file.content.as_bytes())?;
        }
    }
    super::files::insert_source_file(&mut files, PACKAGE_BUNDLE_FILE, descriptor)?;
    super::files::validate_package_limits(&files)?;
    Ok(files.into_values().collect())
}

fn bundled_graph_directory(
    root: &Path,
    source_root: &Path,
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
    let base = bundled_package_path(root, source_root, source_package_root)?;
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

fn bundled_package_path(
    root: &Path,
    source_root: &Path,
    package_root: &Path,
) -> Result<String, RegistryPublishPackageError> {
    if package_root == root {
        Ok(".".to_owned())
    } else {
        dependency_path(source_root, package_root)
    }
}

fn append_dependency_package(
    files: &mut BTreeMap<String, RegistryPackageFile>,
    source_root: &Path,
    package_root: &Path,
    packet_ids: &std::collections::BTreeSet<String>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<(), RegistryPublishPackageError> {
    let dependency = crate::load_validated_skill_package(package_root)?;
    let prefix = dependency_path(source_root, package_root)?;
    insert_prefixed(
        files,
        &prefix,
        "SKILL.md",
        dependency.package.manual_markdown.as_bytes(),
    )?;
    if let Some(profile_path) = dependency.profile_path.as_deref()
        && let Some(profile_document) = dependency.package.file_text(profile_path)
    {
        insert_prefixed(
            files,
            &prefix,
            runx_parser::PROFILE_FILE,
            profile_document.as_bytes(),
        )?;
    }
    for file in super::files::collect_publish_package_files(&dependency, env, cwd, packet_ids)? {
        insert_prefixed(files, &prefix, &file.path, file.content.as_bytes())?;
    }
    Ok(())
}

fn insert_prefixed(
    files: &mut BTreeMap<String, RegistryPackageFile>,
    prefix: &str,
    relative: &str,
    contents: &[u8],
) -> Result<(), RegistryPublishPackageError> {
    super::files::insert_source_file(files, &format!("{prefix}/{relative}"), contents)
}

fn dependency_path(
    source_root: &Path,
    package_root: &Path,
) -> Result<String, RegistryPublishPackageError> {
    let relative = package_root.strip_prefix(source_root).map_err(|_| {
        RegistryPublishPackageError::invalid(format!(
            "publish dependency {} is outside source root {}",
            package_root.display(),
            source_root.display()
        ))
    })?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    if relative.is_empty() {
        return Err(RegistryPublishPackageError::invalid(
            "publish dependency path cannot be empty",
        ));
    }
    Ok(format!("{DEPENDENCY_ROOT}/{relative}"))
}

fn common_source_root<'a>(
    target_root: &Path,
    package_roots: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<PathBuf, RegistryPublishPackageError> {
    let mut common = target_root.parent().map(Path::to_path_buf).ok_or_else(|| {
        RegistryPublishPackageError::invalid(format!(
            "publish package root {} has no parent directory",
            target_root.display()
        ))
    })?;
    for package_root in package_roots {
        while !package_root.starts_with(&common) {
            if !common.pop() {
                return Err(RegistryPublishPackageError::invalid(
                    "publish execution closure has no common filesystem root",
                ));
            }
        }
    }
    if common.parent().is_none() {
        return Err(RegistryPublishPackageError::invalid(
            "publish execution closure spans unrelated filesystem roots",
        ));
    }
    Ok(common)
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, RegistryPublishPackageError> {
    path.canonicalize().map_err(|error| {
        RegistryPublishPackageError::invalid(format!(
            "failed to resolve {label} {}: {error}",
            path.display()
        ))
    })
}
