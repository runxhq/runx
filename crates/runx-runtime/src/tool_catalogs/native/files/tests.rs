#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;

use runx_contracts::{JsonObject, JsonValue};

use super::{
    FileApplyBundleInput, FileReadBundleInput, FileReadInput, FileWriteInput, apply_files, read,
    read_bundle, write,
};
use crate::RuntimeEffectRegistry;
use crate::credentials::CredentialDelivery;
use crate::paths::RUNX_CWD_ENV;
use crate::tool_catalogs::native::{NativeInvocation, fixture_input};

#[test]
fn reads_and_digests_one_contained_file() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    fs::write(workspace.path().join("profile.json"), "{\"ok\":true}\n")?;
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);
    let inputs = fixture_input::<FileReadInput>(JsonObject::from([(
        "path".to_owned(),
        JsonValue::String("profile.json".to_owned()),
    )]))?;
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let output = json_output(read(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: workspace.path(),
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        effects: &effects,
        reach: runx_contracts::Reach::Inputs,
        call: None,
    })?)?;
    let output = output.as_object().ok_or("missing output")?;
    assert_eq!(
        output.get("contents"),
        Some(&JsonValue::String("{\"ok\":true}\n".to_owned()))
    );
    assert!(
        output
            .get("content_digest")
            .and_then(JsonValue::as_str)
            .is_some_and(|value| value.starts_with("sha256:"))
    );
    Ok(())
}

#[test]
fn reads_and_digests_a_bounded_file_bundle() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    fs::write(workspace.path().join("one.txt"), "one\n")?;
    fs::write(workspace.path().join("two.txt"), "two\n")?;
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);
    let inputs = fixture_input::<FileReadBundleInput>(JsonObject::from([(
        "paths".to_owned(),
        JsonValue::Array(vec![
            JsonValue::String("one.txt".to_owned()),
            JsonValue::String("two.txt".to_owned()),
        ]),
    )]))?;
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let output = json_output(read_bundle(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: workspace.path(),
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        effects: &effects,
        reach: runx_contracts::Reach::Inputs,
        call: None,
    })?)?;
    let output = output.as_object().ok_or("missing output")?;

    assert_eq!(
        output.get("file_count"),
        Some(&JsonValue::Number(runx_contracts::JsonNumber::I64(2)))
    );
    assert_eq!(
        output.get("total_bytes"),
        Some(&JsonValue::Number(runx_contracts::JsonNumber::I64(8)))
    );
    assert_eq!(
        output
            .get("files")
            .and_then(JsonValue::as_array)
            .and_then(|files| files.get(1))
            .and_then(JsonValue::as_object)
            .and_then(|file| file.get("contents")),
        Some(&JsonValue::String("two\n".to_owned()))
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn rejects_a_symlink_that_escapes_the_root() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir()?;
    let outside = tempfile::NamedTempFile::new()?;
    symlink(outside.path(), workspace.path().join("escape"))?;
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);
    let inputs = fixture_input::<FileReadInput>(JsonObject::from([(
        "path".to_owned(),
        JsonValue::String("escape".to_owned()),
    )]))?;
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let error = read(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: workspace.path(),
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        effects: &effects,
        reach: runx_contracts::Reach::Inputs,
        call: None,
    })
    .expect_err("escaping symlink must be rejected");
    assert!(error.to_string().contains("escapes the workspace root"));
    Ok(())
}

