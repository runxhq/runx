use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::{
    MAX_BUNDLE_OPERATIONS, MAX_FILESYSTEM_MUTATION_BUNDLE_BYTES, TextBundle, invalid_bundle,
};
use crate::RuntimeError;
use crate::services::WorkspaceBounds;

pub(super) struct AdmittedWrite {
    pub(super) relative: String,
    pub(super) absolute: PathBuf,
    pub(super) contents: String,
}

pub(super) struct AdmittedBundle {
    pub(super) operation: String,
    pub(super) root: PathBuf,
    pub(super) writes: Vec<AdmittedWrite>,
    pub(super) deletes: Vec<(String, PathBuf)>,
}

/// Each change is bound to what it was written against: a named digest is
/// the content now there, and a file that must not exist does not. Within
/// agent bounds a change to a file that exists names the digest it read, so
/// an agent never replaces content it did not see.
fn admit_expected(
    operation: &str,
    named: &str,
    relative: &str,
    absolute: &Path,
    expected: Option<&super::Expected>,
    bounds: WorkspaceBounds,
) -> Result<(), RuntimeError> {
    let current = match fs::read(absolute) {
        Ok(bytes) => Some(runx_contracts::sha256_prefixed(&bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(RuntimeError::io(
                format!("reading {} before it changes", absolute.display()),
                source,
            ));
        }
    };
    let refused = |message: String| Err(invalid_bundle(operation, message));
    match (expected, current) {
        (Some(super::Expected::Digest(digest)), Some(current)) if *digest == current => Ok(()),
        (Some(super::Expected::Digest(_)), Some(current)) => refused(format!(
            "{relative} changed since it was read; it is now {current}"
        )),
        (Some(super::Expected::Digest(_)), None) => refused(format!("{relative} no longer exists")),
        (Some(super::Expected::Absent), Some(_)) => refused(format!(
            "{relative} exists; read it and name the digest it replaces"
        )),
        (Some(super::Expected::Absent), None) | (None, None) => Ok(()),
        (None, Some(_)) if bounds == WorkspaceBounds::Agent => refused(format!(
            "{named} exists and this change names no expected_digest: read it, then name the content_digest the read returned"
        )),
        (None, Some(_)) => Ok(()),
    }
}

pub(super) enum PriorFile {
    Absent,
    File {
        contents: Vec<u8>,
        permissions: fs::Permissions,
    },
}

pub(super) fn admit_text_bundle(
    operation: &str,
    root: &Path,
    bundle: &TextBundle,
    bounds: WorkspaceBounds,
) -> Result<AdmittedBundle, RuntimeError> {
    let operation_count = bundle.writes.len() + bundle.deletes.len();
    if operation_count > MAX_BUNDLE_OPERATIONS {
        return Err(invalid_bundle(
            operation,
            format!("bundle may contain at most {MAX_BUNDLE_OPERATIONS} operations"),
        ));
    }
    let root = fs::canonicalize(root).map_err(|source| {
        RuntimeError::io(format!("resolving bundle root {}", root.display()), source)
    })?;
    if !root.is_dir() {
        return Err(invalid_bundle(operation, "bundle root must be a directory"));
    }
    let named = bundle
        .writes
        .iter()
        .map(|write| write.path.as_str())
        .chain(bundle.deletes.iter().map(String::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(stray) = bundle
        .expected
        .keys()
        .find(|path| !named.contains(path.as_str()))
    {
        return Err(invalid_bundle(
            operation,
            format!("{stray} is expected but the bundle does not change it"),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    let writes = admit_writes(operation, &root, bundle, bounds, &mut seen)?;
    let deletes = admit_deletes(operation, &root, bundle, bounds, &mut seen)?;
    Ok(AdmittedBundle {
        operation: operation.to_owned(),
        root,
        writes,
        deletes,
    })
}

fn admit_writes(
    operation: &str,
    root: &Path,
    bundle: &TextBundle,
    bounds: WorkspaceBounds,
    seen: &mut std::collections::BTreeSet<String>,
) -> Result<Vec<AdmittedWrite>, RuntimeError> {
    let mut total_bytes = 0usize;
    let mut writes = Vec::with_capacity(bundle.writes.len());
    for write in &bundle.writes {
        let (relative, absolute) = admit_file_target(operation, root, &write.path, bounds)?;
        admit_unique_path(operation, seen, &relative)?;
        total_bytes = total_bytes
            .checked_add(write.contents.len())
            .ok_or_else(|| invalid_bundle(operation, "bundle byte count overflow"))?;
        if total_bytes > MAX_FILESYSTEM_MUTATION_BUNDLE_BYTES {
            return Err(invalid_bundle(
                operation,
                format!("bundle writes exceed {MAX_FILESYSTEM_MUTATION_BUNDLE_BYTES} bytes"),
            ));
        }
        reject_directory_target(operation, &absolute, &relative)?;
        admit_expected(
            operation,
            &write.path,
            &relative,
            &absolute,
            bundle.expected.get(&write.path),
            bounds,
        )?;
        writes.push(AdmittedWrite {
            relative,
            absolute,
            contents: write.contents.clone(),
        });
    }
    Ok(writes)
}

fn admit_deletes(
    operation: &str,
    root: &Path,
    bundle: &TextBundle,
    bounds: WorkspaceBounds,
    seen: &mut std::collections::BTreeSet<String>,
) -> Result<Vec<(String, PathBuf)>, RuntimeError> {
    let mut deletes = Vec::with_capacity(bundle.deletes.len());
    for delete in &bundle.deletes {
        let (relative, absolute) = admit_file_target(operation, root, delete, bounds)?;
        admit_unique_path(operation, seen, &relative)?;
        reject_directory_target(operation, &absolute, &relative)?;
        admit_expected(
            operation,
            delete,
            &relative,
            &absolute,
            bundle.expected.get(delete),
            bounds,
        )?;
        deletes.push((relative, absolute));
    }
    Ok(deletes)
}

fn admit_unique_path(
    operation: &str,
    seen: &mut std::collections::BTreeSet<String>,
    relative: &str,
) -> Result<(), RuntimeError> {
    if seen.insert(relative.to_owned()) {
        Ok(())
    } else {
        Err(invalid_bundle(
            operation,
            format!("bundle path has multiple operations: {relative}"),
        ))
    }
}

pub(super) fn admit_file_target(
    operation: &str,
    root: &Path,
    value: &str,
    bounds: WorkspaceBounds,
) -> Result<(String, PathBuf), RuntimeError> {
    if value.trim() != value || value.is_empty() || value.contains('\\') {
        return Err(invalid_bundle(
            operation,
            "bundle paths must be non-empty relative POSIX paths",
        ));
    }
    let path = Path::new(value);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(invalid_bundle(
                    operation,
                    format!("bundle path must stay inside the workspace root: {value}"),
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(invalid_bundle(operation, "bundle path must name a file"));
    }
    bounds
        .admit(&normalized)
        .map_err(|error| invalid_bundle(operation, error.to_string()))?;
    if normalized.file_name().is_some_and(|name| {
        let name = name.to_string_lossy().to_ascii_lowercase();
        name.starts_with(crate::kernel::store::KERNEL_STORE_FILE)
            || name == crate::receipts::store::STORE_LOCK_FILE_NAME
            || name.starts_with(&format!(
                ".{}.tmp.",
                crate::receipts::store::STORE_LOCK_FILE_NAME
            ))
    }) {
        return Err(invalid_bundle(
            operation,
            "kernel state cannot be changed by a file tool",
        ));
    }
    let relative = normalized
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let absolute = root.join(&normalized);
    reject_symlink_path(operation, root, &normalized)?;
    Ok((relative, absolute))
}

fn reject_symlink_path(operation: &str, root: &Path, relative: &Path) -> Result<(), RuntimeError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(invalid_bundle(
                    operation,
                    format!("bundle path crosses a symlink: {}", current.display()),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(RuntimeError::io(
                    format!("checking bundle path {}", current.display()),
                    source,
                ));
            }
        }
    }
    Ok(())
}

fn reject_directory_target(
    operation: &str,
    path: &Path,
    relative: &str,
) -> Result<(), RuntimeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Err(invalid_bundle(
            operation,
            format!("bundle target must be a file, not a directory: {relative}"),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RuntimeError::io(
            format!("checking bundle target {}", path.display()),
            source,
        )),
    }
}

pub(super) fn snapshot_paths(
    bundle: &AdmittedBundle,
) -> Result<std::collections::BTreeMap<String, PriorFile>, RuntimeError> {
    let mut snapshots = std::collections::BTreeMap::new();
    let mut bytes = 0usize;
    for (relative, path) in bundle
        .writes
        .iter()
        .map(|write| (&write.relative, &write.absolute))
        .chain(
            bundle
                .deletes
                .iter()
                .map(|(relative, path)| (relative, path)),
        )
    {
        let prior = match fs::read(path) {
            Ok(contents) => {
                bytes = bytes.checked_add(contents.len()).ok_or_else(|| {
                    invalid_bundle(&bundle.operation, "rollback snapshot byte count overflow")
                })?;
                if bytes > MAX_FILESYSTEM_MUTATION_BUNDLE_BYTES {
                    return Err(invalid_bundle(
                        &bundle.operation,
                        format!(
                            "bundle rollback snapshot exceeds {MAX_FILESYSTEM_MUTATION_BUNDLE_BYTES} bytes"
                        ),
                    ));
                }
                let permissions = fs::metadata(path)
                    .map_err(|source| {
                        RuntimeError::io(
                            format!("reading rollback metadata {}", path.display()),
                            source,
                        )
                    })?
                    .permissions();
                PriorFile::File {
                    contents,
                    permissions,
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => PriorFile::Absent,
            Err(source) => {
                return Err(RuntimeError::io(
                    format!("reading rollback snapshot {}", path.display()),
                    source,
                ));
            }
        };
        snapshots.insert(relative.clone(), prior);
    }
    Ok(snapshots)
}

pub(super) fn missing_parent_directories(
    bundle: &AdmittedBundle,
) -> Result<Vec<PathBuf>, RuntimeError> {
    let mut missing = std::collections::BTreeSet::new();
    for write in &bundle.writes {
        let Some(parent) = write.absolute.parent() else {
            continue;
        };
        let mut current = parent.to_path_buf();
        while current.starts_with(&bundle.root) && current != bundle.root {
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() => break,
                Ok(_) => {
                    return Err(invalid_bundle(
                        &bundle.operation,
                        format!("bundle parent is not a directory: {}", current.display()),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    missing.insert(current.clone());
                    let Some(parent) = current.parent() else {
                        break;
                    };
                    current = parent.to_path_buf();
                }
                Err(source) => {
                    return Err(RuntimeError::io(
                        format!("checking bundle parent {}", current.display()),
                        source,
                    ));
                }
            }
        }
    }
    let mut directories = missing.into_iter().collect::<Vec<_>>();
    directories.sort_by_key(|path| path.components().count());
    Ok(directories)
}
