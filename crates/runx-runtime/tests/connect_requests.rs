#![allow(clippy::expect_used, clippy::unwrap_used)]

//! A hosted declared-HTTP operation, end to end through the test's runx
//! Connect: the kernel admits the call under the case's grant and sends it
//! to Connect by request id, and Connect renders the declared request with
//! the purchase it holds and answers it from the case's exchanges.
//! Without a purchase the request fails closed before anything is
//! sent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_runtime::{TestExpectedStatus, TestReplayError, TestReplayOutput, run_test_fixture};

const CONNECTOR: &str = r#"schema: runx.connector.v1
name: sales
title: Acme Sales
description: Record one sale for the purchase a run fulfills.
category: agent-tools
auth:
  credentials:
    service_token:
      kind: api_key
      fields:
        - {name: token, label: Acme service token}
      delivery: {env: ACME_SERVICE_TOKEN}
      audience: [api.example.com]
placements:
  hosted: {auth: credential, credential: service_token}
operations:
  sale.create:
    description: Record one sale for the purchase this run fulfills.
    verb: write
    scopes: [sale.create]
    credentials: [service_token]
    target:
      template: "acme:sale"
    guarantees:
      provider_idempotency: inherent
      readback: unavailable
    input_schema:
      type: object
      required: [amount]
      additionalProperties: false
      properties:
        amount: {type: integer}
    result_fields: [sale]
    implementation:
      http:
        allowed_hosts: [api.example.com]
        auth: {service_token: {type: bearer, field: token}}
        requests:
          - http_method: POST
            url: https://api.example.com/v1/sales
            body:
              amount: {"$argument": "/amount"}
              purchase: {"$runx": "purchase"}
"#;

const SKILL: &str = r#"
minimum_runx: 0.12.0
methods:
  sell:
    inputs:
      amount: {type: integer}
    steps:
      - id: sell
        operation: sales/sale.create
        result:
          required: [sale]
        idempotency_key: $run.id
        inputs: {amount: $input.amount}
"#;

/// The provider's one exchange: the sale as Connect renders it with the
/// purchase, and the sale it records.
const EXCHANGE: &str = r#"  http:
    - request:
        method: POST
        url: https://api.example.com/v1/sales
        body: {json: {amount: 7, purchase: purchase_sale_1}}
      response:
        status: 201
        headers: {content-type: application/json}
        body: '{"sale_id": "sale_1"}'
"#;

/// Run one case of the sales skill, whose Connect holds `purchase`.
fn run(
    root: &Path,
    name: &str,
    purchase: Option<&str>,
    status: &str,
) -> Result<TestReplayOutput, TestReplayError> {
    // Connect sends the request only with a purchase to send.
    let (purchase, exchanges) = match purchase {
        Some(purchase) => (format!("    purchase_id: {purchase}\n"), EXCHANGE),
        None => (String::new(), ""),
    };
    run_case(
        root,
        name,
        CONNECTOR,
        &format!("{purchase}{exchanges}"),
        status,
    )
}

