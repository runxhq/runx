#![allow(clippy::expect_used)]

use super::*;

#[test]
fn validates_receipt_and_output_expectations() -> Result<(), HarnessFixtureError> {
    let fixture = parse_harness_fixture(
        r#"
name: echo-skill
kind: skill
target: ../skills/echo
expect:
  status: sealed
  receipt:
    schema: runx.receipt.v1
    state: sealed
    disposition: closed
  output:
    subset: { status: ready }
"#,
    )?;

    assert_eq!(fixture.kind, HarnessFixtureKind::Skill);
    assert!(fixture.setup.receipts.is_empty());
    assert!(fixture.expect.receipt.is_some());
    assert!(fixture.expect.output.is_some());
    Ok(())
}

#[test]
fn inline_expectations_use_the_conventional_contract() -> Result<(), HarnessFixtureError> {
    let expectation = parse_harness_expectation(JsonObject::from([
        ("status".to_owned(), JsonValue::String("sealed".to_owned())),
        (
            "receipt".to_owned(),
            JsonValue::Object(JsonObject::from([
                (
                    "schema".to_owned(),
                    JsonValue::String("runx.receipt.v1".to_owned()),
                ),
                (
                    "disposition".to_owned(),
                    JsonValue::String("closed".to_owned()),
                ),
            ])),
        ),
        (
            "step_outputs".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "inspect".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "subset".to_owned(),
                    JsonValue::Object(JsonObject::from([(
                        "status".to_owned(),
                        JsonValue::String("ready".to_owned()),
                    )])),
                )])),
            )])),
        ),
    ]))?;

    assert_eq!(expectation.status, Some(HarnessExpectedStatus::Sealed));
    assert_eq!(
        expectation
            .receipt
            .as_ref()
            .and_then(|receipt| receipt.disposition.as_ref()),
        Some(&ClosureDisposition::Closed)
    );
    assert!(expectation.step_outputs.contains_key("inspect"));
    Ok(())
}

#[test]
fn rejects_retired_receipt_fields() {
    for field in [
        "kind",
        "skill_execution",
        "graph_execution",
        "skill_name",
        "source_type",
        "graph_name",
        "owner",
    ] {
        let error = parse_harness_fixture(&format!(
            "name: old\nkind: skill\ntarget: ..\nexpect:\n  receipt:\n    {field}: value\n"
        ))
        .expect_err("retired receipt field must fail");
        assert!(matches!(
            error,
            HarnessFixtureError::RetiredReceiptField { field_path }
                if field_path == format!("expect.receipt.{field}")
        ));
    }
}

#[test]
fn validates_package_relative_receipt_setup() -> Result<(), HarnessFixtureError> {
    let fixture = parse_harness_fixture(
        "name: receipt-proof\nkind: skill\ntarget: ..\nsetup:\n  receipts:\n    - fixtures/receipt-store/sha256-demo.json\n",
    )?;

    assert_eq!(
        fixture.setup.receipts,
        vec!["fixtures/receipt-store/sha256-demo.json"]
    );
    Ok(())
}

#[test]
fn rejects_receipt_setup_path_escape() {
    let error = parse_harness_fixture(
        "name: receipt-proof\nkind: skill\ntarget: ..\nsetup:\n  receipts:\n    - ../receipt.json\n",
    )
    .expect_err("receipt setup path escape must fail");

    assert!(
        matches!(error, HarnessFixtureError::Invalid { field, .. } if field == "setup.receipts[0]")
    );
}

#[test]
fn admits_bounded_exact_harness_http_responses() -> Result<(), HarnessFixtureError> {
    let fixture = parse_harness_fixture(
        r#"
name: deterministic-web
kind: skill
target: ..
caller:
  http_responses:
    "https://fixture.runx.invalid/source":
      status: 200
      headers: { content-type: text/plain }
      body: hello world
"#,
    )?;

    let responses = parse_harness_http_responses(
        fixture.caller.get("http_responses"),
        "caller.http_responses",
    )?;
    let response = responses
        .get("https://fixture.runx.invalid/source")
        .expect("validated response must remain addressable by exact URL");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "hello world");
    Ok(())
}

