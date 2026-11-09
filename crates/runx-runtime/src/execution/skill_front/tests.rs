#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use runx_contracts::{JsonNumber, JsonValue};

use super::method_inputs_from_text;

/// A skill whose method declares a string code and an integer count.
fn skill() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("SKILL.md"),
        "---\nname: typed\ndescription: Reads typed inputs.\n---\n\n# Typed\n\n## Agent task: read\n\nRead the inputs.\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("run.yaml"),
        "version: 1.0.0\nminimum_runx: 0.12.0\nmethods:\n  run:\n    inputs:\n      code: {type: string}\n      count: {type: integer}\n    steps:\n      - id: read\n        agent: read\n",
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
    let inputs = method_inputs_from_text(
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
    let wrong = method_inputs_from_text(skill.path(), None, &texts(&[("count", "seven")]))
        .unwrap_err()
        .to_string();
    assert!(wrong.contains("input count is integer"), "{wrong}");
    let undeclared = method_inputs_from_text(skill.path(), None, &texts(&[("limit", "3")]))
        .unwrap_err()
        .to_string();
    assert!(
        undeclared.contains("method run declares no input limit; it takes code, count"),
        "{undeclared}"
    );
}

/// A run starts a worker exactly when its own method can run JavaScript: a
/// run of a catalog tool starts none and records none, and a run of a worker
/// tool starts one, which also runs the tool, and records its digest.
#[test]
fn a_run_starts_a_worker_only_when_its_method_can_run_javascript() {
    let root = tempfile::tempdir().unwrap();
    let env: BTreeMap<String, String> = [
        ("RUNX_CWD".to_owned(), root.path().display().to_string()),
        (
            "RUNX_HOME".to_owned(),
            root.path().join("home").display().to_string(),
        ),
    ]
    .into();
    let runs = [
        (
            "digest",
            "        tool: data.digest\n        inputs:\n          value: $input.value\n          encoding: utf8_text\n",
            0,
            None,
        ),
        (
            "echo",
            "        tool: {module: echo.mjs}\n        inputs:\n          value: $input.value\n        outputs:\n          value: {type: string}\n",
            1,
            Some(env!("RUNX_JS_WORKER_SOURCE_DIGEST")),
        ),
    ];
    for (name, step, spawned, worker) in runs {
        let skill = root.path().join(name);
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: A {name} step.\n---\n# {name}\n"),
        )
        .unwrap();
        std::fs::write(
            skill.join("run.yaml"),
            format!(
                "minimum_runx: 0.12.0\nmethods:\n  main:\n    inputs:\n      value: {{type: string}}\n    steps:\n      - id: main\n{step}"
            ),
        )
        .unwrap();
        if worker.is_some() {
            std::fs::write(
                skill.join("echo.mjs"),
                "export default ({ value }) => ({ value });\n",
            )
            .unwrap();
        }
        let javascript = crate::adapters::javascript::JavaScriptAdapter::new_session();
        let result = super::execute_bound_skill_run_with_overrides(
            &crate::execution::orchestrator::SkillRunRequest {
                current_context: Vec::new(),
                historical_context: Vec::new(),
                run_authority: None,
                act: None,
                skill_path: skill,
                receipt_dir: Some(root.path().join("receipts")),
                run_id: None,
                answers: None,
                inputs: [("value".to_owned(), JsonValue::String("hello".to_owned()))].into(),
                env: env.clone(),
                cwd: root.path().to_path_buf(),
                managed_agent: Default::default(),
            },
            &super::SkillRunOverrides {
                javascript: Some(javascript.clone()),
                ..Default::default()
            },
            &crate::effects::RuntimeEffectRegistry::default(),
            None,
            None,
        )
        .unwrap();
        let output = result.output.as_object().unwrap();
        assert_eq!(
            output["status"].as_str(),
            Some("sealed"),
            "{name}: {output:?}"
        );
        assert_eq!(javascript.spawned_process_count(), spawned, "{name}");
        let run_id = output["run_id"].as_str().unwrap();
        let started = crate::execution::run_record::read_run(&env, root.path(), run_id, None)
            .unwrap()
            .events
            .into_iter()
            .find_map(|record| match record.event {
                runx_contracts::RunEventFact::Started { implementation, .. } => implementation,
                _ => None,
            })
            .unwrap();
        assert_eq!(started.worker.as_deref(), worker, "{name}");
    }
}
