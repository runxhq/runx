#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{
    Digest, JsonValue, SkillApplyResult, SkillApplyVerdict, SkillAuthoringMode, SkillChangeBundle,
    SkillChangeBundleSchema, SkillChangeDecision, SkillChangeDraft, SkillChangeDraftSchema,
    SkillFileWrite,
};

use super::{
    CandidateStage, apply_skill_change, assert_mode_path, bind_skill_change,
    inspect_skill_workspace, package_snapshot,
};
use crate::filesystem::TextBundle;
use crate::{RuntimeEffectRegistry, RuntimeError};

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
/// The demo package as it was written before every turn read memory.
const UNRECALLED_MANIFEST: &str = r#"version: "0.1.0"
minimum_runx: 0.12.0

catalog:
  visibility: public

methods:
  decide:
    steps:
      - id: decide
        agent: demo-decide
        outputs:
          decision: {type: object}
"#;

fn files(writes: &[(&str, &str)]) -> Vec<SkillFileWrite> {
    writes
        .iter()
        .map(|(path, contents)| SkillFileWrite {
            path: (*path).into(),
            contents: (*contents).to_owned(),
        })
        .collect()
}

/// What an author returns.
fn draft(decision: SkillChangeDecision, writes: &[(&str, &str)]) -> SkillChangeDraft {
    SkillChangeDraft {
        schema: SkillChangeDraftSchema::V1,
        decision,
        summary: "Apply the bounded demo package change.".into(),
        writes: files(writes),
        deletes: Vec::new(),
    }
}

/// A change bound to `base_digest`, as runx.skill.bind binds one.
fn change_bundle(
    base_digest: &Digest,
    decision: SkillChangeDecision,
    writes: &[(&str, &str)],
    deletes: &[&str],
) -> SkillChangeBundle {
    SkillChangeBundle {
        schema: SkillChangeBundleSchema::V1,
        decision,
        base_digest: base_digest.as_str().into(),
        summary: "Apply the bounded demo package change.".into(),
        writes: files(writes),
        deletes: deletes.iter().map(|path| (*path).into()).collect(),
    }
}

fn apply(
    root: &Path,
    mode: SkillAuthoringMode,
    bundle: &SkillChangeBundle,
) -> Result<SkillApplyResult, RuntimeError> {
    apply_skill_change(
        root,
        "skills/demo",
        mode,
        bundle,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )
}

/// The demo package, written at `skills/demo` under `root`.
fn demo(root: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let target = root.join("skills/demo");
    fs::create_dir_all(&target)?;
    fs::write(target.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(target.join("run.yaml"), VALID_MANIFEST)?;
    Ok(target)
}

#[test]
fn skill_authoring_workspace_inspection_includes_base_digest()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let skill = temp.path().join("skills/team/demo");
    fs::create_dir_all(&skill)?;
    fs::write(skill.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(skill.join("run.yaml"), VALID_MANIFEST)?;
    let expected_digest = package_snapshot(&skill)?.digest;

    let report = inspect_skill_workspace(
        temp.path(),
        Some("skills/team/demo"),
        &RuntimeEffectRegistry::default(),
    )?;

    assert!(report.target_exists);
    assert_eq!(report.base_digest, expected_digest);
    let target_inspection = report
        .target_inspection
        .as_ref()
        .expect("existing skill inspection must include its resolved contract");
    assert!(
        target_inspection
            .get("method_inspections")
            .and_then(JsonValue::as_array)
            .is_some_and(|methods| !methods.is_empty())
    );
    assert_eq!(report.target_metrics.files, 2);
    assert!(
        report
            .catalog_skills
            .iter()
            .any(|skill| skill.path == "skills/team/demo/run.yaml")
    );
    Ok(())
}

#[test]
fn skill_authoring_workspace_keeps_invalid_target_repairable()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let skill = temp.path().join("skills/team/demo");
    fs::create_dir_all(&skill)?;
    fs::write(skill.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(
        skill.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods: []\n",
    )?;

    let report = inspect_skill_workspace(
        temp.path(),
        Some("skills/team/demo"),
        &RuntimeEffectRegistry::default(),
    )?;

    assert!(report.target_exists);
    let target_inspection = report
        .target_inspection
        .as_ref()
        .expect("invalid skill inspection must remain available as repair context");
    assert_eq!(
        target_inspection.get("status").and_then(JsonValue::as_str),
        Some("invalid")
    );
    assert!(
        target_inspection
            .get("error")
            .and_then(JsonValue::as_str)
            .is_some_and(|error| !error.is_empty())
    );
    Ok(())
}

#[test]
fn skill_authoring_no_change_does_not_touch_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let base = package_snapshot(&temp.path().join("skills/demo"))?.digest;
    let bundle = change_bundle(&base, SkillChangeDecision::NoChange, &[], &[]);
    let report = apply(temp.path(), SkillAuthoringMode::Improve, &bundle)?;

    assert_eq!(report.verdict, SkillApplyVerdict::Unchanged);
    assert!(!temp.path().join("skills/demo").exists());
    // No package stands at the target, so the result states no package's
    // digests.
    assert_eq!(report.package_digest, None);
    assert_eq!(report.distribution_digest, None);
    Ok(())
}

