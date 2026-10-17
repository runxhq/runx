use super::*;

fn modules(source: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("main.mjs".to_owned(), source.to_owned())])
}

#[test]
fn rejects_integer_precision_loss_before_any_javascript_runs() {
    for value in [
        serde_json::json!({"id": 9_007_199_254_740_993_u64}),
        serde_json::json!({"nested": [i64::MIN, i64::MAX]}),
        serde_json::json!({"maximum": u64::MAX}),
    ] {
        let error = evaluate(
            "main.mjs",
            "default",
            &modules("export default () => { throw new Error('module executed'); };"),
            value,
            BTreeMap::new(),
            InvocationLimits::default(),
        )
        .err();
        assert!(
            matches!(error, Some(error)
            if error.code == WorkerFailureCode::InvalidRequest
                && error.message.contains("safe range")
                && !error.message.contains("module executed")),
            "an exact integer must be refused before JavaScript executes"
        );
    }
}

#[test]
fn preserves_safe_integer_edges_and_exact_large_integer_strings() -> Result<(), EngineError> {
    let output = evaluate(
        "main.mjs",
        "default",
        &modules("export default (value) => value;"),
        serde_json::json!({"maximum": 9_007_199_254_740_991_i64, "minimum": -9_007_199_254_740_991_i64, "id": "9007199254740993", "large_float": 1.0e18}),
        BTreeMap::new(),
        InvocationLimits::default(),
    )?;
    assert_eq!(output["maximum"].as_f64(), Some(9_007_199_254_740_991_f64));
    assert_eq!(output["minimum"].as_f64(), Some(-9_007_199_254_740_991_f64));
    assert_eq!(output["id"], "9007199254740993");
    assert_eq!(output["large_float"].as_f64(), Some(1.0e18));
    Ok(())
}

#[test]
fn evaluates_default_export_with_fixed_time() -> Result<(), EngineError> {
    let output = evaluate(
        "main.mjs",
        "default",
        &modules("export default ({ value }) => ({ value, now: Date.now() });"),
        serde_json::json!({"value": "runx"}),
        BTreeMap::new(),
        InvocationLimits::default(),
    )?;
    assert_eq!(output, serde_json::json!({"value": "runx", "now": 0}));
    Ok(())
}

#[test]
fn local_time_is_utc_whatever_the_host_zone() -> Result<(), EngineError> {
    let output = evaluate(
        "main.mjs",
        "default",
        &modules(
            "export default () => { const date = new Date(0); return { offset: date.getTimezoneOffset(), hours: date.getHours(), local: date.toString() }; };",
        ),
        serde_json::json!({}),
        BTreeMap::new(),
        InvocationLimits::default(),
    )?;
    assert_eq!(output["offset"].as_f64(), Some(0.0));
    assert_eq!(output["hours"].as_f64(), Some(0.0));
    Ok(())
}

#[test]
fn resolves_relative_modules_from_memory() -> Result<(), EngineError> {
    let bundle = BTreeMap::from([
        (
            "domain/main.mjs".to_owned(),
            "import { value } from './value.mjs'; export default () => ({ value });".to_owned(),
        ),
        (
            "domain/value.mjs".to_owned(),
            "export const value = 42;".to_owned(),
        ),
    ]);
    let output = evaluate(
        "domain/main.mjs",
        "default",
        &bundle,
        serde_json::json!({}),
        BTreeMap::new(),
        InvocationLimits::default(),
    )?;
    assert_eq!(output, serde_json::json!({"value": 42}));
    Ok(())
}

#[test]
fn rejects_host_randomness() {
    let error = evaluate(
        "main.mjs",
        "default",
        &modules("export default () => Math.random();"),
        serde_json::json!({}),
        BTreeMap::new(),
        InvocationLimits::default(),
    )
    .err()
    .map(|error| error.to_string());
    assert!(error.is_some_and(|message| message.contains("Math.random")));
}

#[test]
fn exposes_one_frozen_deterministic_url_parser() -> Result<(), EngineError> {
    let output = evaluate(
        "main.mjs",
        "default",
        &modules(
            r#"
                export default () => ({
                    frozen: Object.isFrozen(Runx),
                    helperFrozen: Object.isFrozen(Runx.parseUrl),
                    bootstrapLeaked: "__runxParseUrl" in globalThis,
                    parsed: Runx.parseUrl("https://Example.com:8443/a?q=1#fragment")
                });
            "#,
        ),
        serde_json::json!({}),
        BTreeMap::new(),
        InvocationLimits::default(),
    )?;
    assert_eq!(
        output,
        serde_json::json!({
            "frozen": true,
            "helperFrozen": true,
            "bootstrapLeaked": false,
            "parsed": {
                "href": "https://example.com:8443/a?q=1#fragment",
                "origin": "https://example.com:8443",
                "protocol": "https:",
                "hostname": "example.com",
                "username": "",
                "password": ""
            }
        })
    );
    Ok(())
}

#[test]
fn rejects_invalid_urls_at_the_worker_boundary() {
    let error = evaluate(
        "main.mjs",
        "default",
        &modules("export default () => Runx.parseUrl('not an absolute URL');"),
        serde_json::json!({}),
        BTreeMap::new(),
        InvocationLimits::default(),
    )
    .err()
    .map(|error| error.to_string());
    assert!(error.is_some_and(|message| message.contains("requires an absolute URL")));
}

#[test]
fn passes_exact_declared_environment_as_frozen_context() -> Result<(), EngineError> {
    let output = evaluate(
        "main.mjs",
        "default",
        &modules(
            "export default (_inputs, context) => ({ environment: context.environment, contextFrozen: Object.isFrozen(context), environmentFrozen: Object.isFrozen(context.environment) });",
        ),
        serde_json::json!({}),
        BTreeMap::from([
            (
                "mixed_Case".to_owned(),
                " value,with punctuation ".to_owned(),
            ),
            ("UNICODE".to_owned(), "München".to_owned()),
        ]),
        InvocationLimits::default(),
    )?;
    assert_eq!(
        output,
        serde_json::json!({
            "environment": {
                "mixed_Case": " value,with punctuation ",
                "UNICODE": "München"
            },
            "contextFrozen": true,
            "environmentFrozen": true
        })
    );
    Ok(())
}

#[test]
fn a_flood_of_jobs_is_reported_as_the_queued_jobs_limit() {
    let error = evaluate(
        "main.mjs",
        "default",
        &modules(
            "export default async function () { for (let i = 0; i < 64; i++) { await Promise.resolve(i); } return {}; }",
        ),
        serde_json::json!({}),
        BTreeMap::new(),
        InvocationLimits {
            queued_jobs: 8,
            ..InvocationLimits::default()
        },
    )
    .err();
    let message = error
        .as_ref()
        .map(|error| error.message.clone())
        .unwrap_or_default();
    assert_eq!(
        error.map(|error| error.code),
        Some(WorkerFailureCode::ResourceLimit),
        "{message}"
    );
}

#[test]
fn a_timer_is_refused_not_reported_as_a_limit() {
    let error = evaluate(
        "main.mjs",
        "default",
        &modules("export default () => new Promise((resolve) => setTimeout(resolve, 1));"),
        serde_json::json!({}),
        BTreeMap::new(),
        InvocationLimits::default(),
    )
    .err();
    assert_eq!(
        error.map(|error| error.code),
        Some(WorkerFailureCode::ExecutionFailed)
    );
}
