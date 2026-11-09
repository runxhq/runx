use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn license_boundary_guard_accepts_current_tree() -> Result<(), Box<dyn std::error::Error>> {
    let root = repo_root();
    assert_success(
        Command::new("node")
            .arg("scripts/check-license-edges.mjs")
            .arg("--check")
            .arg("manifest-complete")
            .current_dir(&root)
            .output()?,
    )?;
    assert_success(
        Command::new("node")
            .arg("scripts/check-license-edges.mjs")
            .arg("--check")
            .arg("identifiers")
            .current_dir(&root)
            .output()?,
    )?;
    assert_success(
        Command::new("sh")
            .arg("-c")
            .arg(
                "cargo metadata --manifest-path crates/Cargo.toml --format-version 1 | node scripts/check-license-edges.mjs --check edges",
            )
            .current_dir(&root)
            .output()?,
    )?;
    Ok(())
}

#[test]
fn license_boundary_guard_rejects_private_identifier_fixture()
-> Result<(), Box<dyn std::error::Error>> {
    let root = repo_root();
    let temp_dir = unique_temp_dir()?;
    fs::create_dir_all(&temp_dir)?;
    let fixture = root
        .join("crates/runx-runtime/tests/fixtures/license_boundary/private_broker_violation.rs");
    fs::copy(fixture, temp_dir.join("private_broker_violation.rs"))?;

    let output = Command::new("node")
        .arg("scripts/check-license-edges.mjs")
        .arg("--check")
        .arg("identifiers")
        .current_dir(&root)
        .env("RUNX_LICENSE_BOUNDARY_SCAN_ROOTS", &temp_dir)
        .output()?;

    let _ = fs::remove_dir_all(&temp_dir);
    assert!(
        !output.status.success(),
        "private identifier fixture must fail identifier scan"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("RunxPrivateBoundarySentinel"),
        "stderr should identify the banned private symbol"
    );
    Ok(())
}

#[test]
fn license_boundary_guard_rejects_private_dependency_edge_fixture()
-> Result<(), Box<dyn std::error::Error>> {
    let root = repo_root();
    let temp_dir = unique_temp_dir()?;
    fs::create_dir_all(&temp_dir)?;

    let manifest_path = temp_dir.join("license-boundary.manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("docs/license-boundary.manifest.json"))?)?;
    manifest["private_crate_names"] = serde_json::json!(["runx-private-auth"]);
    fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;

    let metadata = serde_json::json!({
        "packages": [
            { "id": "path+file:///runx-runtime#0.0.1", "name": "runx-runtime" },
            { "id": "path+file:///runx-private-auth#0.0.1", "name": "runx-private-auth" }
        ],
        "resolve": {
            "nodes": [
                {
                    "id": "path+file:///runx-runtime#0.0.1",
                    "deps": [{ "pkg": "path+file:///runx-private-auth#0.0.1" }]
                },
                { "id": "path+file:///runx-private-auth#0.0.1", "deps": [] }
            ]
        }
    })
    .to_string();

    let mut child = Command::new("node")
        .arg("scripts/check-license-edges.mjs")
        .arg("--check")
        .arg("edges")
        .current_dir(&root)
        .env("RUNX_LICENSE_BOUNDARY_MANIFEST", &manifest_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child
        .stdin
        .as_mut()
        .ok_or("edge check stdin should be piped")?;
    stdin.write_all(metadata.as_bytes())?;
    let output = child.wait_with_output()?;

    let _ = fs::remove_dir_all(&temp_dir);
    assert!(
        !output.status.success(),
        "private dependency edge fixture must fail edge scan"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("runx-runtime -> runx-private-auth"),
        "stderr should identify the forbidden dependency edge"
    );
    Ok(())
}

fn assert_success(output: std::process::Output) -> Result<(), Box<dyn std::error::Error>> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A directory no other test in this process shares: the sequence tells
/// this process's calls apart, the time a process that reuses its id.
fn unique_temp_dir() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    Ok(std::env::temp_dir().join(format!(
        "runx-license-boundary-{}-{nanos}-{sequence}",
        std::process::id()
    )))
}
