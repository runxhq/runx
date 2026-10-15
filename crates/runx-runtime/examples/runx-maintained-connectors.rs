//! Prints the runx-maintained connector manifests, validated, beside their
//! digests, computed only in Rust;
//! `scripts/generate-runx-maintained-connectors.mjs` embeds the runtime
//! capabilities among them.
//!
//! With `--lock`, prints instead the version each runx-maintained connector
//! publishes as, read by the native packager:
//! `registry/runx-maintained-connectors.lock.json`, which
//! `scripts/publish-runx-maintained-connectors.mjs` publishes from.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

const USAGE: &str = "usage: runx-maintained-connectors [--lock] <connectors directory>";

/// One runx-maintained connector as the registry records it: its name, the
/// content-derived version label it publishes under, and that content's digest.
#[derive(Serialize)]
struct LockEntry {
    #[serde(rename = "ref")]
    name: String,
    version: String,
    content_digest: String,
}

#[derive(Serialize)]
struct Lock {
    connectors: Vec<LockEntry>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let first = args.next().ok_or(USAGE)?;
    let (lock, root) = if first == "--lock" {
        (true, PathBuf::from(args.next().ok_or(USAGE)?))
    } else {
        (false, PathBuf::from(first))
    };
    if args.next().is_some() {
        return Err(USAGE.into());
    }
    let mut stdout = std::io::stdout().lock();
    if lock {
        serde_json::to_writer_pretty(&mut stdout, &runx_maintained_lock(&root)?)?;
    } else {
        let connectors =
            runx_runtime::kernel::connectors::runx_maintained_connectors_contract(&root)?;
        serde_json::to_writer_pretty(&mut stdout, &connectors)?;
    }
    writeln!(stdout)?;
    Ok(())
}

/// Every connector below `root`, by name, packaged as its publisher would
/// publish it.
fn runx_maintained_lock(root: &Path) -> Result<Lock, Box<dyn std::error::Error>> {
    let connectors = runx_runtime::kernel::connectors::connector_directories(root)?
        .iter()
        .map(|(_, directory)| {
            let package = runx_runtime::registry::prepare_registry_connector_package(directory)?;
            Ok(LockEntry {
                name: package.name.as_str().to_owned(),
                version: package.version,
                content_digest: package.content_digest.as_str().to_owned(),
            })
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    Ok(Lock { connectors })
}
