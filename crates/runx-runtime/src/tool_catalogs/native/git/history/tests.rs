#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use runx_contracts::{JsonNumber, JsonObject, JsonValue};

use super::super::capability::{GitLogInput, GitPatchInput, GitShowInput};
use super::{diff, log, show};
use crate::RuntimeEffectRegistry;
use crate::credentials::CredentialDelivery;
use crate::paths::RUNX_CWD_ENV;
use crate::services::WorkspaceBounds;
use crate::tool_catalogs::native::{NativeInvocation, fixture_input};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn git(root: &Path, args: &[&str]) -> TestResult {
    let status = Command::new("git").args(args).current_dir(root).status()?;
    if !status.success() {
        return Err(format!("git {args:?} failed").into());
    }
    Ok(())
}

/// A repository of two commits: the second changes a source file and adds a
/// secret beside it.
fn repository() -> TestResult<tempfile::TempDir> {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    git(root, &["init", "-q", "-b", "main"])?;
    git(root, &["config", "user.email", "runx@example.invalid"])?;
    git(root, &["config", "user.name", "Runx Test"])?;
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("src/lib.rs"), "pub fn one() {}\n")?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-q", "-m", "first"])?;
    std::fs::write(root.join("src/lib.rs"), "pub fn two() {}\n")?;
    std::fs::write(root.join(".env"), "TOKEN=hunter2\n")?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-q", "-m", "second"])?;
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

fn strings(output: &JsonObject, field: &str) -> Vec<String> {
    output
        .get(field)
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(JsonValue::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn text<'a>(output: &'a JsonObject, field: &str) -> &'a str {
    output
        .get(field)
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
}

#[test]
fn a_log_lists_commits_newest_first_with_their_files() -> TestResult {
    let workspace = repository()?;
    let output = invoke::<GitLogInput, _>(
        workspace.path(),
        JsonObject::new(),
        WorkspaceBounds::Operator,
        log,
    )?;
    let commits = output
        .get("commits")
        .and_then(JsonValue::as_array)
        .ok_or("commits")?;
    assert_eq!(commits.len(), 2);
    let newest = commits[0].as_object().ok_or("a commit")?;
    assert_eq!(
        newest.get("subject"),
        Some(&JsonValue::String("second".to_owned()))
    );
    assert_eq!(
        strings(newest, "files"),
        vec![".env".to_owned(), "src/lib.rs".to_owned()]
    );
    let one = invoke::<GitLogInput, _>(
        workspace.path(),
        JsonObject::from([(
            "max_count".to_owned(),
            JsonValue::Number(JsonNumber::U64(1)),
        )]),
        WorkspaceBounds::Operator,
        log,
    )?;
    assert_eq!(
        one.get("commits")
            .and_then(JsonValue::as_array)
            .map(Vec::len),
        Some(1)
    );
    Ok(())
}

#[test]
fn a_commit_shown_within_agent_bounds_leaves_its_secret_out() -> TestResult {
    let workspace = repository()?;
    let operator = invoke::<GitShowInput, _>(
        workspace.path(),
        JsonObject::new(),
        WorkspaceBounds::Operator,
        show,
    )?;
    assert!(text(&operator, "text").contains("hunter2"));

    let agent = invoke::<GitShowInput, _>(
        workspace.path(),
        JsonObject::new(),
        WorkspaceBounds::Agent,
        show,
    )?;
    assert!(!text(&agent, "text").contains("hunter2"));
    assert!(text(&agent, "text").contains("pub fn two()"));
    assert_eq!(strings(&agent, "files"), vec!["src/lib.rs".to_owned()]);
    assert_eq!(strings(&agent, "omitted"), vec![".env".to_owned()]);
    Ok(())
}

#[test]
fn a_file_at_a_revision_is_shown_unless_it_is_a_secret() -> TestResult {
    let workspace = repository()?;
    let earlier = invoke::<GitShowInput, _>(
        workspace.path(),
        JsonObject::from([
            (
                "revision".to_owned(),
                JsonValue::String("HEAD~1".to_owned()),
            ),
            (
                "path".to_owned(),
                JsonValue::String("src/lib.rs".to_owned()),
            ),
        ]),
        WorkspaceBounds::Agent,
        show,
    )?;
    assert_eq!(text(&earlier, "text"), "pub fn one() {}\n");
    let secret = invoke::<GitShowInput, _>(
        workspace.path(),
        JsonObject::from([("path".to_owned(), JsonValue::String(".env".to_owned()))]),
        WorkspaceBounds::Agent,
        show,
    );
    assert!(secret.is_err());
    Ok(())
}

#[test]
fn a_patch_against_the_working_tree_or_a_revision_leaves_secrets_out() -> TestResult {
    let workspace = repository()?;
    std::fs::write(workspace.path().join("src/lib.rs"), "pub fn three() {}\n")?;
    std::fs::write(workspace.path().join(".env"), "TOKEN=changed\n")?;
    let working = invoke::<GitPatchInput, _>(
        workspace.path(),
        JsonObject::new(),
        WorkspaceBounds::Agent,
        diff,
    )?;
    assert!(text(&working, "patch").contains("+pub fn three() {}"));
    assert!(!text(&working, "patch").contains("TOKEN"));
    assert_eq!(strings(&working, "omitted"), vec![".env".to_owned()]);

    let between = invoke::<GitPatchInput, _>(
        workspace.path(),
        JsonObject::from([
            ("base".to_owned(), JsonValue::String("HEAD~1".to_owned())),
            ("head".to_owned(), JsonValue::String("HEAD".to_owned())),
            ("stat".to_owned(), JsonValue::Bool(true)),
        ]),
        WorkspaceBounds::Agent,
        diff,
    )?;
    assert!(text(&between, "patch").contains("src/lib.rs"));
    assert!(!text(&between, "patch").contains(".env"));
    Ok(())
}

#[test]
fn an_option_or_a_path_outside_the_repository_is_refused() -> TestResult {
    let workspace = repository()?;
    for (field, value) in [("revision", "--output=/tmp/x"), ("since", "")] {
        let refused = invoke::<GitLogInput, _>(
            workspace.path(),
            JsonObject::from([(field.to_owned(), JsonValue::String(value.to_owned()))]),
            WorkspaceBounds::Agent,
            log,
        );
        assert!(refused.is_err(), "{field}={value} is refused");
    }
    let outside = invoke::<GitPatchInput, _>(
        workspace.path(),
        JsonObject::from([(
            "paths".to_owned(),
            JsonValue::Array(vec![JsonValue::String("../elsewhere".to_owned())]),
        )]),
        WorkspaceBounds::Agent,
        diff,
    );
    assert!(outside.is_err());
    Ok(())
}
