//! Regression guard for the "runs stay local" doctrine.
//!
//! The runtime emits nothing on its own: skills install from the registry,
//! but execution and its receipts stay on the machine that runs them. A run
//! reaches runx only when a principal publishes a receipt or runs on hosted
//! infra. Nothing here proves the absence of every possible egress, but it
//! fences the boundary that would let it regress silently: only `runx-runtime`
//! holds a network client, and its kernel sends only the operations a run
//! admits. No other crate can grow a seam where a usage ping gets bolted on,
//! and the crate that owns the signed proof cannot transmit it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const HTTP_CLIENTS: &[&str] = &[
    "reqwest",
    "hyper",
    "hyper-util",
    "reqwest-middleware",
    "ureq",
    "isahc",
    "surf",
    "attohttpc",
    "curl",
    "awc",
];

/// Resolve a sibling crate path from this crate's manifest dir (`crates/runx-cli`).
fn sibling(crate_dir: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // -> crates/
    path.push(crate_dir);
    path
}

/// Collect dependency names declared in a Cargo manifest across all
/// dependency tables (`[dependencies]`, `[dev-dependencies]`,
/// `[build-dependencies]`, their target-scoped variants, and the
/// `[dependencies.<name>]` long form).
fn dependency_names(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_deps = false;

    for raw in manifest.lines() {
        let line = raw.trim();
        if let Some(header) = line.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            if let Some(idx) = header.find("dependencies.") {
                // `[dependencies.foo]` / `[target.'cfg'.dependencies.foo]`
                let name = header[idx + "dependencies.".len()..].trim();
                if !name.is_empty() {
                    names.push(name.to_string());
                }
                in_deps = false; // body holds that dep's config, not new deps
            } else {
                in_deps = header == "dependencies"
                    || header == "dev-dependencies"
                    || header == "build-dependencies"
                    || header.ends_with(".dependencies")
                    || header.ends_with(".dev-dependencies")
                    || header.ends_with(".build-dependencies");
            }
            continue;
        }

        if !in_deps || line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `name = ...` or `name.workspace = true`
        let key = line.split(['=', '.']).next().unwrap_or("").trim();
        if !key.is_empty() {
            names.push(key.to_string());
        }
    }

    names
}

fn read_manifest(crate_dir: &str) -> io::Result<String> {
    let path = sibling(crate_dir).join("Cargo.toml");
    fs::read_to_string(path)
}

#[test]
fn only_the_runtime_holds_a_network_client() -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(sibling(""))? {
        let entry = entry?;
        let crate_dir = entry.file_name().to_string_lossy().into_owned();
        if crate_dir == "runx-runtime" || !entry.path().join("Cargo.toml").is_file() {
            continue;
        }
        let manifest = read_manifest(&crate_dir)?;
        for dep in dependency_names(&manifest) {
            let lower = dep.to_lowercase();
            assert!(
                !HTTP_CLIENTS.contains(&lower.as_str()),
                "{crate_dir} must not depend on an HTTP client (found `{dep}`); only \
                 runx-runtime reaches the network. See the 'runs stay local' doctrine.",
            );
        }
    }
    Ok(())
}

/// Sanity: the sibling crates resolve where the guard expects them.
#[test]
fn guarded_crates_exist() {
    for crate_dir in ["runx-cli", "runx-runtime", "runx-receipts"] {
        let path: &Path = &sibling(crate_dir);
        assert!(
            path.join("Cargo.toml").is_file(),
            "expected {} to be a crate with a Cargo.toml",
            path.display(),
        );
    }
}
