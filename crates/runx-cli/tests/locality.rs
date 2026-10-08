//! Regression guard for the "runs stay local" doctrine.
//!
//! The runtime emits nothing on its own: skills install from the registry,
//! but execution and its receipts stay on the machine that runs them. A run
//! reaches runx only when a principal publishes a receipt or runs on hosted
//! infra. Nothing here proves the absence of every possible egress, but it
//! fences the two boundaries that would let it regress silently:
//!
//!   1. `runx-receipts` — the crate that owns the signed proof — must never
//!      gain a network client. Receipts are a local artifact; the type that
//!      models them must not be able to transmit them.
//!   2. `runx-runtime` network access stays opt-in (behind a feature, off by
//!      default), so the bounded HTTP a *skill* performs can never become an
//!      always-on channel.
//!
//! A skill making its own HTTP calls to skill-defined hosts is bounded work,
//! not telemetry, so HTTP clients are not banned outright — only walled off
//! from the receipt crate and kept out of the runtime's default features.

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
fn receipts_crate_has_no_network_client() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = read_manifest("runx-receipts")?;
    for dep in dependency_names(&manifest) {
        let lower = dep.to_lowercase();
        assert!(
            !HTTP_CLIENTS.contains(&lower.as_str()),
            "runx-receipts must not depend on an HTTP client (found `{dep}`). \
             Receipts are a local artifact; the crate that owns the signed proof \
             must not be able to transmit it. See the 'runs stay local' doctrine.",
        );
    }
    Ok(())
}

#[test]
fn runtime_network_is_opt_in() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = read_manifest("runx-runtime")?;

    let reqwest = manifest
        .lines()
        .map(str::trim_start)
        .find(|line| line.starts_with("reqwest"))
        .ok_or_else(|| io::Error::other("runx-runtime is expected to declare reqwest"))?;
    assert!(
        reqwest.contains("optional = true"),
        "runx-runtime's reqwest must be `optional = true` so network access is \
         behind a feature, never linked unconditionally. Got: {reqwest}",
    );

    let default = manifest
        .lines()
        .map(str::trim_start)
        .find(|line| line.starts_with("default ="))
        .ok_or_else(|| {
            io::Error::other("runx-runtime [features] is expected to declare `default`")
        })?;
    assert!(
        !default.contains("async-http")
            && !default.contains("process-tool")
            && !default.contains("catalog"),
        "runx-runtime default features must not enable network (async-http). \
         Network stays opt-in. Got: {default}",
    );
    Ok(())
}

#[test]
fn cli_declares_no_direct_network_client() -> Result<(), Box<dyn std::error::Error>> {
    // The CLI may reach the registry transitively (install/acquire is the one
    // public signal), but it should not declare its own HTTP client — that
    // would be the seam where a usage ping gets bolted on.
    let manifest = read_manifest("runx-cli")?;
    for dep in dependency_names(&manifest) {
        let lower = dep.to_lowercase();
        assert!(
            !HTTP_CLIENTS.contains(&lower.as_str()),
            "runx-cli must not declare a direct HTTP client (found `{dep}`). \
             See the 'runs stay local' doctrine.",
        );
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
