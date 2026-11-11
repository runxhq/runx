//! Continue the original local release fixture, counting actual command calls.

use std::fs;

use serde_json::{Value, json};

use super::{TestResult, assert_json, json_string, output_with_stdin, run_read};

#[test]
fn a_release_resumes_its_prepared_run_without_repeating_commands() -> TestResult {
    let root = crate::support::temp_root("runx-release-continuation");
    fs::create_dir_all(&root)?;
    let repo = crate::support::repo_root()?;
    let started = assert_json(
        &super::command(&root)
            .arg("skill")
            .arg(repo.join("skills/release/fixtures/release-fixture"))
            .arg("--json")
            .output()?,
        2,
    )?;
    let run = json_string(&started, "run_id")?;
    let resume = |answer: &Value, exit| -> TestResult<Value> {
        let mut command = super::command(&root);
        command.arg("resume").arg(run).arg("-").arg("--json");
        assert_json(&output_with_stdin(&mut command, &answer.to_string())?, exit)
    };
    let read = run_read(&root, run)?;
    let notes = read["waiting"][0]["address"]
        .as_str()
        .ok_or("notes address")?;
    assert!(notes.ends_with("#draft-notes"), "{read}");
    let answer = crate::support::bind_resume(
        &read,
        json!({"answers": {notes: {
            "release_notes": {
                "headline": "Fixture 1.2.3", "summary": "Exact local fixture release.",
                "changelog": {"added": [], "fixed": ["Prove independent release readback."],
                    "changed": [], "removed": [], "breaking": []},
                "upgrade_guidance": "No migration is required.", "risks": []
            }
        }}}),
    )?;
    resume(&answer, 2)?;
    let read = run_read(&root, run)?;
    let approval = read["waiting"][0]["address"]
        .as_str()
        .ok_or("publish approval address")?;
    assert!(approval.ends_with("#release.publish.approval"), "{read}");
    let approval = crate::support::bind_resume(&read, json!({"approvals": {approval: true}}))?;
    let calls = root.join(".runx/cache/test/release-fixture-project/phase-calls");
    assert_eq!(fs::read_to_string(&calls)?, "prepare\n");
    resume(&json!({}), 2)?;
    assert_eq!(fs::read_to_string(&calls)?, "prepare\n");
    let completed = resume(&approval, 0)?;
    assert_eq!(completed["run_id"], run);
    assert_eq!(
        completed["result"]["verification"]["status"], "verified",
        "{completed}"
    );
    assert_eq!(completed["result"]["verification"]["version"], "1.2.3");
    assert_eq!(fs::read_to_string(&calls)?, "prepare\npublish\nverify\n");
    let sealed = resume(&json!({}), 0)?;
    assert_eq!(sealed["result"], completed["result"]);
    assert_eq!(sealed["receipt_id"], completed["receipt_id"]);
    assert_eq!(fs::read_to_string(&calls)?, "prepare\npublish\nverify\n");
    fs::remove_dir_all(root)?;
    Ok(())
}
