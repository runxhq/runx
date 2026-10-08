use std::collections::BTreeMap;
use std::process::Command;

use runx_contracts::{JsonObject, JsonValue};

use super::{
    GitBlobDigestInput, GitDiffInput, GitInput, blob_digest, current_branch, diff_name_only, status,
};
#[cfg(feature = "catalog")]
use crate::RuntimeEffectRegistry;
use crate::credentials::CredentialDelivery;
use crate::receipts::paths::RUNX_CWD_ENV;
use crate::tool_catalogs::native::NativeInvocation;

#[test]
fn reads_named_and_detached_head() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    git(workspace.path(), &["init", "-b", "main"])?;
    git(
        workspace.path(),
        &["config", "user.email", "runx@example.invalid"],
    )?;
    git(workspace.path(), &["config", "user.name", "Runx Test"])?;
    std::fs::write(workspace.path().join("README.md"), "# Fixture\n")?;
    git(workspace.path(), &["add", "README.md"])?;
    git(workspace.path(), &["commit", "-m", "fixture"])?;

    let named = invoke_current_branch(workspace.path())?;
    assert_eq!(
        named.get("branch"),
        Some(&JsonValue::String("main".to_owned()))
    );
    assert_eq!(named.get("detached"), Some(&JsonValue::Bool(false)));

    git(workspace.path(), &["checkout", "--detach", "HEAD"])?;
    let detached = invoke_current_branch(workspace.path())?;
    assert_eq!(detached.get("detached"), Some(&JsonValue::Bool(true)));
    assert_eq!(
        detached
            .get("branch")
            .and_then(JsonValue::as_str)
            .map(str::len),
        Some(7)
    );
    Ok(())
}

#[test]
fn reports_status_and_changed_files_without_local_wrappers()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    git(workspace.path(), &["init", "-b", "main"])?;
    git(
        workspace.path(),
        &["config", "user.email", "runx@example.invalid"],
    )?;
    git(workspace.path(), &["config", "user.name", "Runx Test"])?;
    std::fs::write(workspace.path().join("README.md"), "original\n")?;
    git(workspace.path(), &["add", "README.md"])?;
    git(workspace.path(), &["commit", "-m", "fixture"])?;
    std::fs::write(workspace.path().join("README.md"), "changed\n")?;

    let status = invoke_tool(
        workspace.path(),
        GitInput {
            repo_root: ".".to_owned(),
        },
        status,
    )?;
    assert_eq!(status.get("clean"), Some(&JsonValue::Bool(false)));
    assert_eq!(
        status.get("entries"),
        Some(&JsonValue::Array(vec![JsonValue::String(
            " M README.md".to_owned()
        )]))
    );

    let diff = invoke_tool(
        workspace.path(),
        GitDiffInput {
            repo_root: ".".to_owned(),
            base: "HEAD".to_owned(),
        },
        diff_name_only,
    )?;
    assert_eq!(
        diff.get("files"),
        Some(&JsonValue::Array(vec![JsonValue::String(
            "README.md".to_owned()
        )]))
    );
    Ok(())
}

#[test]
fn computes_the_canonical_git_blob_identity_without_a_process()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let result = invoke_tool(
        workspace.path(),
        GitBlobDigestInput {
            contents: "hello\n".to_owned(),
        },
        blob_digest,
    )?;
    let digest = result
        .get("git_blob_digest")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| std::io::Error::other("missing git blob digest"))?;
    assert_eq!(
        digest.get("digest"),
        Some(&JsonValue::String(
            "ce013625030ba8dba906f756967f9e9ca394464a".to_owned()
        ))
    );
    assert_eq!(
        digest.get("bytes").and_then(|value| match value {
            JsonValue::Number(number) => number.as_f64(),
            _ => None,
        }),
        Some(6.0)
    );
    Ok(())
}

fn invoke_current_branch(root: &std::path::Path) -> Result<JsonObject, Box<dyn std::error::Error>> {
    invoke_tool(
        root,
        GitInput {
            repo_root: ".".to_owned(),
        },
        current_branch,
    )
}

fn invoke_tool<I, O: serde::Serialize>(
    root: &std::path::Path,
    inputs: I,
    tool: for<'a> fn(&NativeInvocation<'a, I>) -> Result<O, crate::RuntimeError>,
) -> Result<JsonObject, Box<dyn std::error::Error>> {
    let env = BTreeMap::from([(RUNX_CWD_ENV.to_owned(), root.to_string_lossy().into_owned())]);
    let delivery = CredentialDelivery::none();
    #[cfg(feature = "catalog")]
    let effects = RuntimeEffectRegistry::default();
    let output = tool(&NativeInvocation {
        inputs: &inputs,
        observed_at: "2026-01-01T00:00:00Z",
        env: &env,
        skill_directory: root,
        credential_delivery: &delivery,
        local_artifacts: crate::tool_catalogs::native::fixture_local_artifacts(),
        #[cfg(feature = "catalog")]
        effects: &effects,
    })?;
    let value: JsonValue = serde_json::from_value(serde_json::to_value(output)?)?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "missing output".into())
}

fn git(root: &std::path::Path, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()?;
    if !status.success() {
        return Err(format!("git command failed: {args:?}").into());
    }
    Ok(())
}
