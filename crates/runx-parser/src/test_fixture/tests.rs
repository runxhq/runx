#![allow(clippy::expect_used)]

use super::*;

#[test]
fn output_expectations_preserve_explicit_null() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = parse_test_fixture("name: null-result\nexpect:\n  output: {exact: null}\n")?;
    let expected = TestJsonExpectation::Exact {
        exact: JsonValue::Null,
    };
    assert_eq!(fixture.expect.output.as_ref(), Some(&expected));
    assert_eq!(
        serde_json::to_value(expected)?,
        serde_json::json!({"exact": null})
    );
    Ok(())
}

#[test]
fn every_json_expectation_requires_exactly_one_mode() {
    for invalid in ["{}", "{exact: 1, subset: 1}"] {
        for fixture in [
            format!("name: output\nexpect:\n  output: {invalid}\n"),
            format!("name: step\nexpect:\n  step_outputs:\n    work: {invalid}\n"),
            format!(
                "name: turn\nhost:\n  turns:\n    work:\n      calls:\n        - tool: data.echo\n          expect: {invalid}\n      output: {{}}\nexpect:\n  status: sealed\n"
            ),
        ] {
            assert!(parse_test_fixture(&fixture).is_err(), "{fixture}");
        }
    }
}

#[test]
fn validates_receipt_and_output_expectations() -> Result<(), TestFixtureError> {
    let fixture = parse_test_fixture(
        r#"
name: echo-skill
expect:
  status: sealed
  receipt:
    schema: runx.receipt.v1
    state: sealed
    outcome: completed
  output:
    subset: { status: ready }
"#,
    )?;

    // A test runs its own package unless it names a fixture package.
    assert_eq!(fixture.package, None);
    assert!(fixture.setup.receipts.is_empty());
    assert!(fixture.expect.receipt.is_some());
    assert!(fixture.expect.output.is_some());
    Ok(())
}

#[test]
fn expectations_use_the_conventional_contract() -> Result<(), TestFixtureError> {
    let expectation = parse_test_fixture(
        r#"
name: inspect
expect:
  status: sealed
  receipt:
    schema: runx.receipt.v1
    outcome: completed
  step_outputs:
    inspect:
      subset: { status: ready }
"#,
    )?
    .expect;

    assert_eq!(expectation.status, Some(TestExpectedStatus::Sealed));
    assert_eq!(
        expectation
            .receipt
            .as_ref()
            .and_then(|receipt| receipt.outcome.as_ref()),
        Some(&RunOutcome::Completed)
    );
    assert!(expectation.step_outputs.contains_key("inspect"));
    Ok(())
}

#[test]
fn validates_package_relative_receipt_setup() -> Result<(), TestFixtureError> {
    let fixture = parse_test_fixture(
        "name: receipt-proof\nsetup:\n  receipts:\n    - fixtures/receipt-store/sha256-demo.json\nexpect:\n  status: sealed\n",
    )?;

    assert_eq!(
        fixture.setup.receipts,
        vec!["fixtures/receipt-store/sha256-demo.json"]
    );
    Ok(())
}

#[test]
fn rejects_receipt_setup_path_escape() {
    let error = parse_test_fixture(
        "name: receipt-proof\nsetup:\n  receipts:\n    - ../receipt.json\nexpect:\n  status: sealed\n",
    )
    .expect_err("receipt setup path escape must fail");

    assert!(
        matches!(error, TestFixtureError::Invalid { field, .. } if field == "setup.receipts[0]")
    );
}

#[test]
fn admits_request_sensitive_test_http_exchanges() -> Result<(), TestFixtureError> {
    let fixture = parse_test_fixture(
        r#"
name: deterministic-mcp
providers:
  http:
    - request:
        method: POST
        url: https://fixture.runx.invalid/mcp
        body:
          json: { jsonrpc: "2.0", method: tools/call, params: { name: billing } }
      response:
        status: 200
        headers: { content-type: application/json }
        body: '{"jsonrpc":"2.0","result":{"ok":true}}'
    - request:
        method: DELETE
        url: https://fixture.runx.invalid/mcp
        body: none
      response: { status: 204, body: "" }
    - request:
        method: POST
        url: https://fixture.runx.invalid/mcp
        body: { json: null }
      response: { status: 200, body: '{"deleted":true}' }
expect:
  status: sealed
"#,
    )?;

    let exchanges = &fixture.providers.http;
    assert_eq!(exchanges.len(), 3);
    assert_eq!(exchanges[0].request.method, "POST");
    assert_eq!(exchanges[0].request.url, "https://fixture.runx.invalid/mcp");
    assert_eq!(exchanges[0].response.status, 200);
    Ok(())
}

