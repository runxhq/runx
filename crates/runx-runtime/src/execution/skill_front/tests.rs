#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use runx_contracts::{JsonNumber, JsonValue};

use super::runner_inputs_from_text;

/// A skill whose runner declares a string code and an integer count.
fn skill() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("SKILL.md"),
        "---\nname: typed\ndescription: Reads typed inputs.\n---\n\n# Typed\n\nRead the inputs.\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("run.yaml"),
        "skill: typed\nversion: 1.0.0\nrunners:\n  run:\n    default: true\n    inputs:\n      code: {type: string, required: true}\n      count: {type: integer, required: true}\n    steps:\n      - id: read\n        agent: {role: reader, task: read}\n",
    )
    .unwrap();
    root
}

fn texts(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
        .collect()
}

#[test]
fn each_text_is_read_by_its_declared_type() {
    let skill = skill();
    let inputs = runner_inputs_from_text(
        skill.path(),
        None,
        &texts(&[("code", "007"), ("count", "7")]),
    )
    .unwrap();
    assert_eq!(inputs["code"], JsonValue::String("007".to_owned()));
    assert_eq!(inputs["count"], JsonValue::Number(JsonNumber::U64(7)));
}

#[test]
fn a_text_that_is_not_its_type_or_not_declared_is_refused() {
    let skill = skill();
    let wrong = runner_inputs_from_text(skill.path(), None, &texts(&[("count", "seven")]))
        .unwrap_err()
        .to_string();
    assert!(wrong.contains("input count is integer"), "{wrong}");
    let undeclared = runner_inputs_from_text(skill.path(), None, &texts(&[("limit", "3")]))
        .unwrap_err()
        .to_string();
    assert!(
        undeclared.contains("runner run declares no input limit; it takes code, count"),
        "{undeclared}"
    );
}
