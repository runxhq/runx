//! Local cursor continuation; this method deliberately makes no GitHub call.

use std::fs;

use serde_json::{Value, json};

use super::{TestResult, assert_json, output_with_stdin};

#[test]
fn a_github_cursor_continues_from_its_real_local_version() -> TestResult {
    let root = crate::support::temp_root("runx-github-cursor-continuation");
    fs::create_dir_all(&root)?;
    let repo = crate::support::repo_root()?;
    let mut input: Value = serde_json::from_str(&fs::read_to_string(
        repo.join("skills/github/evals/files/triage-sync-cursor.json"),
    )?)?;
    let object = input.as_object_mut().ok_or("cursor input is an object")?;
    object.remove("about");
    object.remove("prior_cursor");
    object.remove("prior_provider_evidence");
    let run = |input: &Value| -> TestResult<Value> {
        let mut command = super::command(&root);
        command
            .arg("skill")
            .arg(format!(
                "{}#plan_and_append_cursor",
                repo.join("skills/github").display()
            ))
            .arg("--inputs")
            .arg("-")
            .arg("--json");
        assert_json(&output_with_stdin(&mut command, &input.to_string())?, 0)
    };
    let mut first = input.clone();
    first["expected_version"] = json!(0);
    first["idempotency_key"] = json!("runxhq/runx:triage-open:pull:v1");
    first["resources"]["filters"]["cursor"] = json!("1");
    let seeded = run(&first)?;
    assert_eq!(
        seeded["result"]["provider_operation"]["result"]["version"], 1,
        "{seeded}"
    );

    let continued = run(&input)?;
    let state = &continued["result"]["provider_operation"]["result"];
    assert_eq!(state["version"], 2, "{continued}");
    assert_eq!(state["key"], input["key"]);
    let plan = &state["last_event"]["data"];
    assert_eq!(plan["repo"], input["repo"]);
    assert_eq!(
        plan["resource_selector"]["filters"],
        input["resources"]["filters"]
    );
    assert_eq!(plan["provider_input"]["page"], 2);
    assert_eq!(plan["provider_status"], "not_called");
    assert_eq!(plan["mutations"], json!([]));

    let repeated = run(&input)?;
    assert_eq!(repeated["result"]["provider_operation"]["result"], *state);
    fs::remove_dir_all(root)?;
    Ok(())
}