#[test]
fn writes_and_proves_one_contained_file() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);
    let inputs = fixture_input::<FileWriteInput>(JsonObject::from([
        (
            "path".to_owned(),
            JsonValue::String("nested/generated.md".to_owned()),
        ),
        (
            "contents".to_owned(),
            JsonValue::String("generated fixture content\n".to_owned()),
        ),
    ]))?;
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let output = json_output(write(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: workspace.path(),
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        effects: &effects,
        reach: runx_contracts::Reach::Inputs,
        call: None,
    })?)?;
    let output = output.as_object().ok_or("missing output")?;

    assert_eq!(
        fs::read_to_string(workspace.path().join("nested/generated.md"))?,
        "generated fixture content\n"
    );
    assert_eq!(
        output.get("path"),
        Some(&JsonValue::String("nested/generated.md".to_owned()))
    );
    assert_eq!(
        output.get("bytes_written"),
        Some(&JsonValue::Number(runx_contracts::JsonNumber::I64(26)))
    );
    assert_eq!(
        output
            .get("content_digest")
            .and_then(JsonValue::as_str)
            .map(str::len),
        Some("sha256:".len() + 64)
    );
    Ok(())
}

#[test]
fn native_file_tools_cannot_change_kernel_state() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let root = workspace.path();
    // Named from the workspace root and by its absolute path inside it, a
    // kernel file is refused as kernel state either way.
    for absolute in [false, true] {
        for filename in [
            "kernel-v2.sqlite",
            "kernel-v2.sqlite-wal",
            "kernel-v2.sqlite-shm",
            ".receipt-store.lock",
            "..receipt-store.lock.tmp.123-456",
        ] {
            let path = format!("receipts/{filename}");
            let protected = root.join(&path);
            fs::create_dir_all(protected.parent().ok_or("missing parent")?)?;
            fs::write(&protected, "protected")?;
            let path = if absolute {
                protected.to_string_lossy().into_owned()
            } else {
                path
            };
            let write_inputs = fixture_input::<FileWriteInput>(JsonObject::from([
                ("path".to_owned(), JsonValue::String(path.clone())),
                (
                    "contents".to_owned(),
                    JsonValue::String("forged".to_owned()),
                ),
            ]))?;
            let refused = write(&NativeInvocation {
                inputs: &write_inputs,
                observed_at: "2026-01-01T00:00:00Z",
                env: &env,
                skill_directory: workspace.path(),
                credential_delivery: &delivery,
                local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
                effects: &effects,
                reach: runx_contracts::Reach::Inputs,
                call: None,
            })
            .err()
            .ok_or("a file tool wrote kernel state")?;
            assert!(
                refused
                    .to_string()
                    .contains("kernel state cannot be changed"),
                "{refused}"
            );
            for (writes, deletes) in [
                (
                    vec![JsonValue::Object(JsonObject::from([
                        ("path".to_owned(), JsonValue::String(path.clone())),
                        (
                            "contents".to_owned(),
                            JsonValue::String("forged".to_owned()),
                        ),
                    ]))],
                    Vec::new(),
                ),
                (
                    Vec::new(),
                    vec![JsonValue::Object(JsonObject::from([
                        ("path".to_owned(), JsonValue::String(path.clone())),
                        (
                            "expected_digest".to_owned(),
                            JsonValue::String(runx_contracts::sha256_prefixed(b"protected")),
                        ),
                    ]))],
                ),
            ] {
                let bundle_inputs = fixture_input::<FileApplyBundleInput>(JsonObject::from([
                    ("writes".to_owned(), JsonValue::Array(writes)),
                    ("deletes".to_owned(), JsonValue::Array(deletes)),
                ]))?;
                assert!(
                    apply_files(&NativeInvocation {
                        inputs: &bundle_inputs,
                        observed_at: "2026-01-01T00:00:00Z",
                        env: &env,
                        skill_directory: workspace.path(),
                        credential_delivery: &delivery,
                        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
                        effects: &effects,
                        reach: runx_contracts::Reach::Inputs,
                        call: None,
                    })
                    .is_err()
                );
            }
            assert_eq!(fs::read_to_string(&protected)?, "protected");
            fs::remove_file(&protected)?;
            assert!(
                write(&NativeInvocation {
                    inputs: &write_inputs,
                    observed_at: "2026-01-01T00:00:00Z",
                    env: &env,
                    skill_directory: workspace.path(),
                    credential_delivery: &delivery,
                    local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
                    effects: &effects,
                    reach: runx_contracts::Reach::Inputs,
                    call: None,
                })
                .is_err()
            );
            assert!(!protected.exists());
        }
    }
    Ok(())
}

