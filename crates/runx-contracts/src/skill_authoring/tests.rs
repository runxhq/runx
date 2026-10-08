use serde_json::{Value, json};

use super::{
    SkillArchitectureDecision, SkillArchitecturePlan, SkillChangeBundle, SkillChangeDraft,
};
use crate::schema::RunxSchema;

#[test]
fn skill_authoring_contract_rejects_unknown_nested_fields() {
    let mut value = architecture_fixture();
    value["knowledge_contract"]["unowned"] = json!(true);
    assert!(serde_json::from_value::<SkillArchitectureDecision>(value).is_err());
}

#[test]
fn skill_authoring_contract_binds_closed_bundle_and_plan() {
    let value = json!({
        "schema": "runx.skill.change_bundle.v1",
        "decision": "write",
        "base_digest": digest('a'),
        "plan_digest": digest('b'),
        "architecture": architecture_fixture(),
        "summary": "Create the bounded package.",
        "non_goals": ["Do not add a provider adapter."],
        "writes": [
            { "path": "SKILL.md", "contents": "---\nname: demo\ndescription: demo fixture.\n---\n" },
            { "path": "run.yaml", "contents": "skill: demo\n" }
        ],
        "deletes": [],
        "expected_outputs": [
            { "name": "decision", "value_type": "object", "packet": "demo.decision.v1" }
        ]
    });
    assert!(serde_json::from_value::<SkillChangeBundle>(value.clone()).is_ok());

    let mut unknown = value;
    unknown["writes"][0]["mode"] = json!("executable");
    assert!(serde_json::from_value::<SkillChangeBundle>(unknown).is_err());
}

#[test]
fn skill_authoring_generated_objects_are_recursively_closed() {
    assert_closed_objects(&SkillArchitectureDecision::json_schema());
    assert_closed_objects(&SkillArchitecturePlan::json_schema());
    assert_closed_objects(&SkillChangeDraft::json_schema());
    assert_closed_objects(&SkillChangeBundle::json_schema());
}

fn architecture_fixture() -> Value {
    json!({
        "schema": "runx.skill.architecture_decision.v1",
        "disposition": "build",
        "identity": {
            "current_name": null,
            "proposed_name": "demo",
            "action": "create",
            "visibility": "public",
            "rationale": "Demo is the natural operation name for the fixture."
        },
        "direct_use": {
            "trigger_requests": ["Make a bounded demo decision."],
            "non_trigger_requests": ["Publish this decision."],
            "default_outcome": "Return one bounded decision.",
            "routine_host_work": ["Inspect the supplied objective."],
            "runx_boundary": "Bind the result and evidence in a receipt.",
            "terminal_result": "A reviewable demo decision.",
            "blocker_behavior": "Block once with the missing evidence named.",
            "native_escape": "Return the gathered evidence for ordinary host continuation."
        },
        "chain_use": {
            "accepted_inputs": ["A supplied objective or prior evidence packet."],
            "result": "A reusable demo decision.",
            "reused_evidence": ["Prior objective evidence."],
            "reused_effects": [],
            "must_not_repeat": ["Do not rediscover supplied objective evidence."]
        },
        "objective": "Create a bounded decision skill.",
        "operator_value": "Turn supplied evidence into one reviewable decision.",
        "knowledge_contract": {
            "purpose": "Guide the operator through the bounded decision.",
            "evidence_required": ["A supplied objective."],
            "decision_logic": ["Preserve the objective exactly."],
            "stop_conditions": ["Stop when evidence is missing."],
            "recovery": ["Resume with the missing evidence."]
        },
        "required_behaviors": [{
            "id": "decide",
            "outcome": "Produce the decision packet.",
            "lane": "agent"
        }],
        "native_reuse": {
            "inspected_capabilities": ["runx.skill.inspect"],
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
            "max_files": 4,
            "max_executable_lines": 0,
            "max_process_spawns": 0,
            "network_allowed": false
        },
        "preservation_obligations": ["Keep the manual substantive."],
        "deletions": [],
        "proof_plan": [
            {
                "name": "cold-selection",
                "kind": "selection_trial",
                "expected": "A natural demo request selects this skill and a publish-only request does not."
            },
            {
                "name": "standalone-result",
                "kind": "standalone_operator_journey",
                "expected": "The direct request returns the bounded domain decision."
            },
            {
                "name": "composed-reuse",
                "kind": "composed_operator_journey",
                "expected": "Prior evidence is reused without rediscovery."
            }
        ]
    })
}

fn digest(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}

fn assert_closed_objects(value: &Value) {
    match value {
        Value::Object(object) => {
            if object.get("type") == Some(&Value::String("object".to_owned())) {
                assert_eq!(
                    object.get("additionalProperties"),
                    Some(&Value::Bool(false)),
                    "open object schema: {value}"
                );
            }
            for child in object.values() {
                assert_closed_objects(child);
            }
        }
        Value::Array(values) => values.iter().for_each(assert_closed_objects),
        _ => {}
    }
}
