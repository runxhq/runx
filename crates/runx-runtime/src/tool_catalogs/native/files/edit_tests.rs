#![allow(clippy::expect_used)]

use std::fs;

use super::capability::{FileApplyBundleInput, FileEditInput, FileWriteInput};
use super::test_support::{TestResult, digest, invoke};
use super::{apply_files, edit, write};
use crate::services::WorkspaceBounds;

#[test]
fn an_edit_replaces_exact_text_once_or_everywhere_and_says_why_it_cannot() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    fs::write(root.join("lib.rs"), "fn one() {}\nfn two() {}\n")?;
    let edited = invoke::<FileEditInput, _>(
        root,
        serde_json::json!({"path": "lib.rs", "edits": [{"old": "fn one", "new": "fn first"}]}),
        WorkspaceBounds::Agent,
        edit,
    )?;
    assert_eq!(edited.replacements, 1);
    assert_eq!(edited.before_digest, digest("fn one() {}\nfn two() {}\n"));
    assert_eq!(
        fs::read_to_string(root.join("lib.rs"))?,
        "fn first() {}\nfn two() {}\n"
    );
    assert_eq!(
        edited.content_digest,
        digest("fn first() {}\nfn two() {}\n")
    );

    let refused = |edits: serde_json::Value| {
        invoke::<FileEditInput, _>(
            root,
            serde_json::json!({"path": "lib.rs", "edits": edits}),
            WorkspaceBounds::Agent,
            edit,
        )
        .err()
        .unwrap_or_default()
    };
    assert!(refused(serde_json::json!([{"old": "fn () {}", "new": "x"}])).contains("is not in"));
    assert!(refused(serde_json::json!([{"old": "() {}", "new": "()"}])).contains("occurs 2 times"));
    assert!(refused(serde_json::json!([{"old": "fn", "new": "fn"}])).contains("changes nothing"));
    assert!(refused(serde_json::json!([])).contains("at least one"));
    assert_eq!(
        fs::read_to_string(root.join("lib.rs"))?,
        "fn first() {}\nfn two() {}\n",
        "a refused edit changes nothing"
    );

    let all = invoke::<FileEditInput, _>(
        root,
        serde_json::json!({"path": "lib.rs", "edits": [{"old": "() {}", "new": "() -> u8 { 0 }", "replace_all": true}]}),
        WorkspaceBounds::Agent,
        edit,
    )?;
    assert_eq!(all.replacements, 2);
    assert!(
        invoke::<FileEditInput, _>(
            root,
            serde_json::json!({"path": "missing.rs", "edits": [{"old": "a", "new": "b"}]}),
            WorkspaceBounds::Agent,
            edit,
        )
        .is_err(),
        "an edit changes a file that exists"
    );
    Ok(())
}

#[test]
fn an_agents_write_names_what_it_replaces_and_a_stale_one_is_refused() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    fs::write(root.join("notes.md"), "read\n")?;
    let write_with = |extra: serde_json::Value, bounds| {
        let mut inputs = serde_json::json!({"path": "notes.md", "contents": "next\n"});
        if let (Some(inputs), Some(extra)) = (inputs.as_object_mut(), extra.as_object()) {
            inputs.extend(extra.clone());
        }
        invoke::<FileWriteInput, _>(root, inputs, bounds, write)
    };

    let unnamed = write_with(serde_json::json!({}), WorkspaceBounds::Agent)
        .err()
        .unwrap_or_default();
    // A change that names no digest is told so, and what to name.
    assert!(
        unnamed.contains("names no expected_digest") && unnamed.contains("content_digest"),
        "{unnamed}"
    );
    let stale = write_with(
        serde_json::json!({"expected_digest": digest("other\n")}),
        WorkspaceBounds::Agent,
    )
    .err()
    .unwrap_or_default();
    assert!(stale.contains("changed since it was read"), "{stale}");
    let created = write_with(serde_json::json!({"create": true}), WorkspaceBounds::Agent)
        .err()
        .unwrap_or_default();
    assert!(created.contains("exists"), "{created}");
    assert_eq!(fs::read_to_string(root.join("notes.md"))?, "read\n");

    write_with(
        serde_json::json!({"expected_digest": digest("read\n")}),
        WorkspaceBounds::Agent,
    )?;
    assert_eq!(fs::read_to_string(root.join("notes.md"))?, "next\n");
    // An operator's write names nothing, as before.
    write_with(serde_json::json!({}), WorkspaceBounds::Operator)?;

    let fresh = invoke::<FileWriteInput, _>(
        root,
        serde_json::json!({"path": "fresh.md", "contents": "new\n", "create": true}),
        WorkspaceBounds::Agent,
        write,
    );
    assert!(fresh.is_ok(), "{fresh:?}");
    Ok(())
}

