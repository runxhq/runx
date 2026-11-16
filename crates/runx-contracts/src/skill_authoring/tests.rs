use serde_json::{Value, json};

use super::{SkillChangeBundle, SkillChangeDraft};
use crate::schema::RunxSchema;

#[test]
fn a_change_bundle_is_its_decision_base_and_files_and_nothing_else() {
    let value = json!({
        "schema": "runx.skill.change_bundle.v1",
        "decision": "write",
        "base_digest": digest('a'),
        "summary": "Create the bounded package.",
        "writes": [
            { "path": "skills/demo/SKILL.md", "contents": "---\nname: demo\ndescription: demo fixture.\n---\n" },
            { "path": "skills/demo/run.yaml", "contents": "minimum_runx: 0.12.0\n" }
        ],
        "deletes": []
    });
    assert!(serde_json::from_value::<SkillChangeBundle>(value.clone()).is_ok());

    let mut unknown = value.clone();
    unknown["writes"][0]["mode"] = json!("executable");
    assert!(serde_json::from_value::<SkillChangeBundle>(unknown).is_err());
    // The architecture form is gone, and its digest with it.
    let mut planned = value;
    planned["plan_digest"] = json!(digest('b'));
    assert!(serde_json::from_value::<SkillChangeBundle>(planned).is_err());
}

#[test]
fn skill_authoring_generated_objects_are_recursively_closed() {
    assert_closed_objects(&SkillChangeDraft::json_schema());
    assert_closed_objects(&SkillChangeBundle::json_schema());
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
