use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use runx_contracts::{JsonObject, JsonValue};
use runx_parser::{SkillPackageSource, validate_skill_package};
use runx_runtime::adapter::InvocationStatus;

use crate::javascript_worker_support::{JavaScriptPackage, expected_json, success_json};

#[test]
fn javascript_worker_hostile_globals_have_no_ambient_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new(
        "export default () => ({ process: typeof process, require: typeof require, fetch: typeof fetch, websocket: typeof WebSocket, timer: typeof setTimeout, crypto: typeof crypto, performance: typeof performance, deno: typeof Deno, bun: typeof Bun });",
    )?;
    let output = package.invoke(JsonObject::new())?;
    assert_eq!(
        success_json(&output)?,
        expected_json(serde_json::json!({
            "process": "undefined",
            "require": "undefined",
            "fetch": "undefined",
            "websocket": "undefined",
            "timer": "undefined",
            "crypto": "undefined",
            "performance": "undefined",
            "deno": "undefined",
            "bun": "undefined"
        }))
    );
    Ok(())
}

#[test]
fn javascript_worker_hostile_imports_are_rejected_before_execution()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "import fs from 'node:fs'; export default () => fs;",
        "import value from 'bare-package'; export default () => value;",
        "import value from 'https://example.com/value.mjs'; export default () => value;",
        "import value from '/etc/passwd'; export default () => value;",
        "import value from '../escape.mjs'; export default () => value;",
        "export default () => import('./other.mjs');",
    ] {
        let package = JavaScriptPackage::new(source)?;
        let error = package.invoke(JsonObject::new()).err();
        assert!(
            error.is_some(),
            "hostile import unexpectedly executed: {source}"
        );
    }
    Ok(())
}

#[test]
fn javascript_worker_hostile_unreferenced_executable_modules_are_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::with_modules(
        "export default () => ({ ok: true });",
        [("unused.mjs", "export default () => 'hidden';")],
    )?;
    let error = package.invoke(JsonObject::new()).err();
    assert!(
        error.is_some_and(|error| error.to_string().contains("not reachable")),
        "unreferenced module unexpectedly entered the worker bundle"
    );
    Ok(())
}

#[test]
fn javascript_worker_hostile_symlink_entries_are_rejected_by_the_aggregate_parser() {
    let source = SkillPackageSource {
        files: BTreeMap::from([
            (
                "SKILL.md".to_owned(),
                b"---\nname: linked-module\ndescription: Reject linked executable sources.\n---\n# Linked module\n".to_vec(),
            ),
            (
                "run.yaml".to_owned(),
                b"skill: linked-module\nrunners:\n  run:\n    default: true\n    steps:\n      - id: run\n        tool: { module: main.mjs }\n".to_vec(),
            ),
        ]),
        symlinks: BTreeSet::from(["main.mjs".to_owned()]),
    };

    let error = validate_skill_package(source).err();
    assert!(
        error.is_some_and(|error| error.to_string().contains("symbolic link")),
        "symlinked executable source unexpectedly validated"
    );
}

#[test]
fn javascript_worker_typed_failures_preserve_the_isolated_session()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new(
        "export default ({ fail }) => { if (fail) return Math.random(); return { state: 'clean' }; };",
    )?;
    let failed = package.invoke(JsonObject::from([(
        "fail".to_owned(),
        JsonValue::Bool(true),
    )]))?;
    assert_eq!(failed.status, InvocationStatus::Failure);
    assert!(
        failed
            .failure_message()
            .is_some_and(|message| message.contains("Math.random"))
    );

    let recovered = package.invoke(JsonObject::from([(
        "fail".to_owned(),
        JsonValue::Bool(false),
    )]))?;
    assert_eq!(
        success_json(&recovered)?,
        expected_json(serde_json::json!({"state": "clean"}))
    );
    assert_eq!(package.session_stats().spawned_process_count, 1);
    Ok(())
}