#[test]
fn a_bundle_binds_its_writes_and_deletes() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    fs::write(root.join("keep.md"), "keep\n")?;
    fs::write(root.join("gone.md"), "gone\n")?;
    let unbound_delete = invoke::<FileApplyBundleInput, _>(
        root,
        serde_json::json!({"deletes": ["gone.md"]}),
        WorkspaceBounds::Agent,
        apply_files,
    );
    assert!(unbound_delete.is_err());
    let applied = invoke::<FileApplyBundleInput, _>(
        root,
        serde_json::json!({
            "writes": [{"path": "keep.md", "contents": "kept\n", "expected_digest": digest("keep\n")}],
            "deletes": [{"path": "gone.md", "expected_digest": digest("gone\n")}],
        }),
        WorkspaceBounds::Agent,
        apply_files,
    );
    assert!(applied.is_ok(), "{applied:?}");
    assert!(!root.join("gone.md").exists());
    assert_eq!(fs::read_to_string(root.join("keep.md"))?, "kept\n");
    Ok(())
}

#[test]
fn every_change_returns_the_diff_it_made() -> TestResult {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    fs::write(root.join("lib.rs"), "fn one() {}\n")?;
    let edited = invoke::<FileEditInput, _>(
        root,
        serde_json::json!({"path": "lib.rs", "edits": [{"old": "one", "new": "first"}]}),
        WorkspaceBounds::Agent,
        edit,
    )?;
    assert_eq!(
        edited.diff,
        "--- lib.rs\n+++ lib.rs\n@@ -1 +1 @@\n-fn one() {}\n+fn first() {}\n"
    );

    let created = invoke::<FileWriteInput, _>(
        root,
        serde_json::json!({"path": "new.md", "contents": "hello\n", "create": true}),
        WorkspaceBounds::Agent,
        write,
    )?;
    assert!(
        created.diff.starts_with("--- new.md\n+++ new.md\n") && created.diff.ends_with("+hello\n"),
        "{}",
        created.diff
    );

    let bundle = invoke::<FileApplyBundleInput, _>(
        root,
        serde_json::json!({
            "writes": [{"path": "new.md", "contents": "hello there\n", "expected_digest": digest("hello\n")}],
            "deletes": [{"path": "lib.rs", "expected_digest": digest("fn first() {}\n")}],
        }),
        WorkspaceBounds::Agent,
        apply_files,
    )?;
    let [write] = bundle.writes.as_slice() else {
        return Err("the bundle wrote one file".into());
    };
    assert_eq!(
        write.diff,
        "--- new.md\n+++ new.md\n@@ -1 +1 @@\n-hello\n+hello there\n"
    );
    let [delete] = bundle.deletes.as_slice() else {
        return Err("the bundle deleted one file".into());
    };
    assert!(
        delete.diff.starts_with("--- lib.rs\n+++ lib.rs\n")
            && delete.diff.ends_with("-fn first() {}\n"),
        "{}",
        delete.diff
    );
    Ok(())
}
