use std::collections::BTreeMap;
use std::fs;

use runx_contracts::{SkillChangeDecision, SkillFileWrite};

use super::*;

const VALID_MANUAL: &str = "---\nname: demo\ndescription: Make one bounded, reviewable decision from supplied evidence.\n---\n\n# Demo\n\n## Agent task: demo-decide\n\nUse this skill when an operator needs one bounded decision. Inspect the supplied evidence, stop when it is incomplete, and return the decision without mutating an external system.\n";
const VALID_MANIFEST: &str = r#"version: "0.1.0"
minimum_runx: 0.12.0

catalog:
  visibility: public

methods:
  decide:
    steps:
      - id: recall
        skill: runx/memory#index
        inputs:
          levels: [{level: personal}, {level: workspace}]
      - id: decide
        agent: demo-decide
        context:
          entries: [$steps.recall.entries]
        outputs:
          decision: {type: object}
"#;

#[test]
fn authoring_rejects_a_change_between_validation_and_commit_without_partial_apply()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target_path = temp.path().join("skills/demo");
    fs::create_dir_all(&target_path)?;
    fs::write(target_path.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(target_path.join("run.yaml"), VALID_MANIFEST)?;

    let target = ApplyTarget::resolve(temp.path(), "skills/demo", &BTreeMap::new())?;
    let change = change_bundle(
        target.before.digest.as_str(),
        VALID_MANUAL.replace("bounded decision", "bounded operator decision"),
    )?;
    validate_change_contract(
        &change,
        runx_contracts::SkillAuthoringMode::Improve,
        target.before.digest.as_str(),
    )?;
    let admitted = admit_skill_change(
        &target.relative,
        runx_contracts::SkillAuthoringMode::Improve,
        &change,
        &std::collections::BTreeSet::new(),
    )?;
    let candidate = validate_candidate_stage(
        &target,
        &admitted,
        &target.authored(),
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )?;

    fs::write(target.path.join("operator-note.txt"), "concurrent edit\n")?;
    let result = commit_candidate(
        &target,
        &change,
        admitted,
        &candidate,
        None,
        &BTreeMap::new(),
    );

    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(target.path.join("SKILL.md"))?,
        VALID_MANUAL
    );
    assert_eq!(
        fs::read_to_string(target.path.join("operator-note.txt"))?,
        "concurrent edit\n"
    );
    assert!(!target.paths.authoring_applications().exists());
    Ok(())
}

fn change_bundle(base_digest: &str, manual: String) -> Result<SkillChangeBundle, RuntimeError> {
    Ok(SkillChangeBundle {
        schema: SkillChangeBundleSchema::V1,
        decision: SkillChangeDecision::Write,
        base_digest: base_digest.into(),
        summary: "Apply one bounded manual correction.".into(),
        writes: vec![SkillFileWrite {
            path: "SKILL.md".into(),
            contents: manual,
        }],
        deletes: Vec::new(),
    })
}
