#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::super::capability::{FileListInput, FileSearchInput};
use super::{list, search};
use crate::RuntimeEffectRegistry;
use crate::credentials::CredentialDelivery;
use crate::paths::RUNX_CWD_ENV;
use crate::services::WorkspaceBounds;
use crate::tool_catalogs::native::{NativeInvocation, fixture_input};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A project with an ignore file, a protected directory, a secret, a nested
/// worktree and a binary file.
fn project() -> TestResult<tempfile::TempDir> {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    fs::create_dir_all(root.join("src/deep/deeper"))?;
    fs::create_dir_all(root.join("build"))?;
    fs::create_dir_all(root.join(".git"))?;
    fs::create_dir_all(root.join(".runx"))?;
    fs::create_dir_all(root.join(".worktrees/feature/src"))?;
    fs::write(root.join(".gitignore"), "build/\n")?;
    fs::write(root.join("src/main.rs"), "fn main() {\n    run();\n}\n")?;
    fs::write(root.join("src/deep/deeper/notes.md"), "run the tests\n")?;
    fs::write(root.join("build/out.txt"), "run output\n")?;
    fs::write(root.join(".git/config"), "run = secret\n")?;
    fs::write(root.join(".runx/state"), "run state\n")?;
    fs::write(root.join(".env"), "RUN_TOKEN=1\n")?;
    fs::write(root.join(".worktrees/feature/.git"), "gitdir: ../../.git\n")?;
    fs::write(root.join(".worktrees/feature/src/main.rs"), "fn run() {}\n")?;
    fs::write(root.join("logo.bin"), b"run\0\x01\x02")?;
    Ok(workspace)
}

fn invoke<I, O>(
    workspace: &Path,
    inputs: JsonObject,
    bounds: WorkspaceBounds,
    handler: fn(&NativeInvocation<'_, I>) -> Result<O, crate::RuntimeError>,
) -> TestResult<JsonObject>
where
    I: crate::CapabilityInput,
    O: serde::Serialize,
{
    let env = BTreeMap::from([(
        RUNX_CWD_ENV.to_owned(),
        workspace.to_string_lossy().into_owned(),
    )]);
    let inputs = fixture_input::<I>(inputs)?;
    let delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let output = handler(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: workspace,
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        effects: &effects,
        bounds,
    })?;
    let value: JsonValue = serde_json::from_value(serde_json::to_value(output)?)?;
    Ok(value.as_object().ok_or("an object output")?.clone())
}

fn paths(output: &JsonObject, list: &str, field: &str) -> Vec<String> {
    output
        .get(list)
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_object()
                        .and_then(|item| item.get(field))
                        .and_then(JsonValue::as_str)
                })
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn number(value: u64) -> JsonValue {
    JsonValue::Number(JsonNumber::U64(value))
}

#[test]
fn a_listing_honours_ignore_files_and_skips_protected_directories_and_worktrees() -> TestResult {
    let workspace = project()?;
    let output = invoke::<FileListInput, _>(
        workspace.path(),
        JsonObject::from([("depth".to_owned(), number(16))]),
        WorkspaceBounds::Operator,
        list,
    )?;
    let listed = paths(&output, "entries", "path");
    assert!(listed.contains(&"src/main.rs".to_owned()));
    assert!(listed.contains(&"src/deep/deeper/notes.md".to_owned()));
    assert!(listed.contains(&".gitignore".to_owned()));
    for absent in [
        "build",
        "build/out.txt",
        ".git",
        ".runx",
        ".worktrees/feature",
    ] {
        assert!(
            !listed.contains(&absent.to_owned()),
            "{absent} is not listed"
        );
    }
    // An operator's run still lists `.env`; an agent's never does.
    assert!(listed.contains(&".env".to_owned()));
    let bounded = invoke::<FileListInput, _>(
        workspace.path(),
        JsonObject::from([("depth".to_owned(), number(16))]),
        WorkspaceBounds::Agent,
        list,
    )?;
    assert!(!paths(&bounded, "entries", "path").contains(&".env".to_owned()));
    Ok(())
}