/// Run one case of the sales skill over `connector`, whose Connect holds
/// the case's grant and answers with `providers`.
fn run_case(
    root: &Path,
    name: &str,
    connector: &str,
    providers: &str,
    status: &str,
) -> Result<TestReplayOutput, TestReplayError> {
    std::fs::write(root.join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
    std::fs::create_dir_all(root.join("connectors/sales")).unwrap();
    std::fs::write(root.join("connectors/sales/connector.yaml"), connector).unwrap();
    let skill = root.join("sales");
    std::fs::create_dir_all(skill.join("tests")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: sales\ndescription: sales fixture.\n---\n# Sales\n",
    )
    .unwrap();
    std::fs::write(skill.join("run.yaml"), SKILL).unwrap();
    let path: PathBuf = skill.join("tests").join(format!("{name}.yaml"));
    std::fs::write(
        &path,
        format!(
            r#"name: {name}
method: sell
inputs: {{amount: 7}}
providers:
  hosted:
    principal_id: operator-fixture
    grant: {{grant_id: sales-fixture, provider: sales, scopes: [sale.create]}}
{providers}expect:
  status: {status}
"#
        ),
    )
    .unwrap();
    let env = std::env::var("PATH")
        .ok()
        .map(|path| BTreeMap::from([("PATH".to_owned(), path)]))
        .unwrap_or_default();
    run_test_fixture(path, env)
}

#[test]
fn a_hosted_declared_write_sends_the_purchase_connect_leases() {
    let root = tempfile::tempdir().unwrap();
    let output =
        run(root.path(), "sells", Some("purchase_sale_1"), "sealed").expect("the sale seals");

    assert_eq!(output.status, TestExpectedStatus::Sealed);
    assert!(
        output.unrequested.is_empty(),
        "Connect sent exactly the declared request: {:?}",
        output.unrequested
    );
    let sell = output
        .steps
        .iter()
        .find(|step| step.step_id == "sell")
        .expect("the sale ran");
    let contract = serde_json::to_value(&sell.contract).unwrap();
    let operation = &contract["provider_operation"]["data"];
    assert_eq!(operation["transport"], "runx_connect", "{contract:#}");
    assert_eq!(
        operation["result"]["sale"]["sale_id"], "sale_1",
        "{contract:#}"
    );
}

#[test]
fn a_declared_write_that_sends_the_purchase_fails_closed_without_one() {
    let root = tempfile::tempdir().unwrap();
    let output = run(root.path(), "unbound", None, "failure").expect("the sale fails closed");

    assert_eq!(output.status, TestExpectedStatus::Failure);
    // Connect refused it before sending: the case answers no request, so
    // one that left would have failed as unanswered instead.
    let failure = format!("{:?}", output.skill_output);
    assert!(
        failure.contains("connect_refused")
            && failure.contains(r#"template binding $runx \"purchase\" is unavailable"#),
        "{failure}"
    );
}

/// The sale as Connect records it, read back by its id.
const READ_BACK_CONNECTOR: &str = r#"schema: runx.connector.v1
name: sales
title: Acme Sales
description: Record one sale, and read it back.
category: agent-tools
auth:
  credentials:
    service_token:
      kind: api_key
      fields:
        - {name: token, label: Acme service token}
      delivery: {env: ACME_SERVICE_TOKEN}
      audience: [api.example.com]
placements:
  hosted: {auth: credential, credential: service_token}
operations:
  sale.create:
    description: Record one sale.
    verb: write
    scopes: [sale.create]
    credentials: [service_token]
    target:
      template: "acme:sale"
    guarantees:
      provider_idempotency: inherent
    input_schema:
      type: object
      required: [amount]
      additionalProperties: false
      properties:
        amount: {type: integer}
    result_fields: [sale]
    implementation:
      http:
        allowed_hosts: [api.example.com]
        auth: {service_token: {type: bearer, field: token}}
        requests:
          - http_method: POST
            url: https://api.example.com/v1/sales
            body:
              amount: {"$argument": "/amount"}
        readbacks:
          - request:
              id: read
              http_method: GET
              url: https://api.example.com/v1/sales/sale_1
            matches:
              - expected: {source: acknowledgement, path: sale_id}
                observed: sale_id
"#;

/// The provider records sale_1, and reading it back finds sale_2: an answer
/// that contradicts the call fails its step with the failure's own record,
/// as the call returns it, which the run returns.
#[test]
fn a_write_whose_readback_contradicts_it_fails_its_step_with_the_record() {
    let root = tempfile::tempdir().unwrap();
    let output = run_case(
        root.path(),
        "contradicted",
        READ_BACK_CONNECTOR,
        r#"  http:
    - request:
        method: POST
        url: https://api.example.com/v1/sales
        body: {json: {amount: 7}}
      response:
        status: 201
        headers: {content-type: application/json}
        body: '{"sale_id": "sale_1"}'
    - request: {method: GET, url: "https://api.example.com/v1/sales/sale_1", body: none}
      response:
        status: 200
        headers: {content-type: application/json}
        body: '{"sale_id": "sale_2"}'
"#,
        "failure",
    )
    .expect("the contradicted sale fails");

    assert_eq!(output.status, TestExpectedStatus::Failure);
    let sell = output
        .steps
        .iter()
        .find(|step| step.step_id == "sell")
        .expect("the sale ran");
    let record =
        serde_json::to_value(sell.failure.as_ref().expect("the step keeps its record")).unwrap();
    assert_eq!(record["code"], "provider_contradicted", "{record:#}");
    assert_eq!(record["retryable"], false, "{record:#}");
    assert_eq!(
        record["mismatches"],
        serde_json::json!([{"field": "sale_id", "asked": "sale_1", "held": "sale_2"}]),
        "{record:#}"
    );
}
