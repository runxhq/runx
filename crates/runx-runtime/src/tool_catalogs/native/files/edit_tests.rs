#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue};

use super::capability::{FileApplyBundleInput, FileEditInput, FileWriteInput};
use super::{apply_files, edit, write};
use crate::RuntimeEffectRegistry;
use crate::credentials::CredentialDelivery;
use crate::paths::RUNX_CWD_ENV;
use crate::services::WorkspaceBounds;
use crate::tool_catalogs::native::{NativeInvocation, fixture_input};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn invoke<I, O>(
    workspace: &Path,
    inputs: serde_json::Value,
    bounds: WorkspaceBounds,
    handler: fn(&NativeInvocation<'_, I>) -> Result<O, crate::RuntimeError>,
) -> Result<O, String>
where
    I: crate::CapabilityInput,
{
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.to_string_lossy().into_owned(),
    )]);
    let inputs: JsonObject = serde_json::from_value(inputs).map_err(|error| error.to_string())?;
    let inputs = fixture_input::<I>(inputs).map_err(|error| error.to_string())?;
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    handler(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: workspace,
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        effects: &effects,
        bounds,
    })
    .map_err(|error| error.to_string())
}

fn digest(text: &str) -> String {
    runx_contracts::sha256_prefixed(text.as_bytes())
}

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
    assert!(unnamed.contains("expected_digest"), "{unnamed}");
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
    let _ = JsonValue::Null;
    Ok(())
}
