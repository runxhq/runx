//! Prints the official connector manifests, validated, beside the digests
//! runx Connect compares against what a runtime admitted. Digests are
//! computed only in Rust; `scripts/generate-official-connectors.mjs` writes
//! this into `@runxhq/contracts`.

use std::io::Write;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: runx-official-connectors <connectors directory>")?;
    let connectors = runx_runtime::kernel::connectors::official_connectors_contract(&root)?;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &connectors)?;
    writeln!(stdout)?;
    Ok(())
}
