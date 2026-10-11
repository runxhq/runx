use std::sync::{Arc, Barrier};
use std::thread;

use runx_contracts::{EnvironmentRequirements, JsonNumber, JsonObject, JsonValue};
use runx_runtime::RuntimeError;

use crate::javascript_worker_support::{JavaScriptPackage, expected_json, success_json};

#[test]
fn javascript_worker_reuses_a_process_without_reusing_javascript_state()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new(
        "export default ({ value }) => { const prior = globalThis.runxLeak; globalThis.runxLeak = value; return { prior: prior ?? null, value }; };",
    )?;

    let first = package.invoke(JsonObject::from([(
        "value".to_owned(),
        JsonValue::String("first".to_owned()),
    )]))?;
    let second = package.invoke(JsonObject::from([(
        "value".to_owned(),
        JsonValue::String("second".to_owned()),
    )]))?;

    assert_eq!(
        success_json(&first)?,
        expected_json(serde_json::json!({"prior": null, "value": "first"}))
    );
    assert_eq!(
        success_json(&second)?,
        expected_json(serde_json::json!({"prior": null, "value": "second"}))
    );
    assert_eq!(package.session_stats().spawned_process_count, 1);
    Ok(())
}

#[test]
fn javascript_receives_only_exact_manifest_declared_environment()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new(
        "export default (_inputs, context) => ({ environment: context.environment, frozen: Object.isFrozen(context.environment) });",
    )?;
    let output = package.invoke_with_environment(
        JsonObject::new(),
        EnvironmentRequirements {
            required: vec!["REGION".to_owned()],
            optional: vec!["TRACE_LABEL".to_owned()],
        },
        [
            ("REGION".to_owned(), " ap-southeast-2 ".to_owned()),
            ("TRACE_LABEL".to_owned(), "München,prod".to_owned()),
            ("UNDECLARED_SECRET".to_owned(), "must-not-cross".to_owned()),
        ]
        .into_iter()
        .collect(),
    )?;

    assert_eq!(
        success_json(&output)?,
        expected_json(serde_json::json!({
            "environment": {
                "REGION": " ap-southeast-2 ",
                "TRACE_LABEL": "München,prod"
            },
            "frozen": true
        }))
    );
    Ok(())
}

#[test]
fn javascript_fails_before_worker_execution_when_required_environment_is_missing()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new("export default () => ({ executed: true });")?;
    let result = package.invoke_with_environment(
        JsonObject::new(),
        EnvironmentRequirements {
            required: vec!["REQUIRED_VALUE".to_owned()],
            optional: vec!["OPTIONAL_VALUE".to_owned()],
        },
        [("OPTIONAL_VALUE".to_owned(), "present".to_owned())]
            .into_iter()
            .collect(),
    );
    let error = result
        .err()
        .ok_or("missing required environment did not fail closed")?;

    assert!(matches!(
        error,
        RuntimeError::MissingEnvironment { names }
            if names == vec!["REQUIRED_VALUE".to_owned()]
    ));
    assert_eq!(package.session_stats().spawned_process_count, 0);
    Ok(())
}

#[test]
fn javascript_omits_absent_optional_environment_without_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new(
        "export default (_inputs, context) => ({ keys: Object.keys(context.environment) });",
    )?;
    let output = package.invoke_with_environment(
        JsonObject::new(),
        EnvironmentRequirements {
            required: Vec::new(),
            optional: vec!["OPTIONAL_VALUE".to_owned()],
        },
        Default::default(),
    )?;

    assert_eq!(
        success_json(&output)?,
        expected_json(serde_json::json!({ "keys": [] }))
    );
    Ok(())
}

#[test]
fn javascript_session_isolates_concurrent_invocations_in_bounded_workers()
-> Result<(), Box<dyn std::error::Error>> {
    let package = Arc::new(JavaScriptPackage::with_max_concurrency(
        "export default ({ value, rounds }) => { let digest = 0; for (let i = 0; i < rounds; i += 1) digest = (digest + i) % 1000003; return { value, digest }; };",
        4,
    )?);
    let barrier = Arc::new(Barrier::new(5));
    let handles = (0_u64..4)
        .map(|value| {
            let package = package.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                package.invoke(JsonObject::from([
                    (
                        "value".to_owned(),
                        JsonValue::Number(JsonNumber::U64(value)),
                    ),
                    (
                        "rounds".to_owned(),
                        // Keep the four invocations overlapping without making
                        // scheduler contention compete with the wall-limit contract.
                        JsonValue::Number(JsonNumber::U64(25_000)),
                    ),
                ]))
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();

    let mut failures = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(Ok(output)) if output.succeeded() => {}
            Ok(Ok(output)) => {
                failures.push(output.failure_message().unwrap_or_else(|| {
                    "JavaScript invocation failed without diagnostic".to_owned()
                }))
            }
            Ok(Err(error)) => failures.push(error.to_string()),
            Err(_) => failures.push("JavaScript invocation thread panicked".to_owned()),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let stats = package.session_stats();
    assert_eq!(stats.spawned_process_count, 4);
    assert_eq!(stats.peak_in_flight, 4);
    Ok(())
}

#[test]
fn javascript_worker_resolves_only_the_validated_in_memory_bundle()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new(
        "export default () => ({ now: Date.now(), process: typeof process, fetch: typeof fetch, require: typeof require });",
    )?;
    let output = package.invoke(JsonObject::new())?;

    assert_eq!(
        success_json(&output)?,
        expected_json(serde_json::json!({
            "now": 0,
            "process": "undefined",
            "fetch": "undefined",
            "require": "undefined"
        }))
    );
    Ok(())
}

#[test]
fn javascript_worker_resolves_static_relative_imports_from_the_validated_bundle()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::with_modules(
        "import { answer } from './lib/answer.mjs'; export default () => ({ answer });",
        [("lib/answer.mjs", "export const answer = 42;")],
    )?;
    let output = package.invoke(JsonObject::new())?;

    assert_eq!(
        success_json(&output)?,
        expected_json(serde_json::json!({"answer": 42}))
    );
    Ok(())
}

#[test]
fn volume_independent_artifacts_drive_one_worker_across_bounded_pages()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new(
        "export default (inputs) => {\n  const page = inputs.runx_page;\n  const state = page.state ?? { count: 0, sum: 0 };\n  for (const raw of page.records) { const value = JSON.parse(raw); state.count += 1; state.sum += value.value; }\n  const runx_page = { state };\n  return page.eof ? { runx_page, result: state } : { runx_page };\n};",
    )?;
    let records = (0_u64..20_000)
        .map(|value| format!("{{\"value\":{value},\"padding\":\"{}\"}}", "x".repeat(32)))
        .collect::<Vec<_>>();
    let archive = format!("window.YTD.items.part0 = [{}]", records.join(","));
    assert!(
        archive.len() > 64 * 1024,
        "the archive spans more than one page"
    );

    let output = package.invoke_paged("archive.data", &archive, 64 * 1024, JsonObject::new())?;

    assert_eq!(
        success_json(&output)?,
        expected_json(serde_json::json!({
            "result": {
                "count": 20_000,
                "sum": 199_990_000_u64
            }
        }))
    );
    assert_eq!(package.session_stats().spawned_process_count, 1);
    assert!(!output.rendered_value().contains("archive.data"));
    Ok(())
}