/// Bind checks an author's decision against the mode and the target it
/// inspected, and binds the base it was authored against, which the author
/// never writes: a build creates where nothing stands, an improve changes what
/// does, a decision that writes nothing carries no files, a design takes any
/// decision, and every file is named from the workspace root, inside the
/// target, as the inspection lists it.
#[test]
fn bind_checks_the_decision_against_the_mode_and_the_target()
-> Result<(), Box<dyn std::error::Error>> {
    use SkillAuthoringMode::{Build, Design, Improve, Test};
    use SkillChangeDecision::{NeedsCore, NoChange, NoSkill, Write};
    let effects = RuntimeEffectRegistry::default();
    let empty = tempfile::tempdir()?;
    let missing = inspect_skill_workspace(empty.path(), Some("skills/demo"), &effects)?;
    let standing = tempfile::tempdir()?;
    demo(standing.path())?;
    let existing = inspect_skill_workspace(standing.path(), Some("skills/demo"), &effects)?;
    let write = draft(Write, &[("skills/demo/SKILL.md", VALID_MANUAL)]);
    assert!(serde_json::to_value(&write)?.get("base_digest").is_none());
    assert!(
        existing
            .target_files
            .iter()
            .any(|file| file.path == "skills/demo/SKILL.md")
    );
    for outside in ["SKILL.md", "skills/demo2/SKILL.md", "skills/demo"] {
        let error = bind_skill_change(&existing, Improve, draft(Write, &[(outside, "x")]))
            .expect_err("a file outside the target");
        assert!(
            error.to_string().contains(&format!(
                "{outside} isn't in the package at skills/demo: a change names the package's files from the workspace root, as skills/demo/SKILL.md"
            )),
            "{error}"
        );
    }

    let built = bind_skill_change(&missing, Build, write.clone())?;
    assert_eq!(built.base_digest.as_str(), missing.base_digest.as_str());
    for (context, mode, expected) in [
        (&existing, Build, "creates a package where none stands"),
        (&missing, Improve, "changes a package that stands"),
        (&missing, Test, "changes a package that stands"),
    ] {
        let error = bind_skill_change(context, mode, write.clone())
            .expect_err("the target doesn't fit the mode");
        assert!(error.to_string().contains(expected), "{error}");
    }
    bind_skill_change(&existing, Improve, write.clone())?;
    for (mode, decision) in [(Improve, NoSkill), (Test, NeedsCore), (Build, NoChange)] {
        assert!(bind_skill_change(&existing, mode, draft(decision, &[])).is_err());
    }
    assert!(
        bind_skill_change(
            &existing,
            Improve,
            draft(NoChange, &[("skills/demo/SKILL.md", "x")])
        )
        .is_err()
    );
    assert!(bind_skill_change(&existing, Improve, draft(Write, &[])).is_err());
    for decision in [Write, NoChange, NoSkill, NeedsCore] {
        let writes: &[(&str, &str)] = if decision == Write {
            &[("skills/demo/SKILL.md", VALID_MANUAL)]
        } else {
            &[]
        };
        bind_skill_change(&existing, Design, draft(decision, writes))?;
    }
    Ok(())
}

