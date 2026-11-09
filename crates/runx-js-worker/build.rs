//! The worker's source-tree digest, made at build time from the sources it
//! builds from, which it reports when it starts (reuse contract,
//! `implementation`).

// Cargo reads a build script's instructions from its standard output.
#![allow(clippy::print_stdout)]

include!("../runx-runtime/source_digest.rs");

fn main() -> std::io::Result<()> {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| std::io::Error::other("CARGO_MANIFEST_DIR is unset"))?,
    );
    let (digest, read) = source_digest(&manifest)?;
    println!("cargo:rustc-env=RUNX_JS_WORKER_SOURCE_DIGEST={digest}");
    println!("cargo:rerun-if-changed=../runx-runtime/source_digest.rs");
    for path in read {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    Ok(())
}
