//! Writes the canonical receipts Cloud's spec shows to
//! fixtures/contracts/canonical-receipts, each made by a real 0.12 test replay
//! under the public fixture key at the fixed clock. When the receipt's shape
//! changes, this one command makes them again:
//!
//!     pnpm fixtures:canonical-receipts:generate
//!
//! The integration test `canonical_receipts` holds the fixtures to what this
//! makes and verifies each against tools/verify/runx-demo-jwks.json.

use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

mod scenarios;

fn main() -> ExitCode {
    match write() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ignored = writeln!(io::stderr().lock(), "runx: {error}");
            ExitCode::from(1)
        }
    }
}

fn write() -> Result<(), Box<dyn Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = repo.join(scenarios::FIXTURE_DIR);
    fs::create_dir_all(&directory)?;
    let mut stdout = io::stdout().lock();
    for canonical in scenarios::canonical_receipts(&repo)? {
        fs::write(
            directory.join(canonical.file),
            scenarios::render(&canonical.receipt)?,
        )?;
        writeln!(stdout, "{} {}", canonical.file, canonical.receipt.id)?;
    }
    Ok(())
}
