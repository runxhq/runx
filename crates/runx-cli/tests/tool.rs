use crate::support::{repo_root, unsigned_runx_command_with_inherited_cwd};
use std::fs;
use std::path::Path;

#[test]
fn tool_search_native_catalog_json() -> Result<(), Box<dyn std::error::Error>> {
    let output = unsigned_runx_command_with_inherited_cwd()
        .args(["tool", "search", "fs.read", "--source", "native", "--json"])
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains(r#""name": "fs.read""#));
    assert!(stdout.contains(r#""tool_id": "native/fs.read""#));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    Ok(())
}

#[test]
fn tool_inspect_native_catalog_json() -> Result<(), Box<dyn std::error::Error>> {
    let output = unsigned_runx_command_with_inherited_cwd()
        .args(["tool", "inspect", "fs.read", "--json"])
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains(r#""origin": "native""#));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    Ok(())
}

#[test]
fn tool_search_matches_native_oracle_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let output = unsigned_runx_command_with_inherited_cwd()
        .args(["tool", "search", "fs.read", "--source", "native", "--json"])
        .output()?;

    assert_oracle_output("search-native", &output, None)
}

#[test]
fn tool_inspect_matches_local_oracle_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let repo = repo_root()?;
    let output = unsigned_runx_command_with_inherited_cwd()
        .args(["tool", "inspect", "fixture.local_echo", "--json"])
        .env(
            "RUNX_TOOL_ROOTS",
            repo.join("fixtures/tool-catalogs/inspect/tool-roots/local"),
        )
        .output()?;

    assert_oracle_output("inspect-local-manifest", &output, Some(&repo))
}

#[test]
fn tool_inspect_matches_missing_oracle_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let output = unsigned_runx_command_with_inherited_cwd()
        .args(["tool", "inspect", "fixture.missing", "--json"])
        .output()?;

    assert_oracle_output("inspect-missing", &output, None)
}

fn assert_oracle_output(
    oracle_name: &str,
    output: &std::process::Output,
    repo_root: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = normalize_repo(String::from_utf8(output.stdout.clone())?, repo_root);
    let stderr = normalize_repo(String::from_utf8(output.stderr.clone())?, repo_root);
    let status = format!("{}\n", output.status.code().unwrap_or(1));

    assert_eq!(stdout, oracle_contents(&format!("{oracle_name}.stdout"))?);
    assert_eq!(stderr, oracle_contents(&format!("{oracle_name}.stderr"))?);
    assert_eq!(status, oracle_contents(&format!("{oracle_name}.status"))?);
    Ok(())
}

fn normalize_repo(contents: String, repo_root: Option<&Path>) -> String {
    repo_root.map_or(contents.clone(), |root| {
        contents.replace(&display(root), "<repo>")
    })
}

fn oracle_contents(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    optional_oracle_contents(name)?.ok_or_else(|| format!("missing oracle file: {name}").into())
}

fn optional_oracle_contents(name: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let path = repo_root()?
        .join("fixtures/tool-catalogs/oracles")
        .join(name);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(fs::read_to_string(path)?))
}

fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