/// A decision to write nothing is about the package as it stands: applied,
/// it leaves the package as it was, and once the package changes it refuses.
#[test]
fn a_decision_that_writes_nothing_is_about_the_package_as_it_stands()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target = demo(temp.path())?;
    let base = package_snapshot(&target)?.digest;
    let context = inspect_skill_workspace(
        temp.path(),
        Some("skills/demo"),
        &RuntimeEffectRegistry::default(),
    )?;
    let bundle = bind_skill_change(
        &context,
        SkillAuthoringMode::Improve,
        draft(SkillChangeDecision::NoChange, &[]),
    )?;
    let result = apply(temp.path(), SkillAuthoringMode::Improve, &bundle)?;
    assert_eq!(result.verdict, SkillApplyVerdict::Unchanged);
    assert_eq!(fs::read_to_string(target.join("SKILL.md"))?, VALID_MANUAL);
    assert_eq!(package_snapshot(&target)?.digest, base);

    fs::write(target.join("operator-note.txt"), "concurrent edit")?;
    assert!(apply(temp.path(), SkillAuthoringMode::Improve, &bundle).is_err());
    assert_eq!(
        fs::read_to_string(target.join("operator-note.txt"))?,
        "concurrent edit"
    );
    Ok(())
}

#[test]
fn skill_authoring_invalid_candidate_does_not_touch_target()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let base = package_snapshot(&temp.path().join("skills/demo"))?.digest;
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        &[
            ("skills/demo/SKILL.md", "not valid skill frontmatter"),
            ("skills/demo/run.yaml", "schema: runx.method.v1\n"),
        ],
        &[],
    );
    let result = apply(temp.path(), SkillAuthoringMode::Build, &bundle);

    assert!(result.is_err());
    assert!(!temp.path().join("skills/demo").exists());
    let staging = temp.path().join(".runx/staging");
    assert!(
        !staging.exists() || fs::read_dir(staging)?.next().is_none(),
        "failed validation must not leave a staged candidate"
    );
    Ok(())
}

/// A written package states its catalog, so its visibility is never a
/// default; a change to a package keeps its name, since a rename is the
/// person's own act; and every turn a change writes reads the person's
/// memory, while a turn that stood already is the person's to change. Each
/// refuses before anything is written.
#[test]
fn an_authored_package_states_its_catalog_keeps_its_name_and_its_new_turns_read_memory()
-> Result<(), Box<dyn std::error::Error>> {
    let no_catalog = VALID_MANIFEST.replacen("catalog:\n  visibility: public\n", "", 1);
    let empty = tempfile::tempdir()?;
    let base = package_snapshot(&empty.path().join("skills/demo"))?.digest;
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        &[
            ("skills/demo/SKILL.md", VALID_MANUAL),
            ("skills/demo/run.yaml", &no_catalog),
        ],
        &[],
    );
    let error = apply(empty.path(), SkillAuthoringMode::Build, &bundle)
        .expect_err("a package with no catalog is refused");
    assert!(error.to_string().contains("states its catalog"), "{error}");
    assert!(!empty.path().join("skills/demo").exists());

    let internal = VALID_MANIFEST.replace("visibility: public", "visibility: internal");
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        &[
            ("skills/demo/SKILL.md", VALID_MANUAL),
            ("skills/demo/run.yaml", &internal),
        ],
        &[],
    );
    assert_eq!(
        apply(empty.path(), SkillAuthoringMode::Build, &bundle)?.verdict,
        SkillApplyVerdict::ValidatedAndApplied
    );

    let standing = tempfile::tempdir()?;
    let target = demo(standing.path())?;
    let renamed = VALID_MANUAL.replacen("name: demo", "name: other", 1);
    let bundle = change_bundle(
        &package_snapshot(&target)?.digest,
        SkillChangeDecision::Write,
        &[("skills/demo/SKILL.md", &renamed)],
        &[],
    );
    let error = apply(standing.path(), SkillAuthoringMode::Improve, &bundle)
        .expect_err("a renamed package is refused");
    assert!(error.to_string().contains("keeps its name"), "{error}");
    assert_eq!(fs::read_to_string(target.join("SKILL.md"))?, VALID_MANUAL);

    // A new turn that reads no memory is refused.
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        &[
            ("skills/demo/SKILL.md", VALID_MANUAL),
            ("skills/demo/run.yaml", UNRECALLED_MANIFEST),
        ],
        &[],
    );
    let fresh = tempfile::tempdir()?;
    let error = apply(fresh.path(), SkillAuthoringMode::Build, &bundle)
        .expect_err("a written turn that reads no memory is refused");
    assert!(
        error.to_string().contains("reads the person's memory"),
        "{error}"
    );
    assert!(!fresh.path().join("skills/demo").exists());
    // A turn that stood already isn't the change's to rewrite.
    let older = tempfile::tempdir()?;
    let target = older.path().join("skills/demo");
    fs::create_dir_all(&target)?;
    fs::write(target.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(target.join("run.yaml"), UNRECALLED_MANIFEST)?;
    let corrected = VALID_MANUAL.replace("bounded decision", "bounded operator decision");
    let bundle = change_bundle(
        &package_snapshot(&target)?.digest,
        SkillChangeDecision::Write,
        &[("skills/demo/SKILL.md", &corrected)],
        &[],
    );
    assert_eq!(
        apply(older.path(), SkillAuthoringMode::Improve, &bundle)?.verdict,
        SkillApplyVerdict::ValidatedAndApplied
    );
    Ok(())
}

