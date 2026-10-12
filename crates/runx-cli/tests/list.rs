use std::fs;

use crate::support::{isolated_runx_command_with_inherited_cwd, temp_root};

#[test]
fn list_discovers_nested_repo_skills() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("runx-list-nested-skills");
    let skill = root.join("skills/acme/triage");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: triage\ndescription: Triage one bounded request.\n---\n\n# Triage\n\nReview the supplied request and return a bounded triage decision.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "skill: triage\nversion: 1.0.0\nrunners:\n  triage:\n    default: true\n    steps:\n      - id: triage\n        agent: { role: reviewer, task: triage }\n",
    )?;

    let output = isolated_runx_command_with_inherited_cwd("list-nested")
        .current_dir(&root)
        .args(["list", "skills", "--ok-only", "--json"])
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    // The bundled core skills are listed beside the repository's own.
    let items = report["items"]
        .as_array()
        .ok_or("items must be an array")?
        .iter()
        .filter(|item| item["source"] == "local")
        .collect::<Vec<_>>();
    assert_eq!(items.len(), 1, "{report}");
    assert_eq!(items[0]["name"], "triage");
    assert_eq!(items[0]["path"], "skills/acme/triage/run.yaml");

    fs::remove_dir_all(root)?;
    Ok(())
}