#[test]
fn a_listing_is_bounded_by_depth_and_count() -> TestResult {
    let workspace = project()?;
    let shallow = invoke::<FileListInput, _>(
        workspace.path(),
        JsonObject::from([("depth".to_owned(), number(1))]),
        WorkspaceBounds::Operator,
        list,
    )?;
    let listed = paths(&shallow, "entries", "path");
    assert!(listed.contains(&"src".to_owned()));
    assert!(!listed.contains(&"src/main.rs".to_owned()));
    let capped = invoke::<FileListInput, _>(
        workspace.path(),
        JsonObject::from([
            ("depth".to_owned(), number(16)),
            ("max_entries".to_owned(), number(2)),
        ]),
        WorkspaceBounds::Operator,
        list,
    )?;
    assert_eq!(paths(&capped, "entries", "path").len(), 2);
    assert_eq!(capped.get("truncated"), Some(&JsonValue::Bool(true)));
    Ok(())
}

#[test]
fn a_listing_inside_a_worktree_lists_it() -> TestResult {
    let workspace = project()?;
    let output = invoke::<FileListInput, _>(
        workspace.path(),
        JsonObject::from([
            (
                "path".to_owned(),
                JsonValue::String(".worktrees/feature".to_owned()),
            ),
            ("depth".to_owned(), number(4)),
        ]),
        WorkspaceBounds::Agent,
        list,
    )?;
    assert!(
        paths(&output, "entries", "path").contains(&".worktrees/feature/src/main.rs".to_owned())
    );
    Ok(())
}

#[test]
fn a_search_finds_text_with_context_and_digests_and_skips_what_a_listing_skips() -> TestResult {
    let workspace = project()?;
    let output = invoke::<FileSearchInput, _>(
        workspace.path(),
        JsonObject::from([("pattern".to_owned(), JsonValue::String("run".to_owned()))]),
        WorkspaceBounds::Agent,
        search,
    )?;
    let found = paths(&output, "matches", "path");
    assert!(found.contains(&"src/main.rs".to_owned()));
    assert!(found.contains(&"src/deep/deeper/notes.md".to_owned()));
    for absent in [
        "build/out.txt",
        ".git/config",
        ".runx/state",
        ".env",
        ".worktrees/feature/src/main.rs",
        "logo.bin",
    ] {
        assert!(
            !found.contains(&absent.to_owned()),
            "{absent} is not searched"
        );
    }
    let first = output
        .get("matches")
        .and_then(JsonValue::as_array)
        .and_then(|matches| {
            matches
                .iter()
                .filter_map(JsonValue::as_object)
                .find(|item| item.get("path") == Some(&JsonValue::String("src/main.rs".to_owned())))
        })
        .ok_or("src/main.rs matched")?;
    assert_eq!(first.get("line"), Some(&number(2)));
    assert_eq!(
        first.get("before"),
        Some(&JsonValue::Array(vec![JsonValue::String(
            "fn main() {".to_owned()
        )]))
    );
    assert!(
        first
            .get("content_digest")
            .and_then(JsonValue::as_str)
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    Ok(())
}

#[test]
fn a_search_takes_a_regex_a_glob_and_a_limit() -> TestResult {
    let workspace = project()?;
    let regex = invoke::<FileSearchInput, _>(
        workspace.path(),
        JsonObject::from([
            (
                "pattern".to_owned(),
                JsonValue::String(r"^fn \w+\(\)".to_owned()),
            ),
            ("regex".to_owned(), JsonValue::Bool(true)),
            ("glob".to_owned(), JsonValue::String("**/*.rs".to_owned())),
        ]),
        WorkspaceBounds::Operator,
        search,
    )?;
    assert_eq!(
        paths(&regex, "matches", "path"),
        vec!["src/main.rs".to_owned()]
    );
    let limited = invoke::<FileSearchInput, _>(
        workspace.path(),
        JsonObject::from([
            ("pattern".to_owned(), JsonValue::String("RUN".to_owned())),
            ("case_sensitive".to_owned(), JsonValue::Bool(false)),
            ("max_results".to_owned(), number(1)),
        ]),
        WorkspaceBounds::Operator,
        search,
    )?;
    assert_eq!(paths(&limited, "matches", "path").len(), 1);
    assert_eq!(limited.get("truncated"), Some(&JsonValue::Bool(true)));
    Ok(())
}

#[test]
fn agent_bounds_refuse_a_start_outside_or_protected() -> TestResult {
    let workspace = project()?;
    for path in ["..", ".git", ".runx"] {
        let refused = invoke::<FileListInput, _>(
            workspace.path(),
            JsonObject::from([("path".to_owned(), JsonValue::String(path.to_owned()))]),
            WorkspaceBounds::Agent,
            list,
        );
        assert!(refused.is_err(), "{path} is refused");
    }
    Ok(())
}
