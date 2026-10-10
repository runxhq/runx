use super::*;

fn modules(source: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("main.mjs".to_owned(), source.to_owned())])
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
