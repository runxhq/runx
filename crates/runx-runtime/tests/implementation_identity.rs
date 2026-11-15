//! A build's identity is the digest of the sources it was built from (reuse
//! contract, `implementation`), never of where or for what it was built: the
//! same sources built for another target give the same identity, so nothing
//! recomputes, and a changed source gives another.

#[allow(dead_code)]
mod digest {
    include!("../source_digest.rs");
}

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A workspace holding `runx-a`, which builds from its sibling `runx-b`.
fn workspace(root: &std::path::Path) -> TestResult<std::path::PathBuf> {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"runx-a\", \"runx-b\"]\n",
    )?;
    for (name, manifest) in [
        (
            "runx-a",
            "[package]\nname = \"runx-a\"\n\n[dependencies]\nrunx-b = { path = \"../runx-b\" }\n",
        ),
        ("runx-b", "[package]\nname = \"runx-b\"\n"),
    ] {
        std::fs::create_dir_all(root.join(name).join("src"))?;
        std::fs::write(root.join(name).join("Cargo.toml"), manifest)?;
        std::fs::write(root.join(name).join("src/lib.rs"), format!("// {name}\n"))?;
    }
    Ok(root.join("runx-a"))
}

#[test]
fn the_same_sources_give_one_identity_whatever_they_were_built_for() -> TestResult {
    let temp = tempfile::tempdir()?;
    let crate_dir = workspace(temp.path())?;
    let (built, _) = digest::source_digest(&crate_dir)?;

    // A build for another target, its tests and its benches change nothing.
    for (path, bytes) in [
        (
            "runx-a/target/aarch64-unknown-linux-gnu/release/runx",
            "a binary",
        ),
        (
            "runx-a/target/x86_64-pc-windows-msvc/release/runx.exe",
            "another",
        ),
        ("runx-b/tests/case.rs", "#[test] fn case() {}"),
        ("runx-b/benches/bench.rs", "fn main() {}"),
    ] {
        let path = temp.path().join(path);
        std::fs::create_dir_all(path.parent().ok_or("a file has a directory")?)?;
        std::fs::write(path, bytes)?;
    }
    assert_eq!(digest::source_digest(&crate_dir)?.0, built);

    // A changed source, in the crate or one it builds from, is another build.
    std::fs::write(
        temp.path().join("runx-b/src/lib.rs"),
        "// runx-b, changed\n",
    )?;
    assert_ne!(digest::source_digest(&crate_dir)?.0, built);
    Ok(())
}
