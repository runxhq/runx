#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;

use runx_contracts::{
    JsonNumber, JsonValue, SkillApplyVerdict, SkillArchitectureDecision,
    SkillArchitectureDisposition, SkillChangeBundle, SkillChangeDecision, SkillChangeDraft,
    SkillChangeDraftSchema, SkillFileWrite, SkillPackageVisibility, SkillProofKind,
};
use serde_json::json;

use super::{
    CandidateStage, apply_skill_change, assert_allowed_package_delete_path,
    assert_allowed_package_write_path, bind_skill_change, inspect_skill_workspace,
    package_snapshot, plan_skill_architecture,
};
use crate::RuntimeEffectRegistry;
use crate::filesystem::TextBundle;

const VALID_MANUAL: &str = "---\nname: demo\ndescription: Make one bounded, reviewable decision from supplied evidence.\n---\n\n# Demo\n\nUse this skill when an operator needs one bounded decision. Inspect the supplied evidence, stop when it is incomplete, and return the decision without mutating an external system.\n";
const VALID_MANIFEST: &str = r#"skill: demo
version: "0.1.0"

catalog:
  kind: skill
  audience: builder
  visibility: public
  role: canonical
  execution: plan
  completion: plan
  approval: none

harness:
  cases:
    - name: demo-operator-journeys
      runner: decide
      operator_journeys:
        - mode: standalone
          confusors: [work-plan, issue-intake]
          request: Make one bounded demo decision from supplied evidence.
          expected_outcome: Return the advertised reviewable decision.
        - mode: composed
          request: Continue a bounded decision from prior evidence.
          expected_outcome: Reuse the supplied evidence without rediscovery.
          prior_evidence:
            - The objective evidence supplied by the prior step.
          must_not_repeat:
            - Do not rediscover supplied evidence.
      caller:
        answers:
          decide:
            decision:
              status: ready
      expect:
        status: sealed

runners:
  decide:
    default: true
    steps:
      - id: decide
        agent: { role: builder, task: demo-decide }
        outputs:
          decision: object
"#;

fn architecture(
    disposition: SkillArchitectureDisposition,
    deletes: &[&str],
) -> SkillArchitectureDecision {
    let disposition_name = match disposition {
        SkillArchitectureDisposition::Build => "build",
        SkillArchitectureDisposition::ExtendExisting => "extend_existing",
        SkillArchitectureDisposition::NoSkill => "no_skill",
        SkillArchitectureDisposition::NeedsCore => "needs_core",
    };
    let implements_package = matches!(
        disposition,
        SkillArchitectureDisposition::Build | SkillArchitectureDisposition::ExtendExisting
    );
    serde_json::from_value(json!({
        "schema": "runx.skill.architecture_decision.v1",
        "disposition": disposition_name,
        "identity": if implements_package {
            json!({
                "current_name": if disposition == SkillArchitectureDisposition::ExtendExisting {
                    json!("demo")
                } else {
                    json!(null)
                },
                "proposed_name": "demo",
                "action": if disposition == SkillArchitectureDisposition::Build { "create" } else { "keep" },
                "visibility": "public",
                "rationale": "Demo is the exact operator-facing identity."
            })
        } else {
            json!(null)
        },
        "direct_use": if implements_package {
            json!({
                "trigger_requests": ["Make one bounded demo decision."],
                "non_trigger_requests": ["Publish this decision."],
                "default_outcome": "Return one bounded decision.",
                "routine_host_work": ["Inspect the supplied objective."],
                "runx_boundary": "Bind the result and evidence in a receipt.",
                "terminal_result": "A reviewable demo decision.",
                "blocker_behavior": "Block once and name missing evidence.",
                "native_escape": "Return gathered context for native continuation."
            })
        } else {
            json!(null)
        },
        "chain_use": if implements_package {
            json!({
                "accepted_inputs": ["A supplied objective or prior evidence packet."],
                "result": "A reusable demo decision.",
                "reused_evidence": ["Prior objective evidence."],
                "reused_effects": [],
                "must_not_repeat": ["Do not rediscover supplied evidence."]
            })
        } else {
            json!(null)
        },
        "objective": "Create or maintain the bounded demo skill.",
        "operator_value": "Give the operator one reviewable decision.",
        "knowledge_contract": {
            "purpose": "Explain and perform the bounded decision.",
            "evidence_required": ["A supplied objective."],
            "decision_logic": ["Preserve supplied evidence."],
            "stop_conditions": ["Stop when evidence is incomplete."],
            "recovery": ["Resume with the missing evidence."]
        },
        "required_behaviors": if implements_package {
            json!([{
                "id": "decide",
                "outcome": "Return one bounded decision.",
                "lane": "agent"
            }])
        } else {
            json!([])
        },
        "native_reuse": {
            "inspected_capabilities": ["runx.skill.inspect", "runx.skill.apply"],
            "selected_capabilities": [],
            "missing_capabilities": if disposition == SkillArchitectureDisposition::NeedsCore {
                json!(["runx.missing.capability"])
            } else {
                json!([])
            }
        },
        "effects": [{
            "effect": "none",
            "authority_scopes": [],
            "approval": "none",
            "provider_boundary": false
        }],
        "skill_chain": { "context_skills": [], "routes": [] },
        "resource_budget": {
            "max_files": 8,
            "max_executable_lines": 0,
            "max_process_spawns": 0,
            "network_allowed": false
        },
        "preservation_obligations": ["Keep the operating manual substantive."],
        "deletions": deletes,
        "proof_plan": if implements_package {
            json!([
                {
                    "name": "cold-selection",
                    "kind": "selection_trial",
                    "expected": "The natural request selects demo and a publish request does not."
                },
                {
                    "name": "standalone-result",
                    "kind": "standalone_operator_journey",
                    "expected": "The direct request returns a decision."
                },
                {
                    "name": "composed-reuse",
                    "kind": "composed_operator_journey",
                    "expected": "Prior evidence is reused without rediscovery."
                }
            ])
        } else {
            json!([])
        }
    }))
    .expect("architecture fixture must be valid")
}

