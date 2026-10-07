//! Every official connector manifest parses and validates, and its id
//! matches its directory.

use std::fs;
use std::path::{Path, PathBuf};

use runx_parser::{parse_connector_manifest, validate_connector_manifest};

fn connectors_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../connectors")
}

#[test]
fn official_connectors_validate() -> Result<(), String> {
    let Ok(entries) = fs::read_dir(connectors_root()) else {
        return Ok(());
    };
    let mut failures = Vec::new();
    for entry in entries {
        let directory = entry.map_err(|error| error.to_string())?.path();
        if !directory.is_dir() {
            failures.push(format!(
                "{} is not a connector directory",
                directory.display()
            ));
            continue;
        }
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
        let directory_name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if manifest.id.as_str() != directory_name {
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