#[test]
fn admits_request_sensitive_harness_http_exchanges() -> Result<(), HarnessFixtureError> {
    let fixture = parse_harness_fixture(
        r#"
name: deterministic-mcp
kind: skill
target: ..
caller:
  http_exchanges:
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
"#,
    )?;

    let exchanges = parse_harness_http_exchanges(
        fixture.caller.get("http_exchanges"),
        "caller.http_exchanges",
    )?;
    assert_eq!(exchanges.len(), 3);
    assert_eq!(exchanges[0].request.method, "POST");
    assert_eq!(exchanges[0].request.url, "https://fixture.runx.invalid/mcp");
    assert_eq!(exchanges[0].response.status, 200);
    Ok(())
}

#[test]
fn harness_http_exchanges_reject_malformed_urls_and_bound_characters() {
    for malformed in [
        "https://",
        "https://?query",
        "ftp://fixture.runx.invalid/source",
        "https://fixture.runx.invalid/source\u{0007}",
        "https://user:pass@fixture.runx.invalid/source",
        "https://fixture.runx.invalid/source#fragment",
    ] {
        assert!(
            validate_harness_http_url(malformed, "caller.http_exchanges[0].request.url").is_err(),
            "malformed URL must fail: {malformed:?}",
        );
    }

    let prefix = "https://fixture.runx.invalid/";
    let at_limit = format!("{prefix}{}", "é".repeat(2048 - prefix.chars().count()));
    assert!(validate_harness_http_url(&at_limit, "caller.http_exchanges[0].request.url").is_ok());
    let over_limit = format!("{at_limit}é");
    assert!(
        validate_harness_http_url(&over_limit, "caller.http_exchanges[0].request.url").is_err()
    );
}

#[test]
fn harness_http_exchanges_and_legacy_responses_reject_credentials_and_fragments() {
    for url in [
        "https://user:pass@fixture.runx.invalid/source",
        "https://fixture.runx.invalid/source#fragment",
    ] {
        let fixture = format!(
            r#"
name: unreachable-url
kind: skill
target: ..
caller:
  http_responses:
    "{url}": {{ status: 200, body: unreachable }}
"#,
        );
        assert!(
            parse_harness_fixture(&fixture).is_err(),
            "legacy URL must fail: {url:?}",
        );
    }
}

#[test]
fn harness_http_exchanges_canonicalize_final_urls_and_reject_aliases() {
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
kind: skill
target: ..
caller:
  http_exchanges:
    - request: {{ method: POST, url: "{first}", body: none }}
      response: {{ status: 200, body: first }}
    - request: {{ method: POST, url: "{alias}", body: none }}
      response: {{ status: 200, body: alias }}
"#,
        );
        let error = parse_harness_fixture(&fixture)
            .expect_err("canonical final-URL aliases must be duplicate identities");
        assert!(
            matches!(error, HarnessFixtureError::Invalid { field, .. } if field == "caller.http_exchanges[1].request")
        );
    }
}