#[test]
fn test_http_exchanges_reject_malformed_urls_and_bound_characters() {
    for malformed in [
        "https://",
        "https://?query",
        "ftp://fixture.runx.invalid/source",
        "https://fixture.runx.invalid/source\u{0007}",
        "https://user:pass@fixture.runx.invalid/source",
        "https://fixture.runx.invalid/source#fragment",
    ] {
        assert!(
            validate_test_http_url(malformed, "providers.http[0].request.url").is_err(),
            "malformed URL must fail: {malformed:?}",
        );
    }

    let prefix = "https://fixture.runx.invalid/";
    let at_limit = format!("{prefix}{}", "é".repeat(2048 - prefix.chars().count()));
    assert!(validate_test_http_url(&at_limit, "providers.http[0].request.url").is_ok());
    let over_limit = format!("{at_limit}é");
    assert!(validate_test_http_url(&over_limit, "providers.http[0].request.url").is_err());
}

#[test]
fn test_http_exchanges_reject_credentials_and_fragments() {
    for url in [
        "https://user:pass@fixture.runx.invalid/source",
        "https://fixture.runx.invalid/source#fragment",
    ] {
        let fixture = format!(
            r#"
name: unreachable-url
providers:
  http:
    - request: {{ method: GET, url: "{url}", body: none }}
      response: {{ status: 200, body: unreachable }}
expect:
  status: sealed
"#,
        );
        assert!(
            parse_test_fixture(&fixture).is_err(),
            "URL must fail: {url:?}",
        );
    }
}

#[test]
fn test_http_exchanges_canonicalize_final_urls_so_an_alias_is_one_request()
-> Result<(), TestFixtureError> {
    for (first, alias) in [
        ("https://api.example.com", "https://api.example.com/"),
        ("https://API.example.com/mcp", "https://api.example.com/mcp"),
        (
            "https://api.example.com/π",
            "https://api.example.com/%CF%80",
        ),
    ] {
        let fixture = format!(
            r#"
name: canonical-url-alias
providers:
  http:
    - request: {{ method: POST, url: "{first}", body: none }}
      response: {{ status: 200, body: first }}
    - request: {{ method: POST, url: "{alias}", body: none }}
      response: {{ status: 200, body: alias }}
expect:
  status: sealed
"#,
        );
        // An alias is the same final URL, so both answer one request, in order.
        let parsed = parse_test_fixture(&fixture)?;
        let exchanges = &parsed.providers.http;
        assert_eq!(exchanges[0].request.url, exchanges[1].request.url);
    }
    Ok(())
}

