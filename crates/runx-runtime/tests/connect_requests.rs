#![allow(clippy::expect_used, clippy::unwrap_used)]

//! A hosted declared-HTTP operation, end to end through the harness's runx
//! Connect: the kernel admits the call under the case's grant and sends it
//! to Connect by request id, and Connect renders the declared request with
//! the paid invocation it holds and answers it from the case's exchanges.
//! Without a paid invocation the request fails closed before anything is
//! sent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_runtime::{
    HarnessExpectedStatus, HarnessReplayError, HarnessReplayOutput, run_harness_fixture,
};

const CONNECTOR: &str = r#"schema: runx.connector.v1
id: sales
name: Acme Sales
description: Record one sale for the paid invocation a run fulfills.
category: agent-tools
runtime_capability: false
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
    description: Record one sale for the paid invocation this run fulfills.
    verb: create
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
    hosted: {}
    implementation:
      http:
        allowed_hosts: [api.example.com]
        auth: {service_token: {type: bearer, field: token}}
        requests:
          - method: POST
            url: https://api.example.com/v1/sales
            body:
              amount: {"$argument": "/amount"}
              invocation: {"$runx": "invocation"}
"#;

const SKILL: &str = r#"
skill: sales
connectors:
  sales:
    operations: [sale.create]
runners:
  sell:
    default: true
    inputs:
      amount: {type: integer, required: true}
    steps:
      - id: sell
        operation: sales/sale.create
        result_fields: [sale]
        idempotency_key: $run.id
        input: {amount: $input.amount}
"#;

/// The provider's one exchange: the sale as Connect renders it with the
/// paid invocation, and the sale it records.
const EXCHANGE: &str = r#"  http_exchanges:
    - request:
        method: POST
        url: https://api.example.com/v1/sales
        body: {json: {amount: 7, invocation: pinv_sale_1}}
      response:
        status: 201
        headers: {content-type: application/json}
        body: '{"sale_id": "sale_1"}'
"#;

/// Run one case of the sales skill, whose Connect holds `invocation`.
fn run(
    root: &Path,
    name: &str,
    invocation: Option<&str>,
    status: &str,
) -> Result<HarnessReplayOutput, HarnessReplayError> {
    std::fs::write(root.join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
    std::fs::create_dir_all(root.join("connectors/sales")).unwrap();
    std::fs::write(root.join("connectors/sales/connector.yaml"), CONNECTOR).unwrap();
    let skill = root.join("sales");
    std::fs::create_dir_all(skill.join("fixtures")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: sales\ndescription: sales fixture.\n---\n# Sales\n",
    )
    .unwrap();
    std::fs::write(skill.join("run.yaml"), SKILL).unwrap();
    // Connect sends the request only with a paid invocation to send.
    let (invocation, exchanges) = match invocation {
        Some(invocation) => (format!("    invocation_id: {invocation}\n"), EXCHANGE),
        None => (String::new(), ""),
    };
    let path: PathBuf = skill.join("fixtures").join(format!("{name}.yaml"));
    std::fs::write(
        &path,
        format!(
            r#"name: {name}
kind: skill
target: ..
runner: sell
inputs: {{amount: 7}}
caller:
  provider_responses:
    principal_id: operator-fixture
    grant: {{grant_id: sales-fixture, provider: sales, scopes: [sale.create]}}
{invocation}{exchanges}expect:
  status: {status}
"#
        ),
    )
    .unwrap();
    let env = std::env::var("PATH")
        .ok()
        .map(|path| BTreeMap::from([("PATH".to_owned(), path)]))
        .unwrap_or_default();
    run_harness_fixture(path, env)
}

#[test]
fn a_hosted_declared_write_sends_the_paid_invocation_connect_holds() {
    let root = tempfile::tempdir().unwrap();
    let output = run(root.path(), "sells", Some("pinv_sale_1"), "sealed").expect("the sale seals");

    assert_eq!(output.status, HarnessExpectedStatus::Sealed);
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
fn a_declared_write_that_sends_the_paid_invocation_fails_closed_without_one() {
    let root = tempfile::tempdir().unwrap();
    let output = run(root.path(), "unbound", None, "failure").expect("the sale fails closed");

    assert_eq!(output.status, HarnessExpectedStatus::Failure);
    // Connect refused it before sending: the case answers no request, so
    // one that left would have failed as unanswered instead.
    let failure = format!("{:?}", output.skill_output);
    assert!(
        failure.contains("connect_refused")
            && failure.contains(r#"template binding $runx \"invocation\" is unavailable"#),
        "{failure}"
    );
}