#[test]
fn harness_http_exchanges_enforce_count_and_body_bounds() {
    let empty = parse_harness_fixture(
        "name: empty-exchanges\nkind: skill\ntarget: ..\ncaller:\n  http_exchanges: []\n",
    )
    .expect_err("an explicitly empty exchange list must fail");
    assert!(
        matches!(empty, HarnessFixtureError::Invalid { field, .. } if field == "caller.http_exchanges")
    );

    let entries = (0..33)
        .map(|index| {
            format!(
                "    - request:\n        method: POST\n        url: https://fixture.runx.invalid/mcp\n        body: {{ json: {{ index: {index} }} }}\n      response: {{ status: 200, body: ok }}\n"
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let too_many = format!(
        "name: too-many-exchanges\nkind: skill\ntarget: ..\ncaller:\n  http_exchanges:\n{entries}"
    );
    assert!(
        parse_harness_fixture(&too_many).is_err(),
        "33 exchanges must exceed the declared bound",
    );

    let oversized = "x".repeat(1_048_576);
    let oversized_body = format!(
        r#"
name: oversized-exchange-body
kind: skill
target: ..
caller:
  http_exchanges:
    - request:
        method: POST
        url: https://fixture.runx.invalid/mcp
        body: {{ json: "{oversized}" }}
      response: {{ status: 200, body: ok }}
"#,
    );
    assert!(
        parse_harness_fixture(&oversized_body).is_err(),
        "serialized JSON over 1 MiB must fail",
    );
}

#[test]
fn harness_http_exchanges_reject_unknown_body_fields() {
    let error = parse_harness_fixture(
        r#"
name: widened-mcp-body
kind: skill
target: ..
caller:
  http_exchanges:
    - request:
        method: POST
        url: https://fixture.runx.invalid/mcp
        body: { json: { operation: status }, extra: ignored }
      response: { status: 200, body: ok }
"#,
    )
    .expect_err("unknown body fields must fail instead of changing identity silently");

    assert!(matches!(error, HarnessFixtureError::Invalid { .. }));
}

#[test]
fn harness_http_exchanges_accept_json_bodies_for_get_and_delete() -> Result<(), HarnessFixtureError>
{
    let fixture = parse_harness_fixture(
        r#"
name: method-agnostic-body-identity
kind: skill
target: ..
caller:
  http_exchanges:
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
"#,
    )?;
    let exchanges = parse_harness_http_exchanges(
        fixture.caller.get("http_exchanges"),
        "caller.http_exchanges",
    )?;
    assert_eq!(exchanges.len(), 2);
    assert_eq!(exchanges[0].request.method, "GET");
    assert_eq!(exchanges[1].request.method, "DELETE");
    Ok(())
}

#[test]
fn rejects_duplicate_harness_http_exchanges() {
    let error = parse_harness_fixture(
        r#"
name: duplicate-mcp
kind: skill
target: ..
caller:
  http_exchanges:
    - request: &request
        method: POST
        url: https://fixture.runx.invalid/mcp
        body:
          json: { operation: status }
      response: { status: 200, body: first }
    - request: *request
      response: { status: 200, body: second }
"#,
    )
    .expect_err("duplicate exact exchanges must fail");

    assert!(
        matches!(error, HarnessFixtureError::Invalid { field, .. } if field == "caller.http_exchanges[1].request")
    );
}

#[test]
fn rejects_non_http_harness_response_keys() {
    let error = parse_harness_fixture(
        r#"
name: deterministic-web
kind: skill
target: ..
caller:
  http_responses:
    "file:///private/source":
      status: 200
      body: hidden
"#,
    )
    .expect_err("non-HTTP fixture response must fail");

    assert!(
        matches!(error, HarnessFixtureError::Invalid { field, .. } if field.contains("file:///private/source"))
    );
}

#[test]
fn rejects_empty_declared_harness_response_map() {
    let error = parse_harness_fixture(
        "name: deterministic-web\nkind: skill\ntarget: ..\ncaller:\n  http_responses: {}\n",
    )
    .expect_err("declared response map must fail closed when empty");

    assert!(
        matches!(error, HarnessFixtureError::Invalid { field, .. } if field == "caller.http_responses")
    );
}

#[test]
fn rejects_retired_web_response_field_without_network_fallback() {
    let error = parse_harness_fixture(
        r#"
name: deterministic-web
kind: skill
target: ..
caller:
  web_responses:
    "https://fixture.runx.invalid/source":
      status: 200
      body: hello world
"#,
    )
    .expect_err("retired response field must fail instead of reaching the network");

    assert!(
        matches!(error, HarnessFixtureError::Invalid { field, .. } if field == "caller.web_responses")
    );
}
