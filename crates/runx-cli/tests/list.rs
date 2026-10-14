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
    assert_eq!(items[0]["visibility"], "public");
    assert_eq!(items[0]["description"], "Triage one bounded request.");

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn list_marks_a_graph_stage_internal_with_its_own_description()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("runx-list-graph-stage");
    let skill = root.join("skills/reply");
    let stage = skill.join("graph/draft");
    fs::create_dir_all(&stage)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: reply\ndescription: Reply to one request.\n---\n\n# Reply\n\nDraft a reply through the draft stage.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "skill: reply\nversion: 1.0.0\nrunners:\n  reply:\n    default: true\n    steps:\n      - id: draft\n        skill: ./graph/draft\n",
    )?;
    fs::write(
        stage.join("SKILL.md"),
        "---\nname: reply-draft\ndescription: Draft one reply.\n---\n\n# Draft\n\nDraft one reply to the request.\n",
    )?;
    fs::write(
        stage.join("run.yaml"),
        "skill: reply-draft\nversion: 1.0.0\ncatalog:\n  kind: skill\n  audience: public\n  visibility: internal\n  role: graph-stage\n  part_of:\n    - reply\nrunners:\n  draft:\n    default: true\n    steps:\n      - id: draft\n        agent: { role: writer, task: draft }\n",
    )?;

    let output = isolated_runx_command_with_inherited_cwd("list-graph-stage")
        .current_dir(&root)
        .args(["list", "skills", "--ok-only", "--json"])
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
    let owner = local("reply").ok_or(format!("reply is not listed: {report}"))?;
    let draft = local("reply-draft").ok_or(format!("reply-draft is not listed: {report}"))?;
    assert_eq!(owner["visibility"], "public");
    assert_eq!(owner["description"], "Reply to one request.");
    assert_eq!(draft["visibility"], "internal");
    assert_eq!(draft["description"], "Draft one reply.");

    fs::remove_dir_all(root)?;
    Ok(())
}