#[test]
fn javascript_worker_hostile_memory_fault_is_contained_and_recoverable()
-> Result<(), Box<dyn std::error::Error>> {
    let hostile = JavaScriptPackage::new(
        "export default () => { const bytes = new ArrayBuffer(70 * 1024 * 1024); return { bytes: bytes.byteLength }; };",
    )?;
    let result = hostile.invoke(JsonObject::new());
    assert!(
        result.is_err() || result.is_ok_and(|output| output.status == InvocationStatus::Failure),
        "memory limit did not stop the hostile module"
    );

    let clean = JavaScriptPackage::new("export default () => ({ recovered: true });")?;
    let recovered = clean.invoke(JsonObject::new())?;
    assert_eq!(
        success_json(&recovered)?,
        expected_json(serde_json::json!({"recovered": true}))
    );
    Ok(())
}

#[test]
fn javascript_worker_hostile_prototype_state_does_not_cross_invocations()
-> Result<(), Box<dyn std::error::Error>> {
    let package = JavaScriptPackage::new(
        "export default ({ pollute }) => { if (pollute) { Object.prototype.runxPolluted = 'blocked'; return { polluted: true }; } return { inherited: Object.prototype.runxPolluted ?? null }; };",
    )?;
    success_json(&package.invoke(JsonObject::from([(
        "pollute".to_owned(),
        JsonValue::Bool(true),
    )]))?)?;

    assert_eq!(
        success_json(&package.invoke(JsonObject::from([(
            "pollute".to_owned(),
            JsonValue::Bool(false),
        )]))?)?,
        expected_json(serde_json::json!({"inherited": null}))
    );
    assert_eq!(package.session_stats().spawned_process_count, 1);
    Ok(())
}

#[test]
fn javascript_worker_hostile_promises_are_immediate_and_job_bounded()
-> Result<(), Box<dyn std::error::Error>> {
    let immediate = JavaScriptPackage::new(
        "export default async function execute({ value }) { return { value: await Promise.resolve(value) }; }",
    )?;
    let inputs = JsonObject::from([("value".to_owned(), JsonValue::String("settled".to_owned()))]);
    assert_eq!(
        success_json(&immediate.invoke(inputs)?)?,
        expected_json(serde_json::json!({"value": "settled"}))
    );

    let pending = JavaScriptPackage::new("export default () => new Promise(() => {});")?;
    let output = pending.invoke(JsonObject::new())?;
    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(
        output
            .failure_message()
            .is_some_and(|message| message.contains("did not settle"))
    );

    let storm = JavaScriptPackage::new(
        "export default () => { let value = Promise.resolve(0); for (let index = 0; index < 5000; index += 1) value = value.then(number => number + 1); return value; };",
    )?;
    let output = storm.invoke(JsonObject::new())?;
    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(
        output
            .failure_message()
            .is_some_and(|message| message.contains("job"))
    );
    Ok(())
}

#[test]
fn javascript_worker_hostile_infinite_loop_is_bounded_and_recoverable()
-> Result<(), Box<dyn std::error::Error>> {
    let hostile = JavaScriptPackage::new("export default () => { while (true) {} };")?;
    let started = Instant::now();
    let result = hostile.invoke(JsonObject::new());
    assert!(
        result.is_err() || result.is_ok_and(|output| output.status == InvocationStatus::Failure),
        "infinite loop unexpectedly completed"
    );
    assert!(started.elapsed() < Duration::from_secs(5));

    let clean = JavaScriptPackage::new("export default () => ({ recovered: 'loop' });")?;
    assert_eq!(
        success_json(&clean.invoke(JsonObject::new())?)?,
        expected_json(serde_json::json!({"recovered": "loop"}))
    );
    Ok(())
}