/// A design validates the package its change makes, staged, and writes
/// nothing.
#[test]
fn a_design_validates_its_candidate_and_writes_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let base = package_snapshot(&temp.path().join("skills/demo"))?.digest;
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        &[
            ("skills/demo/SKILL.md", VALID_MANUAL),
            ("skills/demo/run.yaml", VALID_MANIFEST),
        ],
        &[],
    );
    let result = apply(temp.path(), SkillAuthoringMode::Design, &bundle)?;
    assert_eq!(result.verdict, SkillApplyVerdict::Validated);
    assert!(result.validation.is_some());
    assert!(result.changed_paths.is_empty());
    assert!(!temp.path().join("skills/demo").exists());
    Ok(())
}

#[test]
fn skill_authoring_needs_core_does_not_touch_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let base = package_snapshot(&temp.path().join("skills/demo"))?.digest;
    let bundle = change_bundle(&base, SkillChangeDecision::NeedsCore, &[], &[]);
    let report = apply(temp.path(), SkillAuthoringMode::Build, &bundle)?;

    assert_eq!(report.verdict, SkillApplyVerdict::NeedsCore);
    assert!(!temp.path().join("skills/demo").exists());
    Ok(())
}

#[test]
fn skill_authoring_rejects_a_stale_base() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target = demo(temp.path())?;
    let bundle = change_bundle(
        &package_snapshot(&target)?.digest,
        SkillChangeDecision::Write,
        &[("skills/demo/SKILL.md", VALID_MANUAL)],
        &[],
    );
    fs::write(target.join("operator-note.txt"), "concurrent change\n")?;
    assert!(apply(temp.path(), SkillAuthoringMode::Improve, &bundle).is_err());
    Ok(())
}

#[test]
fn skill_authoring_apply_is_idempotent_and_reports_deletion_delta()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("skills/demo");
    fs::create_dir_all(&target)?;
    fs::write(target.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(target.join("run.yaml"), VALID_MANIFEST)?;
    fs::write(target.join("obsolete.txt"), "old path\n")?;
    let base = package_snapshot(&target)?.digest;
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        &[],
        &["skills/demo/obsolete.txt"],
    );

    let first = apply_skill_change(
        temp.path(),
        "skills/demo",
        SkillAuthoringMode::Improve,
        &bundle,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )?;
    assert_eq!(first.verdict, SkillApplyVerdict::ValidatedAndApplied);
    assert_eq!(first.deleted_paths, vec!["skills/demo/obsolete.txt"]);
    assert_eq!(
        first.validation.as_ref().map(|value| value.delta.files),
        Some(-1)
    );

    let retry = apply_skill_change(
        temp.path(),
        "skills/demo",
        SkillAuthoringMode::Improve,
        &bundle,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )?;
    assert_eq!(retry.verdict, SkillApplyVerdict::Unchanged);
    assert!(!target.join("obsolete.txt").exists());
    Ok(())
}

