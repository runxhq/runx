// The source-tree digest of a crate and the workspace crates it builds from
// (reuse contract, `implementation`): each binary reports it, so a run
// started by one build is never resumed or reused by a build of other
// sources. Shared by the build scripts that make it; it reads only files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The digest of `crate_dir`'s crate and every workspace crate it depends
/// on, as `sha256:<hex>`, with each path it read: every file of each crate
/// but its tests, benches, examples and fuzz targets, which no binary is
/// built from, by its path in the workspace. A crate built outside its
/// workspace, from a package, reads its own files alone. The same files give
/// the same digest.
pub fn source_digest(crate_dir: &Path) -> std::io::Result<(String, Vec<PathBuf>)> {
    let workspace = crate_dir.parent().filter(|parent| parent.join("Cargo.toml").is_file());
    let mut crates = BTreeSet::new();
    let mut pending = vec![crate_dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        if !crates.insert(dir.clone()) {
            continue;
        }
        let Some(workspace) = workspace else {
            continue;
        };
        for dependency in workspace_dependencies(&dir.join("Cargo.toml"))? {
            let sibling = workspace.join(&dependency);
            if sibling.join("Cargo.toml").is_file() {
                pending.push(sibling);
            }
        }
    }
    let root = workspace.unwrap_or(crate_dir);
    let mut files = BTreeMap::new();
    let mut read = Vec::new();
    for dir in &crates {
        let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name();
            if entry.file_type()?.is_dir() {
                if NOT_BUILT.iter().any(|skipped| name == *skipped) {
                    continue;
                }
                collect(root, &path, &mut files)?;
            } else {
                files.insert(relative(root, &path), std::fs::read(&path)?);
            }
            read.push(path);
        }
    }
    let mut hash = Sha256::new();
    for (path, bytes) in &files {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(bytes);
    }
    let digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok((format!("sha256:{digest}"), read))
}

/// A crate's directories no binary is built from.
const NOT_BUILT: [&str; 5] = ["tests", "benches", "examples", "fuzz", "target"];

/// The workspace crates a manifest's dependencies name: each `runx-` key of
/// its dependency tables, the workspace's own crates.
fn workspace_dependencies(manifest: &Path) -> std::io::Result<Vec<String>> {
    let text = std::fs::read_to_string(manifest)?;
    let mut in_dependencies = false;
    let mut found = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_dependencies = line == "[dependencies]"
                || (line.starts_with("[target.") && line.ends_with(".dependencies]"));
            continue;
        }
        if !in_dependencies {
            continue;
        }
        let key = line
            .split(['=', '.', ' '])
            .next()
            .unwrap_or_default();
        if key.starts_with("runx-") {
            found.push(key.to_owned());
        }
    }
    Ok(found)
}

fn collect(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect(root, &path, files)?;
        } else {
            files.insert(relative(root, &path), std::fs::read(&path)?);
        }
    }
    Ok(())
}

/// `path` below `root`, with `/` between its parts on every platform.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}