fn change_bundle(
    base_digest: &str,
    decision: SkillChangeDecision,
    architecture: SkillArchitectureDecision,
    writes: &[(&str, &str)],
    deletes: &[&str],
) -> SkillChangeBundle {
    let plan = plan_skill_architecture(base_digest, architecture)
        .expect("architecture fixture must produce a plan");
    let draft = SkillChangeDraft {
        schema: SkillChangeDraftSchema::V1,
        decision,
        summary: "Apply the bounded demo package change.".into(),
        non_goals: vec!["Do not publish or call a provider.".into()],
        writes: writes
            .iter()
            .map(|(path, contents)| SkillFileWrite {
                path: (*path).into(),
                contents: (*contents).to_owned(),
            })
            .collect(),
        deletes: deletes.iter().map(|path| (*path).into()).collect(),
        expected_outputs: Vec::new(),
    };
    bind_skill_change(&plan, draft).expect("valid plan and draft must bind")
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

    assert_eq!(report["target_exists"], JsonValue::Bool(true));
    assert_eq!(report["base_digest"], JsonValue::String(expected_digest));
    let target_inspection = report
        .get("target_inspection")
        .and_then(JsonValue::as_object)
        .expect("existing skill inspection must include its resolved contract");
    assert!(
        target_inspection
            .get("runner_inspections")
            .and_then(JsonValue::as_array)
            .is_some_and(|runners| !runners.is_empty())
    );
    let target_metrics = report
        .get("target_metrics")
        .and_then(JsonValue::as_object)
        .expect("inspection must return typed package metrics");
    assert_eq!(
        target_metrics.get("files"),
        Some(&JsonValue::Number(JsonNumber::U64(2)))
    );
    assert_eq!(
        target_metrics
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "bytes",
            "executable_files",
            "executable_lines",
            "files",
            "generated_lines",
            "production_lines",
            "test_lines",
        ]
    );
    assert!(
        report["catalog_skills"]
            .as_array()
            .is_some_and(|skills| skills.iter().any(|skill| {
                skill
                    .as_object()
                    .and_then(|skill| skill.get("path"))
                    .and_then(JsonValue::as_str)
                    == Some("skills/team/demo/run.yaml")
            }))
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
    fs::write(skill.join("run.yaml"), "skill: demo\nrunners: []\n")?;

    let report = inspect_skill_workspace(
        temp.path(),
        Some("skills/team/demo"),
        &RuntimeEffectRegistry::default(),
    )?;

    assert_eq!(report["target_exists"], JsonValue::Bool(true));
    let target_inspection = report
        .get("target_inspection")
        .and_then(JsonValue::as_object)
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
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::NoChange,
        architecture(SkillArchitectureDisposition::ExtendExisting, &[]),
        &[],
        &[],
    );
    let report = apply_skill_change(
        temp.path(),
        "skills/demo",
        "improve",
        &bundle,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )?;

    assert_eq!(report.verdict, SkillApplyVerdict::Unchanged);
    assert!(!temp.path().join("skills/demo").exists());
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
        architecture(SkillArchitectureDisposition::Build, &[]),
        &[
            ("SKILL.md", "not valid skill frontmatter"),
            ("run.yaml", "schema: runx.runner.v1\n"),
        ],
        &[],
    );
    let result = apply_skill_change(
        temp.path(),
        "skills/demo",
        "build",
        &bundle,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    );

    assert!(result.is_err());
    assert!(!temp.path().join("skills/demo").exists());
    let staging = temp.path().join(".runx/staging");
    assert!(
        !staging.exists() || fs::read_dir(staging)?.next().is_none(),
        "failed validation must not leave a staged candidate"
    );
    Ok(())
}

