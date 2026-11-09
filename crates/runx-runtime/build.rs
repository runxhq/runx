//! The runtime's source-tree digest, and the worker's it runs with, made at
//! build time from the sources each builds from (reuse contract,
//! `implementation`). A build from a package, outside its workspace, has
//! the worker's sources to hand no more, and names no worker.

// Cargo reads a build script's instructions from its standard output.
#![allow(clippy::print_stdout)]

include!("source_digest.rs");

fn main() -> std::io::Result<()> {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| std::io::Error::other("CARGO_MANIFEST_DIR is unset"))?,
    );
    let (runtime, mut read) = source_digest(&manifest)?;
    println!("cargo:rustc-env=RUNX_RUNTIME_SOURCE_DIGEST={runtime}");
    let worker = manifest
        .parent()
        .map(|workspace| workspace.join("runx-js-worker"))
        .filter(|worker| worker.join("Cargo.toml").is_file());
    if let Some(worker) = worker {
        let (digest, watched) = source_digest(&worker)?;
        println!("cargo:rustc-env=RUNX_JS_WORKER_SOURCE_DIGEST={digest}");
        read.extend(watched);
    }
    for path in read {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    Ok(())
}
