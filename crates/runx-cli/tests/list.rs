use std::fs;

use crate::support::{isolated_runx_command_with_inherited_cwd, temp_root};

#[test]
fn list_discovers_nested_repo_skills() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("runx-list-nested-skills");
    let skill = root.join("skills/acme/triage");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: triage\ndescription: Triage one bounded request.\n---\n\n# Triage\n\n## Agent task: triage\n\nReview the supplied request and return a bounded triage decision.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "version: 1.0.0\nminimum_runx: 0.12.0\nmethods:\n  triage:\n    steps:\n      - id: triage\n        agent: triage\n",
    )?;

    let output = isolated_runx_command_with_inherited_cwd("list-nested")
        .current_dir(&root)
        .args(["list", "skills", "--only", "valid", "--json"])
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
    assert_eq!(items[0]["visibility"], "public");
    assert_eq!(items[0]["description"], "Triage one bounded request.");

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn list_marks_a_workflow_stage_internal_with_its_own_description()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("runx-list-workflow-stage");
    let skill = root.join("skills/reply");
    let stage = skill.join("graph/draft");
    fs::create_dir_all(&stage)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: reply\ndescription: Reply to one request.\n---\n\n# Reply\n\nDraft a reply through the draft stage.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "version: 1.0.0\nminimum_runx: 0.12.0\nmethods:\n  reply:\n    steps:\n      - id: draft\n        skill: ./graph/draft\n",
    )?;
    fs::write(
        stage.join("SKILL.md"),
        "---\nname: reply-draft\ndescription: Draft one reply.\n---\n\n# Draft\n\n## Agent task: draft\n\nDraft one reply to the request.\n",
    )?;
    fs::write(
        stage.join("run.yaml"),
        "version: 1.0.0\nminimum_runx: 0.12.0\ncatalog:\n  visibility: internal\nmethods:\n  draft:\n    steps:\n      - id: draft\n        agent: draft\n",
    )?;

    let output = isolated_runx_command_with_inherited_cwd("list-workflow-stage")
        .current_dir(&root)
        .args(["list", "skills", "--only", "valid", "--json"])
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    let local = |name: &str| {
        report["items"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["source"] == "local" && item["name"] == name)
            })
            .cloned()
    };
    let reply = local("reply").ok_or(format!("reply is not listed: {report}"))?;
    let draft = local("reply-draft").ok_or(format!("reply-draft is not listed: {report}"))?;
    assert_eq!(reply["visibility"], "public");
    assert_eq!(reply["description"], "Reply to one request.");
    assert_eq!(draft["visibility"], "internal");
    assert_eq!(draft["description"], "Draft one reply.");

    fs::remove_dir_all(root)?;
    Ok(())
}
