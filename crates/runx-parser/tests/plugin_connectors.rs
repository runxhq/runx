//! Every runx-maintained connector manifest parses and validates, and its
//! name is its directory: `connectors/<name>/` for a leaf,
//! `connectors/<namespace>/<surface>/` under a parent.

use std::fs;
use std::path::{Path, PathBuf};

use runx_parser::{parse_connector_manifest, validate_connector_manifest};

fn connectors_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../connectors")
}

fn directories(path: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            found.push((name, path));
        }
    }
    found.sort();
    Ok(found)
}

#[test]
fn runx_maintained_connectors_validate() -> Result<(), String> {
    let root = connectors_root();
    if !root.is_dir() {
        return Ok(());
    }
    let mut failures = Vec::new();
    let mut connectors = Vec::new();
    for entry in fs::read_dir(&root).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if !path.is_dir() {
            failures.push(format!("{} is not a connector directory", path.display()));
        }
    }
    for (namespace, directory) in directories(&root)? {
        if directory.join("connector.yaml").is_file() {
            connectors.push((namespace, directory));
            continue;
        }
        let surfaces = directories(&directory)?;
        if surfaces.is_empty() {
            failures.push(format!("{} holds no connector", directory.display()));
        }
        for (surface, path) in surfaces {
            connectors.push((format!("{namespace}/{surface}"), path));
        }
    }
    for (name, directory) in connectors {
        let manifest_path = directory.join("connector.yaml");
        let manifest = match fs::read_to_string(&manifest_path)
            .map_err(|error| error.to_string())
            .and_then(|source| parse_connector_manifest(&source).map_err(|error| error.to_string()))
        {
            Ok(manifest) => manifest,
            Err(error) => {
                failures.push(format!("{}: {error}", manifest_path.display()));
                continue;
            }
        };
        if manifest.id.as_str() != name {
            failures.push(format!(
                "{}: id {} does not match its directory",
                manifest_path.display(),
                manifest.id
            ));
        }
        if let Err(errors) = validate_connector_manifest(manifest) {
            failures.extend(
                errors
                    .into_iter()
                    .map(|error| format!("{}: {error:?}", manifest_path.display())),
            );
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