/// An applied package's result states the package digest `runx skill
/// inspect` reports for what it wrote and the distribution digest of the
/// package `runx registry package` makes of it, and applying it again states
/// the same.
#[test]
fn skill_authoring_apply_states_the_package_and_distribution_digests_of_what_it_wrote()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("skills/demo");
    fs::create_dir_all(&target)?;
    fs::write(target.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(target.join("run.yaml"), VALID_MANIFEST)?;
    fs::write(target.join("obsolete.txt"), "old path\n")?;
    let base = package_snapshot(&target)?.digest;
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        &[],
        &["skills/demo/obsolete.txt"],
    );
    let env = BTreeMap::new();
    let apply = || {
        apply_skill_change(
            temp.path(),
            "skills/demo",
            SkillAuthoringMode::Improve,
            &bundle,
            &env,
            &RuntimeEffectRegistry::default(),
        )
    };

    let applied = apply()?;
    assert_eq!(applied.verdict, SkillApplyVerdict::ValidatedAndApplied);
    let inspected = crate::inspect_skill_package(&target, None, None)?;
    let subject = target.to_string_lossy();
    let packaged = crate::registry::prepare_registry_publish_package(
        crate::registry::RegistryPublishPackageRequest {
            subject: &subject,
            env: &env,
            cwd: temp.path(),
        },
    )?
    .into_parts()
    .package_digest()?;
    assert_eq!(
        applied
            .package_digest
            .as_ref()
            .map(|digest| digest.as_str()),
        Some(inspected.package_digest.as_str())
    );
    assert_eq!(
        applied
            .distribution_digest
            .as_ref()
            .map(|digest| digest.as_str()),
        Some(packaged.as_str())
    );

    let again = apply()?;
    assert_eq!(again.verdict, SkillApplyVerdict::Unchanged);
    assert_eq!(again.package_digest, applied.package_digest);
    assert_eq!(again.distribution_digest, applied.distribution_digest);
    Ok(())
}

/// A test changes only the package's tests and what they read, and every
/// other mode writes any path in the package, a README among them.
#[test]
fn a_test_changes_only_tests_and_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    assert_mode_path("tests/reads-the-catalog.yaml", SkillAuthoringMode::Test)?;
    assert_mode_path("fixtures/catalog.json", SkillAuthoringMode::Test)?;
    assert!(assert_mode_path("SKILL.md", SkillAuthoringMode::Test).is_err());
    assert!(assert_mode_path("tests/nested/case.yaml", SkillAuthoringMode::Test).is_err());
    assert_mode_path("README.md", SkillAuthoringMode::Improve)?;
    assert_mode_path("README.md", SkillAuthoringMode::Build)?;
    Ok(())
}

#[test]
fn skill_authoring_package_snapshot_detects_creation_and_content_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("skills/demo");
    let absent = package_snapshot(&target)?.digest;
    fs::create_dir_all(&target)?;
    let empty = package_snapshot(&target)?.digest;
    fs::write(target.join("SKILL.md"), "first")?;
    let first = package_snapshot(&target)?.digest;
    fs::write(target.join("SKILL.md"), "second")?;
    let second = package_snapshot(&target)?.digest;

    assert_ne!(absent, empty);
    assert_ne!(empty, first);
    assert_ne!(first, second);
    Ok(())
}

#[test]
fn skill_authoring_candidate_stage_preserves_relative_sibling_skill_context()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let skills = temp.path().join("skills");
    let target = skills.join("twitter");
    let dependency = skills.join("ledger");
    fs::create_dir_all(&target)?;
    fs::create_dir_all(&dependency)?;
    fs::write(
        target.join("SKILL.md"),
        "---\nname: twitter\ndescription: twitter fixture.\n---\n",
    )?;

    let stage = CandidateStage::prepare(
        temp.path(),
        &target,
        &TextBundle {
            writes: Vec::new(),
            deletes: Vec::new(),
            expected: Default::default(),
        },
    )?;

    assert_eq!(stage.skill_dir.parent(), target.parent());
    assert!(stage.skill_dir.join("../ledger").is_dir());
    drop(stage);
    assert!(target.is_dir());
    assert!(dependency.is_dir());
    Ok(())
}