#[test]
fn skill_authoring_needs_core_does_not_touch_workspace() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let base = package_snapshot(&temp.path().join("skills/demo"))?.digest;
    let bundle = change_bundle(
        &base,
        SkillChangeDecision::NeedsCore,
        architecture(SkillArchitectureDisposition::NeedsCore, &[]),
        &[],
        &[],
    );
    let report = apply_skill_change(
        temp.path(),
        "skills/demo",
        "build",
        &bundle,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )?;

    assert_eq!(report.verdict, SkillApplyVerdict::NeedsCore);
    assert!(!temp.path().join("skills/demo").exists());
    Ok(())
}

#[test]
fn skill_authoring_plan_digest_binds_the_base_and_architecture()
-> Result<(), Box<dyn std::error::Error>> {
    let architecture = architecture(SkillArchitectureDisposition::Build, &[]);
    let first =
        plan_skill_architecture(&format!("sha256:{}", "a".repeat(64)), architecture.clone())?;
    let second = plan_skill_architecture(&format!("sha256:{}", "b".repeat(64)), architecture)?;

    assert_ne!(first.plan_digest, second.plan_digest);
    Ok(())
}

#[test]
fn skill_authoring_rejects_public_architecture_without_operator_contract_and_each_journey_proof() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let mut missing_identity = architecture(SkillArchitectureDisposition::Build, &[]);
    missing_identity.identity = None;
    assert!(plan_skill_architecture(&digest, missing_identity).is_err());

    for missing in [
        SkillProofKind::SelectionTrial,
        SkillProofKind::StandaloneOperatorJourney,
        SkillProofKind::ComposedOperatorJourney,
    ] {
        let mut incomplete = architecture(SkillArchitectureDisposition::Build, &[]);
        incomplete.proof_plan.retain(|proof| proof.kind != missing);
        assert!(
            plan_skill_architecture(&digest, incomplete).is_err(),
            "missing {missing:?} must fail admission"
        );
    }

    let mut internal = architecture(SkillArchitectureDisposition::ExtendExisting, &[]);
    let identity = internal
        .identity
        .as_mut()
        .expect("extension fixture must have identity");
    identity.visibility = SkillPackageVisibility::Internal;
    internal.direct_use = None;
    assert!(plan_skill_architecture(&digest, internal).is_ok());

    let mut internal_with_public_triggers =
        architecture(SkillArchitectureDisposition::ExtendExisting, &[]);
    internal_with_public_triggers
        .identity
        .as_mut()
        .expect("extension fixture must have identity")
        .visibility = SkillPackageVisibility::Internal;
    assert!(plan_skill_architecture(&digest, internal_with_public_triggers).is_err());
}