#[test]
fn javascript_worker_hostile_input_and_output_bounds_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let echo = JavaScriptPackage::new("export default ({ value }) => ({ value });")?;
    let oversized_input = JsonObject::from([(
        "value".to_owned(),
        JsonValue::String("x".repeat((4 * 1024 * 1024) + 1)),
    )]);
    let input_output = echo.invoke(oversized_input)?;
    assert_eq!(input_output.status, InvocationStatus::Failure);
    assert!(
        input_output
            .failure_message()
            .is_some_and(|message| message.contains("JavaScript input is")),
        "worker input limit failure omitted its ceiling"
    );

    let accepted_source = format!(
        "export default () => ({{ accepted: true }}); /*{}*/",
        "x".repeat((1024 * 1024) + 1)
    );
    let accepted_package = JavaScriptPackage::new(&accepted_source)?;
    assert_eq!(
        success_json(&accepted_package.invoke(JsonObject::new())?)?,
        expected_json(serde_json::json!({"accepted": true}))
    );

    let oversized_source = format!(
        "export default () => ({{ ok: true }}); /*{}*/",
        "x".repeat(4 * 1024 * 1024)
    );
    let source_package = JavaScriptPackage::new(&oversized_source)?;
    let source_result = source_package.invoke(JsonObject::new());
    assert!(
        source_result.is_err()
            || source_result.is_ok_and(|output| output.status == InvocationStatus::Failure),
        "oversized source unexpectedly entered the engine"
    );

    let oversized_output = JavaScriptPackage::new(
        "export default () => ({ value: 'x'.repeat((4 * 1024 * 1024) + 1) });",
    )?;
    let result = oversized_output.invoke(JsonObject::new());
    assert!(
        result.is_err() || result.is_ok_and(|output| output.status == InvocationStatus::Failure),
        "oversized output unexpectedly crossed the protocol"
    );

    let clean = JavaScriptPackage::new("export default () => ({ recovered: 'bounds' });")?;
    assert_eq!(
        success_json(&clean.invoke(JsonObject::new())?)?,
        expected_json(serde_json::json!({"recovered": "bounds"}))
    );
    Ok(())
}

#[test]
fn javascript_worker_hostile_stack_growth_is_bounded_and_recoverable()
-> Result<(), Box<dyn std::error::Error>> {
    let recursive = JavaScriptPackage::new(
        "function recurse() { return recurse(); } export default () => recurse();",
    )?;
    let result = recursive.invoke(JsonObject::new());
    assert!(
        result.is_err() || result.is_ok_and(|output| output.status == InvocationStatus::Failure),
        "unbounded recursion unexpectedly completed"
    );

    let clean = JavaScriptPackage::new("export default () => ({ recovered: 'stack' });")?;
    assert_eq!(
        success_json(&clean.invoke(JsonObject::new())?)?,
        expected_json(serde_json::json!({"recovered": "stack"}))
    );
    Ok(())
}

#[test]
fn javascript_worker_hostile_non_json_and_stdout_surfaces_cannot_pollute_protocol()
-> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "export default () => 1n;",
        "export default () => Symbol('not-json');",
        "export default () => function notJson() {};",
        "export default () => { const value = {}; value.self = value; return value; };",
        "export default () => console.log('protocol pollution');",
    ] {
        let package = JavaScriptPackage::new(source)?;
        let output = package.invoke(JsonObject::new())?;
        assert_eq!(output.status, InvocationStatus::Failure, "{source}");
    }

    let clean = JavaScriptPackage::new("export default () => ({ protocol: 'clean' });")?;
    assert_eq!(
        success_json(&clean.invoke(JsonObject::new())?)?,
        expected_json(serde_json::json!({"protocol": "clean"}))
    );
    Ok(())
}

#[test]
fn javascript_worker_hostile_undefined_output_is_rejected() -> Result<(), Box<dyn std::error::Error>>
{
    let package = JavaScriptPackage::new("export default () => undefined;")?;
    let output = package.invoke(JsonObject::new())?;
    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(
        output
            .failure_message()
            .is_some_and(|message| message.contains("not JSON-compatible"))
    );
    Ok(())
}
