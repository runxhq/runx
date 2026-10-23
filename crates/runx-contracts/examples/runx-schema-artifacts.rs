use std::io::Write;
use std::path::PathBuf;

use runx_contracts::{SchemaDrift, generated_schema_artifacts, reconcile_schema_artifacts};

enum Options {
    /// Write, or with `check` compare, every artifact in `out_dir`.
    Reconcile { out_dir: PathBuf, check: bool },
    /// Print what generated bindings declare: each artifact's file name with
    /// the Rust type it comes from, and every named type with its own schema.
    Types,
}

fn main() -> Result<(), std::io::Error> {
    let (out_dir, check) = match parse_args()? {
        Options::Reconcile { out_dir, check } => (out_dir, check),
        Options::Types => return print_types(),
    };
    let drift = reconcile_schema_artifacts(&out_dir, check, generated_schema_artifacts())?;
    if drift.is_empty() {
        return Ok(());
    }
    report_schema_drift(&drift)?;
    Err(std::io::Error::other(
        "generated contract schemas are stale or orphaned",
    ))
}

fn print_types() -> Result<(), std::io::Error> {
    let mut artifacts = serde_json::Map::new();
    let mut types = serde_json::Map::new();
    for artifact in generated_schema_artifacts() {
        artifacts.insert(
            artifact.file_name.to_owned(),
            serde_json::Value::String(artifact.type_name.to_owned()),
        );
        for (name, schema) in artifact.named_schemas {
            match types.get(name) {
                Some(known) if *known != schema => {
                    return Err(std::io::Error::other(format!(
                        "two contract types are named {name}; rename one"
                    )));
                }
                Some(_) => {}
                None => {
                    types.insert(name.to_owned(), schema);
                }
            }
        }
    }
    let document = serde_json::json!({ "artifacts": artifacts, "types": types });
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &document).map_err(std::io::Error::other)?;
    writeln!(stdout)
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
    let mut types = false;
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
            "--types" => types = true,
            other => {
                return Err(std::io::Error::other(format!(
                    "unsupported argument: {other}"
                )));
            }
        }
    }
    if types {
        return Ok(Options::Types);
    }
    Ok(Options::Reconcile {
        out_dir: out_dir.ok_or_else(|| std::io::Error::other("--out is required"))?,
        check,
    })
}