#[test]
fn skill_authoring_bind_owns_integrity_fields_and_rejects_tampered_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let base_digest = format!("sha256:{}", "a".repeat(64));
    let plan = plan_skill_architecture(
        &base_digest,
        architecture(SkillArchitectureDisposition::Build, &[]),
    )?;
    let draft = SkillChangeDraft {
        schema: SkillChangeDraftSchema::V1,
        decision: SkillChangeDecision::Write,
        summary: "Create the bounded demo package.".into(),
        non_goals: vec!["Do not call a provider.".into()],
        writes: vec![SkillFileWrite {
            path: "SKILL.md".into(),
            contents: VALID_MANUAL.to_owned(),
        }],
        deletes: Vec::new(),
        expected_outputs: Vec::new(),
    };
    let serialized_draft = serde_json::to_value(&draft)?;
    assert!(serialized_draft.get("base_digest").is_none());
    assert!(serialized_draft.get("plan_digest").is_none());

    let bundle = bind_skill_change(&plan, draft.clone())?;
    assert_eq!(bundle.base_digest, plan.base_digest);
    assert_eq!(bundle.plan_digest, plan.plan_digest);
    assert_eq!(bundle.architecture, plan.architecture);

    let mut tampered = plan;
    tampered.plan_digest = format!("sha256:{}", "f".repeat(64)).into();
    assert!(bind_skill_change(&tampered, draft).is_err());
    Ok(())
}

#[test]
fn skill_authoring_rejects_stale_base_and_plan_drift() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("skills/demo");
    fs::create_dir_all(&target)?;
    fs::write(target.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(target.join("run.yaml"), VALID_MANIFEST)?;
    let base = package_snapshot(&target)?.digest;
    let mut bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        architecture(SkillArchitectureDisposition::ExtendExisting, &[]),
        &[(
            "SKILL.md",
            &VALID_MANUAL.replace("bounded decision", "bounded operator decision"),
        )],
        &[],
    );
    bundle.plan_digest = format!("sha256:{}", "f".repeat(64)).into();
    assert!(
        apply_skill_change(
            temp.path(),
            "skills/demo",
            "improve",
            &bundle,
            &BTreeMap::new(),
            &RuntimeEffectRegistry::default(),
        )
        .is_err()
    );

    let bundle = change_bundle(
        &base,
        SkillChangeDecision::Write,
        architecture(SkillArchitectureDisposition::ExtendExisting, &[]),
        &[("SKILL.md", VALID_MANUAL)],
        &[],
    );
    fs::write(target.join("operator-note.txt"), "concurrent change\n")?;
    assert!(
        apply_skill_change(
            temp.path(),
            "skills/demo",
            "improve",
            &bundle,
            &BTreeMap::new(),
            &RuntimeEffectRegistry::default(),
        )
        .is_err()
    );
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
        architecture(
            SkillArchitectureDisposition::ExtendExisting,
            &["obsolete.txt"],
        ),
        &[],
        &["obsolete.txt"],
    );

    let first = apply_skill_change(
        temp.path(),
        "skills/demo",
        "improve",
        &bundle,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )?;
    assert_eq!(first.verdict, SkillApplyVerdict::ValidatedAndApplied);
    assert_eq!(first.deleted_paths, vec!["obsolete.txt"]);
    assert_eq!(
        first.validation.as_ref().map(|value| value.delta.files),
        Some(-1)
    );

    let retry = apply_skill_change(
        temp.path(),
        "skills/demo",
        "improve",
        &bundle,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )?;
    assert_eq!(retry.verdict, SkillApplyVerdict::Unchanged);
    assert!(!target.join("obsolete.txt").exists());
    Ok(())
}

#[test]
fn skill_authoring_improve_may_maintain_but_not_add_or_delete_public_docs()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("skills/demo");
    fs::create_dir_all(&target)?;
    fs::write(target.join("README.md"), "existing public surface\n")?;

    assert_allowed_package_write_path("README.md", &target, "improve")?;
    assert!(assert_allowed_package_write_path("CHANGELOG.md", &target, "improve").is_err());
    assert!(assert_allowed_package_write_path("README.md", &target, "build").is_err());
    assert!(assert_allowed_package_delete_path("README.md", "improve").is_err());
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
    let dependency = skills.join("data-store");
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
        },
    )?;

    assert_eq!(stage.skill_dir.parent(), target.parent());
    assert!(stage.skill_dir.join("../data-store").is_dir());
    drop(stage);
    assert!(target.is_dir());
    assert!(dependency.is_dir());
    Ok(())
}