#[test]
fn test_http_exchanges_enforce_count_and_body_bounds() {
    let entries = (0..33)
        .map(|index| {
            format!(
                "    - request:\n        method: POST\n        url: https://fixture.runx.invalid/mcp\n        body: {{ json: {{ index: {index} }} }}\n      response: {{ status: 200, body: ok }}\n"
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let too_many = format!(
        "name: too-many-exchanges\nproviders:\n  http:\n{entries}expect:\n  status: sealed\n"
    );
    assert!(
        parse_test_fixture(&too_many).is_err(),
        "33 exchanges must exceed the declared bound",
    );

    let oversized = "x".repeat(1_048_576);
    let oversized_body = format!(
        r#"
name: oversized-exchange-body
providers:
  http:
    - request:
        method: POST
        url: https://fixture.runx.invalid/mcp
        body: {{ json: "{oversized}" }}
      response: {{ status: 200, body: ok }}
expect:
  status: sealed
"#,
    );
    assert!(
        parse_test_fixture(&oversized_body).is_err(),
        "serialized JSON over 1 MiB must fail",
    );
}

#[test]
fn test_http_exchanges_reject_unknown_body_fields() {
    let error = parse_test_fixture(
        r#"
name: widened-mcp-body
providers:
  http:
    - request:
        method: POST
        url: https://fixture.runx.invalid/mcp
        body: { json: { operation: status }, extra: ignored }
      response: { status: 200, body: ok }
expect:
  status: sealed
"#,
    )
    .expect_err("unknown body fields must fail instead of changing identity silently");

    // The providers are typed in the test's document, so a body with a field
    // it doesn't know fails to parse.
    assert!(
        matches!(error, TestFixtureError::Parse(_))
            && error.to_string().contains("TestHttpRequestBodyFixture"),
        "{error}"
    );
}

#[test]
fn test_http_exchanges_accept_json_bodies_for_get_and_delete() -> Result<(), TestFixtureError> {
    let fixture = parse_test_fixture(
        r#"
name: method-agnostic-body-identity
providers:
  http:
    - request:
        method: GET
        url: https://fixture.runx.invalid/source
        body: { json: { query: state } }
      response: { status: 200, body: get }
    - request:
        method: DELETE
        url: https://fixture.runx.invalid/source
        body: { json: null }
      response: { status: 200, body: delete }
expect:
  status: sealed
"#,
    )?;
    let exchanges = &fixture.providers.http;
    assert_eq!(exchanges.len(), 2);
    assert_eq!(exchanges[0].request.method, "GET");
    assert_eq!(exchanges[1].request.method, "DELETE");
    Ok(())
}

#[test]
fn admits_a_repeated_request_as_answers_in_order() -> Result<(), TestFixtureError> {
    let fixture = parse_test_fixture(
        r#"
name: read-change-read
providers:
  http:
    - request: &request
        method: GET
        url: https://fixture.runx.invalid/item
        body: none
      response: { status: 200, body: before }
    - request: *request
      response: { status: 200, body: after }
expect:
  status: sealed
"#,
    )?;

    let exchanges = &fixture.providers.http;
    assert_eq!(exchanges.len(), 2);
    assert_eq!(exchanges[0].request.url, exchanges[1].request.url);
    Ok(())
}

#[test]
fn rejects_non_http_test_exchange_urls() {
    let error = parse_test_fixture(
        r#"
name: deterministic-web
providers:
  http:
    - request: { method: GET, url: "file:///private/source", body: none }
      response: { status: 200, body: hidden }
expect:
  status: sealed
"#,
    )
    .expect_err("a non-HTTP exchange must fail");

    assert!(
        matches!(error, TestFixtureError::Invalid { field, .. } if field == "providers.http[0].request.url")
    );
}

#[test]
fn rejects_unknown_host_and_provider_fields() {
    // A test's caller is its host and its providers, and neither takes a
    // field it doesn't know.
    for block in ["host", "providers"] {
        let error = parse_test_fixture(&format!(
            "name: deterministic-web\n{block}:\n  network: true\nexpect:\n  status: sealed\n"
        ))
        .expect_err("an unknown field must fail");
        assert!(
            matches!(error, TestFixtureError::Parse(_))
                && error.to_string().contains("unknown field `network`"),
            "{block}: {error}"
        );
    }
}

#[test]
fn process_calls_are_bounded_and_exact_not_an_order_dependent_fallback()
-> Result<(), TestFixtureError> {
    let prefix = "name: process\nexpect:\n  status: sealed\nproviders:\n  processes:\n";
    let call = "    - tool: source.read\n      arguments: {id: one}\n      result: {value: 7}\n";
    let fixture = parse_test_fixture(&format!("{prefix}{call}"))?;
    assert_eq!(fixture.providers.processes.len(), 1);
    assert!(parse_test_fixture(&format!("{prefix}{call}{call}")).is_err());
    assert!(parse_test_fixture(&format!("{prefix}{}", call.repeat(33))).is_err());
    assert!(parse_test_fixture(&format!("{prefix}{}", call.replace("source.read", "''"))).is_err());
    Ok(())
}

#[test]
fn exact_text_http_bodies_cover_forms_and_remain_byte_bounded() -> Result<(), TestFixtureError> {
    let document = "name: form\nproviders:\n  http:\n    - request:\n        method: POST\n        url: https://provider.example/payment\n        body: {text: 'amount=150&currency=usd'}\n      response: {status: 200, body: '{}'}\nexpect:\n  status: sealed\n";
    let fixture = parse_test_fixture(document)?;
    assert_eq!(
        fixture.providers.http[0].request.body,
        TestHttpRequestBodyFixture::Text {
            text: "amount=150&currency=usd".to_owned()
        }
    );
    let oversized = document.replace("amount=150&currency=usd", &"é".repeat(524289));
    assert!(parse_test_fixture(&oversized).is_err());
    Ok(())
}