/// A kernel file that is a link to another file is still kernel state by its
/// name: naming it by its absolute path never swaps in the file it links to.
#[cfg(unix)]
#[test]
fn a_linked_kernel_file_is_refused_by_the_name_it_was_given()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let root = fs::canonicalize(workspace.path())?;
    fs::create_dir_all(root.join("receipts"))?;
    fs::write(root.join("state.sqlite"), "protected")?;
    std::os::unix::fs::symlink(
        root.join("state.sqlite"),
        root.join("receipts/kernel-v2.sqlite"),
    )?;
    let env = BTreeMap::from([(RUNX_CWD_ENV.to_owned(), root.to_string_lossy().into_owned())]);
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    for path in [
        "receipts/kernel-v2.sqlite".to_owned(),
        root.join("receipts/kernel-v2.sqlite")
            .to_string_lossy()
            .into_owned(),
    ] {
        let inputs = fixture_input::<FileWriteInput>(JsonObject::from([
            ("path".to_owned(), JsonValue::String(path.clone())),
            (
                "contents".to_owned(),
                JsonValue::String("forged".to_owned()),
            ),
            (
                "expected_digest".to_owned(),
                JsonValue::String(runx_contracts::sha256_prefixed(b"protected")),
            ),
        ]))?;
        let refused = write(&NativeInvocation {
            inputs: &inputs,
            observed_at: "2026-01-01T00:00:00Z",
            env: &env,
            skill_directory: &root,
            credential_delivery: &delivery,
            local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
            effects: &effects,
            reach: runx_contracts::Reach::Inputs,
            call: None,
        })
        .err()
        .ok_or("a file tool wrote through a linked kernel file")?;
        assert!(
            refused
                .to_string()
                .contains("kernel state cannot be changed"),
            "{path}: {refused}"
        );
        assert_eq!(fs::read_to_string(root.join("state.sqlite"))?, "protected");
    }
    Ok(())
}

