use std::collections::BTreeMap;
use std::fs;

use runx_contracts::{
    SkillArchitectureDecision, SkillArchitectureDisposition, SkillChangeDecision, SkillChangeDraft,
    SkillChangeDraftSchema, SkillFileWrite,
};
use serde_json::json;

use super::*;

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
  requires_adapter: false
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
          agent.demo-decide.output:
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

#[test]
fn authoring_rejects_a_change_between_validation_and_commit_without_partial_apply()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let target_path = temp.path().join("skills/demo");
    fs::create_dir_all(&target_path)?;
    fs::write(target_path.join("SKILL.md"), VALID_MANUAL)?;
    fs::write(target_path.join("run.yaml"), VALID_MANIFEST)?;

    let target = ApplyTarget::resolve(temp.path(), "skills/demo")?;
    let change = change_bundle(
        &target.before.digest,
        VALID_MANUAL.replace("bounded decision", "bounded operator decision"),
    )?;
    validate_change_contract(&change, "improve", &target.before.digest)?;
    let admitted = admit_skill_change(&target.relative, &target.path, "improve", &change)?;
    let candidate = validate_candidate_stage(
        &target,
        &change,
        &admitted,
        &BTreeMap::new(),
        &RuntimeEffectRegistry::default(),
    )?;

    fs::write(target.path.join("operator-note.txt"), "concurrent edit\n")?;
    let result = commit_candidate(&target, &change, admitted, &candidate);

    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(target.path.join("SKILL.md"))?,
        VALID_MANUAL
    );
    assert_eq!(
        fs::read_to_string(target.path.join("operator-note.txt"))?,
        "concurrent edit\n"
    );
    assert!(!temp.path().join(".runx/authoring/applications").exists());
    Ok(())
}

fn change_bundle(base_digest: &str, manual: String) -> Result<SkillChangeBundle, RuntimeError> {
    let plan = plan_skill_architecture(base_digest, architecture()?)?;
    bind_skill_change(
        &plan,
        SkillChangeDraft {
            schema: SkillChangeDraftSchema::V1,
            decision: SkillChangeDecision::Write,
            summary: "Apply one bounded manual correction.".into(),
            non_goals: vec!["Do not publish or call a provider.".into()],
            writes: vec![SkillFileWrite {
                path: "SKILL.md".into(),
                contents: manual,
            }],
            deletes: Vec::new(),
            expected_outputs: Vec::new(),
        },
    )
}

fn architecture() -> Result<SkillArchitectureDecision, RuntimeError> {
    serde_json::from_value(json!({
        "schema": "runx.skill.architecture_decision.v1",
        "disposition": disposition_name(SkillArchitectureDisposition::ExtendExisting),
        "identity": {
            "current_name": "demo",
            "proposed_name": "demo",
            "action": "keep",
            "visibility": "public",
            "rationale": "Demo remains the exact operator-facing identity."
        },
        "direct_use": {
            "trigger_requests": ["Make one bounded demo decision."],
            "non_trigger_requests": ["Publish this decision."],
            "default_outcome": "Return one bounded decision.",
            "routine_host_work": ["Inspect the supplied objective."],
            "runx_boundary": "Bind result evidence in a receipt.",
            "terminal_result": "A reviewable demo decision.",
            "blocker_behavior": "Block once and name missing evidence.",
            "native_escape": "Return gathered evidence for native continuation."
        },
        "chain_use": {
            "accepted_inputs": ["A supplied objective or prior evidence packet."],
            "result": "A reusable demo decision.",
            "reused_evidence": ["Prior objective evidence."],
            "reused_effects": [],
            "must_not_repeat": ["Do not rediscover supplied evidence."]
        },
        "objective": "Maintain the bounded demo skill.",
        "operator_value": "Give the operator one reviewable decision.",
        "knowledge_contract": {
            "purpose": "Explain and perform the bounded decision.",
            "evidence_required": ["A supplied objective."],
            "decision_logic": ["Preserve supplied evidence."],
            "stop_conditions": ["Stop when evidence is incomplete."],
            "recovery": ["Resume with the missing evidence."]
        },
        "required_behaviors": [{
            "id": "decide",
            "outcome": "Return one bounded decision.",
            "lane": "agent"
        }],
        "native_reuse": {
            "inspected_capabilities": ["runx.skill.inspect", "runx.skill.apply"],
            "selected_capabilities": [],
            "missing_capabilities": []
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
        "deletions": [],
        "proof_plan": [
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
        ]
    }))
    .map_err(|source| RuntimeError::Json {
        context: "parsing the skill architecture test fixture".to_owned(),
        source,
    })
}

fn disposition_name(disposition: SkillArchitectureDisposition) -> &'static str {
    match disposition {
        SkillArchitectureDisposition::Build => "build",
        SkillArchitectureDisposition::ExtendExisting => "extend_existing",
        SkillArchitectureDisposition::NoSkill => "no_skill",
        SkillArchitectureDisposition::NeedsCore => "needs_core",
    }
}
