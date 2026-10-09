#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;

use runx_contracts::{JsonObject, JsonValue};

use super::*;

fn steps(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn object(json: serde_json::Value) -> JsonObject {
    serde_json::from_value(json).unwrap()
}

fn at(path: &[&str]) -> Vec<String> {
    path.iter().map(|segment| (*segment).to_owned()).collect()
}

#[test]
fn references_bind_at_any_depth_and_literals_stay() {
    let (template, bindings) = parse_values(
        &object(serde_json::json!({
            "claim_id": "$input.selection.claim_id",
            "request": {"artifacts": "$steps.packet.data.artifacts", "audience": "human"},
            "ids": ["$steps.packet.data.ids.0", 7],
            "note": "$$5 off",
        })),
        &steps(&["packet"]),
        "inputs",
    )
    .expect("values parse");

    assert_eq!(
        bindings,
        [
            Binding {
                at: at(&["claim_id"]),
                source: Source::Input {
                    path: at(&["selection", "claim_id"])
                },
            },
            Binding {
                at: at(&["ids", "0"]),
                source: Source::Step {
                    step: "packet".to_owned(),
                    path: at(&["data", "ids", "0"])
                },
            },
            Binding {
                at: at(&["request", "artifacts"]),
                source: Source::Step {
                    step: "packet".to_owned(),
                    path: at(&["data", "artifacts"])
                },
            },
        ]
    );
    assert_eq!(template["note"], JsonValue::String("$5 off".to_owned()));
    let request = template["request"]
        .as_object()
        .expect("the request stays an object");
    assert_eq!(request["audience"], JsonValue::String("human".to_owned()));
}

#[test]
fn a_whole_value_may_be_one_reference() {
    let (template, bindings) = parse_whole(
        &JsonValue::String("$input".to_owned()),
        &steps(&[]),
        "inputs",
    )
    .expect("parses");
    assert!(template.is_empty());
    assert_eq!(
        bindings,
        [Binding {
            at: Vec::new(),
            source: Source::Input { path: Vec::new() }
        }]
    );
    let (_, bindings) = parse_whole(
        &JsonValue::String("$steps.plan".to_owned()),
        &steps(&["plan"]),
        "inputs",
    )
    .expect("parses");
    assert_eq!(bindings[0].source.reference(), "$steps.plan");
    assert!(
        parse_whole(
            &JsonValue::String("plain".to_owned()),
            &steps(&[]),
            "inputs"
        )
        .is_err()
    );
}

#[test]
fn a_reference_names_an_earlier_step_and_a_known_form() {
    let refuse = |text: &str, expected: &str| {
        let error = parse_values(
            &object(serde_json::json!({"value": text})),
            &steps(&["plan"]),
            "inputs",
        )
        .expect_err("refused")
        .to_string();
        assert!(error.contains(expected), "{text}: {error}");
    };
    refuse(
        "$steps.later.data",
        "references unknown or later step 'later'",
    );
    refuse("$steps", "must name a step");
    refuse("$input..x", "empty path segment");
    refuse("$env.HOME", "is not a reference");
    refuse("$5 off", "is not a reference");
}

#[test]
fn an_operand_is_a_literal_or_one_reference() {
    assert_eq!(
        parse_operand(
            &JsonValue::String("$input.mode".to_owned()),
            &steps(&[]),
            "when.value"
        )
        .unwrap(),
        Operand::Reference(Source::Input {
            path: at(&["mode"])
        })
    );
    assert_eq!(
        parse_operand(
            &JsonValue::String("$$draft".to_owned()),
            &steps(&[]),
            "when.value"
        )
        .unwrap(),
        Operand::Literal(JsonValue::String("$draft".to_owned()))
    );
    assert_eq!(
        parse_operand(&JsonValue::Bool(true), &steps(&[]), "when.value").unwrap(),
        Operand::Literal(JsonValue::Bool(true))
    );
}
