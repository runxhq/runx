use std::io::Write;
use std::path::PathBuf;

use runx_contracts::{SchemaDrift, generated_schema_artifacts, reconcile_schema_artifacts};

struct Options {
    out_dir: PathBuf,
    check: bool,
}

fn main() -> Result<(), std::io::Error> {
    let options = parse_args()?;
    let drift = reconcile_schema_artifacts(
        &options.out_dir,
        options.check,
        generated_schema_artifacts(),
    )?;
    if drift.is_empty() {
        return Ok(());
    }
    report_schema_drift(&drift)?;
    Err(std::io::Error::other(
        "generated contract schemas are stale or orphaned",
    ))
}

fn report_schema_drift(drift: &SchemaDrift) -> Result<(), std::io::Error> {
    let mut stderr = std::io::stderr().lock();
    if !drift.stale.is_empty() {
        writeln!(stderr, "Generated contract schemas are stale:")?;
        for file_name in &drift.stale {
            writeln!(stderr, "- {file_name}")?;
        }
    }
    if !drift.orphans.is_empty() {
        writeln!(stderr, "Orphan contract schemas are present:")?;
        for file_name in &drift.orphans {
            writeln!(stderr, "- {file_name}")?;
        }
    }
    Ok(())
}

fn parse_args() -> Result<Options, std::io::Error> {
    let mut out_dir = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                let value = args
                    .next()
                    .ok_or_else(|| std::io::Error::other("--out requires a directory"))?;
                out_dir = Some(PathBuf::from(value));
            }
            "--check" => check = true,
            other => {
                return Err(std::io::Error::other(format!(
                    "unsupported argument: {other}"
                )));
            }
        }
    }
    Ok(Options {
        out_dir: out_dir.ok_or_else(|| std::io::Error::other("--out is required"))?,
        check,
    })
}
