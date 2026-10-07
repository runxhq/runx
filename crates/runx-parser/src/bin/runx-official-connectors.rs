//! Prints the official connector manifests, validated, beside the digests
//! runx Connect compares against what a runtime admitted. Digests are computed only in Rust;
//! `scripts/generate-official-connectors.mjs` writes this into
//! `@runxhq/contracts`.

use std::io::Write;
use std::path::PathBuf;

use runx_contracts::connector::implementation_digest;
use serde_json::{Map, Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: runx-official-connectors <connectors directory>")?;
    let mut ids = std::fs::read_dir(&root)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    ids.sort();
    let mut connectors = Vec::with_capacity(ids.len());
    for id in ids {
        let yaml = std::fs::read_to_string(root.join(&id).join("connector.yaml"))?;
        let validated =
            runx_parser::validate_connector_manifest(runx_parser::parse_connector_manifest(&yaml)?)
                .map_err(|errors| format!("connectors/{id}: {errors:?}"))?;
        let manifest = &validated.manifest;
        let server = manifest
            .placements
            .local
            .as_ref()
            .and_then(|local| local.mcp_server.as_ref());
        let mut digests = Map::new();
        for (name, operation) in &manifest.operations {
            let mut entry = Map::from_iter([(
                "operation".to_owned(),
                Value::from(validated.operation_digests[name].as_str()),
            )]);
            if let Some(implementation) = &operation.implementation {
                entry.insert(
                    "implementation".to_owned(),
                    Value::from(implementation_digest(implementation, server)?.as_str()),
                );
            }
            digests.insert(name.as_str().to_owned(), Value::Object(entry));
        }
        connectors.push(json!({
            "manifest_digest": validated.manifest_digest.as_str(),
            "digests": digests,
            "manifest": serde_json::to_value(manifest)?,
        }));
    }
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &connectors)?;
    writeln!(stdout)?;
    Ok(())
}