#[test]
fn a_write_stays_in_the_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let elsewhere = tempfile::tempdir()?;
    let package = workspace.path().join("skills/own");
    fs::create_dir_all(&package)?;
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    // The path a write names back, or why it was refused.
    let write_to = |path: String| -> Result<Result<String, String>, Box<dyn std::error::Error>> {
        let inputs = fixture_input::<FileWriteInput>(JsonObject::from([
            ("path".to_owned(), JsonValue::String(path)),
            (
                "contents".to_owned(),
                JsonValue::String("written".to_owned()),
            ),
        ]))?;
        Ok(write(&NativeInvocation {
            inputs: &inputs,
            observed_at: "2026-01-01T00:00:00Z",
            env: &env,
            skill_directory: &package,
            credential_delivery: &delivery,
            local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
            effects: &effects,
            reach: runx_contracts::Reach::Inputs,
            call: None,
        })
        .map(|written| written.path.as_str().to_owned())
        .map_err(|error| error.to_string()))
    };
    // Its absolute path inside the workspace is that file, named from the root.
    let inside = workspace.path().join("notes/new.txt");
    let written = write_to(inside.to_string_lossy().into_owned())??;
    assert_eq!(written, "notes/new.txt");
    assert_eq!(fs::read_to_string(&inside)?, "written");
    // Anywhere else, and the skill's own package, are refused with why.
    for (path, why) in [
        (
            elsewhere
                .path()
                .join("new.txt")
                .to_string_lossy()
                .into_owned(),
            "is outside the workspace",
        ),
        (
            "$package/new.txt".to_owned(),
            "names the skill's own package",
        ),
    ] {
        let refused = write_to(path.clone())?
            .err()
            .ok_or("a write left the workspace")?;
        assert!(refused.contains(why), "{path}: {refused}");
    }
    assert!(!elsewhere.path().join("new.txt").exists());
    assert!(!package.join("new.txt").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn file_tool_cannot_replace_lock_while_a_receipt_write_waits()
-> Result<(), Box<dyn std::error::Error>> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::MetadataExt;
    use std::sync::{Arc, Barrier, mpsc};

    use fs2::FileExt;

    let workspace = tempfile::tempdir()?;
    let receipt_dir = workspace.path().join("receipts");
    fs::create_dir_all(&receipt_dir)?;
    let signature = crate::receipts::RuntimeReceiptSignatureConfig::fixture()?;
    crate::LocalReceiptStore::new(&receipt_dir).rebuild_index(signature.signature_policy())?;
    let lock_path = receipt_dir.join(".receipt-store.lock");
    let lock = OpenOptions::new().read(true).write(true).open(&lock_path)?;
    lock.lock_exclusive()?;
    let original_inode = fs::metadata(&lock_path)?.ino();

    let started = Arc::new(Barrier::new(2));
    let (sender, receiver) = mpsc::channel();
    let worker_root = receipt_dir.clone();
    let worker_started = Arc::clone(&started);
    let worker_signature = signature.clone();
    let worker = std::thread::spawn(move || {
        worker_started.wait();
        let result = crate::LocalReceiptStore::new(&worker_root)
            .rebuild_index(worker_signature.signature_policy());
        let _ignored = sender.send(result);
    });
    started.wait();

    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let inputs = fixture_input::<FileApplyBundleInput>(JsonObject::from([
        ("writes".to_owned(), JsonValue::Array(Vec::new())),
        (
            "deletes".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                (
                    "path".to_owned(),
                    JsonValue::String("receipts/.receipt-store.lock".to_owned()),
                ),
                (
                    "expected_digest".to_owned(),
                    JsonValue::String(runx_contracts::sha256_prefixed(&fs::read(
                        workspace.path().join("receipts/.receipt-store.lock"),
                    )?)),
                ),
            ]))]),
        ),
    ]))?;
    assert!(
        apply_files(&NativeInvocation {
            inputs: &inputs,
            observed_at: "2026-01-01T00:00:00Z",
            env: &env,
            skill_directory: workspace.path(),
            credential_delivery: &delivery,
            local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
            effects: &effects,
            reach: runx_contracts::Reach::Inputs,
            call: None,
        })
        .is_err()
    );
    assert_eq!(fs::metadata(&lock_path)?.ino(), original_inode);
    // The test holds the lock, so the worker's write hasn't happened; once
    // it's released, the worker writes and answers.
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    FileExt::unlock(&lock)?;
    receiver.recv()??;
    worker.join().map_err(|_| "receipt worker panicked")?;
    assert_eq!(fs::metadata(&lock_path)?.ino(), original_inode);
    Ok(())
}

#[cfg(unix)]
#[test]
fn write_rejects_a_symlink_that_escapes_the_root() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    symlink(outside.path(), workspace.path().join("escape"))?;
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);
    let inputs = fixture_input::<FileWriteInput>(JsonObject::from([
        (
            "path".to_owned(),
            JsonValue::String("escape/generated.md".to_owned()),
        ),
        (
            "contents".to_owned(),
            JsonValue::String("blocked".to_owned()),
        ),
    ]))?;
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let error = write(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: workspace.path(),
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        effects: &effects,
        reach: runx_contracts::Reach::Inputs,
        call: None,
    })
    .expect_err("escaping symlink must be rejected");

    assert!(error.to_string().contains("fs.write"));
    assert!(!outside.path().join("generated.md").exists());
    Ok(())
}

fn json_output(output: impl serde::Serialize) -> Result<JsonValue, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(serde_json::to_value(output)?)?)
}
