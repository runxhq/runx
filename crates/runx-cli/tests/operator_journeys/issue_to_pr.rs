//! Native checkout proof for composed issue work. The required governance
//! verifier is an explicit process stub; this is not signature verification.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use serde_json::{Value, json};

use super::{TestResult, assert_json, output_with_stdin};

#[test]
fn completed_issue_work_checks_the_real_checkout_before_its_required_verifier() -> TestResult {
    let root = crate::support::temp_root("runx-issue-composed-real-checkout");
    let checkout = root.join("checkout");
    let home = root.join("home");
    let bin = root.join("bin");
    fs::create_dir_all(&checkout)?;
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&bin)?;
    let git = crate::support::find_program("git").ok_or("Git is required for checkout proof")?;
    let path = std::env::join_paths([
        bin.clone(),
        git.parent().ok_or("Git has no directory")?.to_path_buf(),
        "/usr/bin".into(),
        "/bin".into(),
    ])?;
    let git_run = |args: &[&str]| -> TestResult<String> {
        let result = Command::new(&git)
            .args(args)
            .current_dir(&checkout)
            .env_clear()
            .env("PATH", &path)
            .env("HOME", &home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(String::from_utf8(result.stdout)?.trim().to_owned())
    };
    git_run(&["init", "-b", "fix/442-operator-first"])?;
    fs::write(checkout.join("README.md"), "A real candidate checkout.\n")?;
    fs::write(checkout.join(".gitignore"), ".scafld/\n")?;
    git_run(&["add", "README.md", ".gitignore"])?;
    git_run(&["diff", "--cached", "--check"])?;
    git_run(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-m",
        "test(fixture): create candidate",
    ])?;
    let head = git_run(&["rev-parse", "HEAD"])?;
    fs::create_dir_all(checkout.join(".scafld/receipts"))?;
    let receipt = json!({"body": {
        "task_id": "issue-fixture", "verdict": "pass", "head_commit": head,
        "spec_fingerprint": "b".repeat(64), "open_blockers": []
    }});
    fs::write(
        checkout.join(".scafld/receipts/fixture.json"),
        receipt.to_string(),
    )?;
    let verifier = bin.join("scafld");
    fs::write(
        &verifier,
        r#"#!/bin/sh
set -eu
[ "$#" -eq 7 ] && [ "$1" = verify ] && [ "$3" = --target ] && [ "$5" = --root ] && [ "$7" = --json ]
[ "$2" = "$6/.scafld/receipts/fixture.json" ]
[ "$4" = "$(git -C "$6" rev-parse HEAD)" ]
printf '%s\n' "$4" >> "$6/.scafld/verifier-calls"
printf '%s\n' '{"ok":true,"command":"verify"}'
"#,
    )?;
    fs::set_permissions(&verifier, fs::Permissions::from_mode(0o755))?;
    let repo = crate::support::repo_root()?;
    let mut input: Value = serde_json::from_str(&fs::read_to_string(
        repo.join("skills/github-issue-to-pr/evals/files/issue-442-finalized-work.json"),
    )?)?;
    input
        .as_object_mut()
        .ok_or("issue fixture is an object")?
        .remove("about");
    input["host_result"]["commit"] = json!(head);
    input["host_result"]["files"] = json!(["README.md"]);
    input["host_result"]["tests"] = json!([{
        "command": "git diff --cached --check", "status": "passed", "evidence": "host:fixture-check"
    }]);
    input["host_result"]["governance"]["receipt_path"] = json!(".scafld/receipts/fixture.json");
    let run = || -> TestResult<Value> {
        let mut command = super::command(&checkout);
        command
            .env("RUNX_HOME", &home)
            .env("RUNX_PROJECT_DIR", root.join("state"))
            .env("RUNX_CWD", &checkout)
            .env("HOME", &home)
            .env("PATH", &path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .arg("skill")
            .arg(format!(
                "{}#resume",
                repo.join("skills/github-issue-to-pr").display()
            ))
            .arg("--inputs")
            .arg("-")
            .arg("--json");
        assert_json(&output_with_stdin(&mut command, &input.to_string())?, 0)
    };
    let complete = run()?;
    let result = &complete["result"]["issue_to_pr_result"];
    assert_eq!(result["status"], "completed", "{complete}");
    assert_eq!(result["commit"], head);
    assert_eq!(result["publication"]["status"], "not_requested");
    assert_eq!(result["governance"]["status"], "verified");
    assert_eq!(
        fs::read_to_string(checkout.join(".scafld/verifier-calls"))?,
        format!("{head}\n")
    );
    fs::write(checkout.join("README.md"), "An uncommitted change.\n")?;
    let dirty = run()?;
    assert_eq!(
        dirty["result"]["issue_to_pr_result"]["status"], "blocked",
        "{dirty}"
    );
    assert_eq!(
        fs::read_to_string(checkout.join(".scafld/verifier-calls"))?,
        format!("{head}\n")
    );
    fs::remove_dir_all(&root)?;
    Ok(())
}
