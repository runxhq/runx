// Module rationale: a real run measures a package against the catalog a host
// sees, so this serves the checkout's own first-party catalog as a registry
// on loopback: every package under `skills/`, packaged as `runx registry
// publish` packages it, under the `catalog` publisher, signed by the loopback
// registry's key. A turn that loads `catalog/<name>` binds the same package,
// by digest, as one that loads the published `runx/<name>` of the same files.
//
//   cargo run --manifest-path crates/Cargo.toml -p runx-runtime \
//     --example runx-catalog-registry [-- --root <checkout>]
//
// It prints the environment a run reaching it needs, as shell exports, and
// serves until it is stopped.

#[path = "../tests/registry_fixture.rs"]
mod registry_fixture;

use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use registry_fixture::LoopbackRegistry;

/// The namespace the catalog is published under. `runx` is the official
/// registry's own, which no operator key signs.
const PUBLISHER: &str = "catalog";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ignored = writeln!(io::stderr().lock(), "runx: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let root = root(std::env::args_os().skip(1))?;
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let registry = LoopbackRegistry::serve()?;
    let packages = packages(&root.join("skills"))?;
    for package in &packages {
        registry
            .publish_directory(package, PUBLISHER, &env, &root)
            .map_err(|error| format!("{}: {error}", package.display()))?;
    }

    let mut exports = registry.env_trusting(PUBLISHER)?;
    exports.insert("RUNX_REGISTRY_URL".to_owned(), registry.url.clone());
    let mut stdout = io::stdout().lock();
    for (key, value) in &exports {
        writeln!(stdout, "export {key}={}", quoted(value))?;
    }
    stdout.flush()?;
    drop(stdout);
    writeln!(
        io::stderr().lock(),
        "serving {} packages from {} as {PUBLISHER}/<name> at {}, until stopped",
        packages.len(),
        root.join("skills").display(),
        registry.url
    )?;
    loop {
        std::thread::park();
    }
}

/// The checkout `--root` names, else the current directory.
fn root(mut args: impl Iterator<Item = OsString>) -> Result<PathBuf, Box<dyn Error>> {
    let mut root = std::env::current_dir()?;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--root") => {
                root = PathBuf::from(args.next().ok_or("--root names a checkout")?);
            }
            _ => return Err(format!("unknown argument {}", arg.to_string_lossy()).into()),
        }
    }
    Ok(root)
}

/// Each package under `skills`: every directory holding a `SKILL.md`, in
/// name order.
fn packages(skills: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut packages = Vec::new();
    for entry in std::fs::read_dir(skills)
        .map_err(|error| format!("{} does not read: {error}", skills.display()))?
    {
        let path = entry?.path();
        if path.join("SKILL.md").is_file() {
            packages.push(path);
        }
    }
    packages.sort();
    if packages.is_empty() {
        return Err(format!("{} holds no package", skills.display()).into());
    }
    Ok(packages)
}

/// `value` as one shell word.
fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}
