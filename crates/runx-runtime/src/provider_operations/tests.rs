use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use super::*;

fn slack_binding() -> ConnectorBinding {
    crate::kernel::connectors::fixture_scope()
        .connector("fixture/chat")
        .expect("fixture/chat is official")
        .binding()
}
use crate::HostedApiEnvironment;
use crate::http::{
    RuntimeHttpError, RuntimeHttpRequest as HttpRequest, RuntimeHttpResponse as HttpResponse,
};

#[derive(Default)]
struct StubTransport {
    requests: RefCell<Vec<HttpRequest>>,
    responses: RefCell<Vec<HttpResponse>>,
    /// The URLs of the requests sent as repeatable.
    repeatable: RefCell<Vec<String>>,
}

impl StubTransport {
    fn with_responses(responses: Vec<HttpResponse>) -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            responses: RefCell::new(responses.into_iter().rev().collect()),
            repeatable: RefCell::new(Vec::new()),
        }
    }
}

impl Transport for StubTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, RuntimeHttpError> {
        self.requests.borrow_mut().push(request);
        self.responses
            .borrow_mut()
            .pop()
            .ok_or_else(|| RuntimeHttpError::Transport {
                message: "missing stub response".to_owned(),
            })
    }

    fn send_idempotent(&self, request: HttpRequest) -> Result<HttpResponse, RuntimeHttpError> {
        self.repeatable.borrow_mut().push(request.url.clone());
        self.send(request)
    }
}

#[test]
fn a_provider_read_is_sent_once_never_through_the_repeating_lane() {
    // Its cost is not declared, so Connect is never asked to run it twice.
    let env = BTreeMap::from([("RUNX_PUBLIC_API_TOKEN".to_owned(), "rxk_test".to_owned())]);
    let transport = StubTransport::with_responses(vec![
        HttpResponse::new(
            200,
            serde_json::json!({
                "status": "success",
                "principal": {"principal_id": "operator:test"}
            })
            .to_string(),
        ),
        HttpResponse::new(
            200,
            serde_json::json!({
                "status": "success",
                "provider": "x402",
                "operation": "payment.x402.read",
                "target": "https://vendor.example/v1/invocations",
                "access": "read",
                "readback_ref": "runx:provider-readback:payment-1",
                "result": {"status": "settled"}
            })
            .to_string(),
        ),
    ]);
    let environment =
        HostedApiEnvironment::resolve(Some("https://api.runx.test"), &env, Path::new("."))
            .expect("environment")
            .authenticate(&transport)
            .expect("authenticated");

    invoke_provider_operation(
        &transport,
        &environment,
        &ProviderOperationRequest {
            connector_binding: slack_binding(),
            grant_id: "grant_x402_1".to_owned(),
            operation: "payment.x402.read".to_owned(),
            operation_digest: format!("sha256:{}", "a".repeat(64)),
            target: "https://vendor.example/v1/invocations".to_owned(),
            scopes: vec!["payment.x402.read".to_owned()],
            input: JsonObject::new(),
            access: OperationAccess::Read,
        },
    )
    .expect("provider read");

    // Only reading the principal may repeat.
    assert_eq!(
        *transport.repeatable.borrow(),
        ["https://api.runx.test/v1/me"]
    );
}

#[test]
fn provider_operation_authenticates_and_returns_bounded_readback() {
    let env = BTreeMap::from([("RUNX_PUBLIC_API_TOKEN".to_owned(), "rxk_test".to_owned())]);
    let transport = StubTransport::with_responses(vec![
        HttpResponse::new(
            200,
            serde_json::json!({
                "status": "success",
                "principal": {"principal_id": "operator:test"}
            })
            .to_string(),
        ),
        HttpResponse::new(
            200,
            serde_json::json!({
                "status": "success",
                "provider": "fixture/chat",
                "operation": "thread.reply",
                "target": "chat://T/C/2",
                "access": "mutate",
                "operation_id": "provider-op-1",
                "idempotency_key": "runx:test-operation",
                "readback_ref": "runx:provider-readback:provider-op-1",
                "result": {"message_locator": "chat://T/C/2"}
            })
            .to_string(),
        ),
    ]);
    let environment =
        HostedApiEnvironment::resolve(Some("https://api.runx.test"), &env, Path::new("."))
            .expect("environment")
            .authenticate(&transport)
            .expect("authenticated");
    let response = invoke_provider_operation(
        &transport,
        &environment,
        &ProviderOperationRequest {
            connector_binding: slack_binding(),
            grant_id: "grant_slack_1".to_owned(),
            operation: "thread.reply".to_owned(),
            operation_digest: format!("sha256:{}", "a".repeat(64)),
            target: "chat://T/C/2".to_owned(),
            scopes: vec!["thread.reply".to_owned()],
            input: JsonObject::from([(
                "idempotency_key".to_owned(),
                JsonValue::String("runx:test-operation".to_owned()),
            )]),
            access: OperationAccess::Mutate,
        },
    )
    .expect("provider operation");

    assert_eq!(response.provider, "fixture/chat");
    assert_eq!(transport.requests.borrow().len(), 2);
    assert_eq!(
        *transport.repeatable.borrow(),
        ["https://api.runx.test/v1/me"]
    );
    let request_body: JsonValue = serde_json::from_str(
        transport.requests.borrow()[1]
            .body
            .as_deref()
            .expect("provider operation body"),
    )
    .expect("provider operation JSON");
    let request_body = request_body
        .as_object()
        .expect("provider operation body must be an object");
    assert_eq!(
        request_body.get("scopes"),
        Some(&JsonValue::Array(vec![JsonValue::String(
            "thread.reply".to_owned(),
        )]))
    );
}

#[test]
fn provider_grant_listing_returns_only_bounded_authority_metadata() {
    let env = BTreeMap::from([("RUNX_PUBLIC_API_TOKEN".to_owned(), "rxk_test".to_owned())]);
    let transport = StubTransport::with_responses(vec![
        HttpResponse::new(
            200,
            serde_json::json!({
                "status": "success",
                "principal": {"principal_id": "operator:test"}
            })
            .to_string(),
        ),
        HttpResponse::new(
            200,
            serde_json::json!({
                "status": "success",
                "grants": [{
                    "grant_id": "grant_slack_1",
                    "provider": "fixture/chat",
                    "connector_binding": slack_binding(),
                    "scopes": [
                        "channel.post",
                        "https://provider.example/auth/custom.scope?mode=read,write",
                        "opaque capability with spaces"
                    ],
                    "status": "active",
                    "credential_material_bound": true
                }, {
                    "grant_id": "grant_retired_1",
                    "provider": "retired",
                    "scopes": ["records.read"],
                    "status": "active"
                }]
            })
            .to_string(),
        ),
    ]);
    let environment =
        HostedApiEnvironment::resolve(Some("https://api.runx.test"), &env, Path::new("."))
            .expect("environment")
            .authenticate(&transport)
            .expect("authenticated");

    let grants = list_provider_grants(&transport, &environment).expect("grants");

    assert_eq!(
        grants,
        vec![
            HostedProviderGrant {
                connector_binding: Some(slack_binding()),
                grant_id: "grant_slack_1".to_owned(),
                provider: "fixture/chat".to_owned(),
                scopes: vec![
                    "channel.post".to_owned(),
                    "https://provider.example/auth/custom.scope?mode=read,write".to_owned(),
                    "opaque capability with spaces".to_owned(),
                ],
                status: "active".to_owned(),
                credential_material_bound: true,
                target_locator: None,
            },
            // A grant Connect executes under no connector still lists; only
            // confirming or using it is refused.
            HostedProviderGrant {
                connector_binding: None,
                grant_id: "grant_retired_1".to_owned(),
                provider: "retired".to_owned(),
                scopes: vec!["records.read".to_owned()],
                status: "active".to_owned(),
                credential_material_bound: false,
                target_locator: None,
            },
        ]
    );
    let requests = transport.requests.borrow();
    assert_eq!(requests[1].method, HttpMethod::Get);
    assert_eq!(requests[1].url, "https://api.runx.test/v1/grants");
    assert!(requests[1].body.is_none());
}

#[test]
fn provider_operation_rejects_mismatched_access_before_trusting_result() {
    let request = ProviderOperationRequest {
        connector_binding: slack_binding(),
        grant_id: "grant_slack_1".to_owned(),
        operation: "thread.reply".to_owned(),
        operation_digest: format!("sha256:{}", "a".repeat(64)),
        target: "chat://T/C/2".to_owned(),
        scopes: vec!["thread.reply".to_owned()],
        input: JsonObject::new(),
        access: OperationAccess::Read,
    };
    let error = decode_readback(
        response_object(serde_json::json!({
            "status": "success",
            "provider": "fixture/chat",
            "operation": "thread.reply",
            "target": "chat://T/C/2",
            "access": "mutate",
            "readback_ref": "runx:provider-readback:1",
            "result": {"message_locator": "chat://T/C/2"}
        })),
        request.access,
    )
    .expect_err("access mismatch");
    assert!(error.to_string().contains("response access"));
}

#[test]
fn provider_operation_requires_explicit_success_status() {
    let request = ProviderOperationRequest {
        connector_binding: slack_binding(),
        grant_id: "grant_slack_1".to_owned(),
        operation: "thread.read".to_owned(),
        operation_digest: format!("sha256:{}", "a".repeat(64)),
        target: "chat://T/C/2".to_owned(),
        scopes: vec!["thread.read".to_owned()],
        input: JsonObject::new(),
        access: OperationAccess::Read,
    };
    let error = decode_readback(
        response_object(serde_json::json!({
            "provider": "fixture/chat",
            "operation": "thread.read",
            "target": "chat://T/C/2",
            "access": "read",
            "result": {"messages": []}
        })),
        request.access,
    )
    .expect_err("a missing status must fail closed");

    assert!(error.to_string().contains("status"));
}

#[test]
fn provider_operation_requires_readback_evidence() {
    let request = ProviderOperationRequest {
        connector_binding: slack_binding(),
        grant_id: "grant_slack_1".to_owned(),
        operation: "thread.read".to_owned(),
        operation_digest: format!("sha256:{}", "a".repeat(64)),
        target: "chat://T/C/2".to_owned(),
        scopes: vec!["thread.read".to_owned()],
        input: JsonObject::new(),
        access: OperationAccess::Read,
    };
    let error = decode_readback(
        response_object(serde_json::json!({
            "status": "success",
            "provider": "fixture/chat",
            "operation": "thread.read",
            "target": "chat://T/C/2",
            "access": "read",
            "result": {"messages": []}
        })),
        request.access,
    )
    .expect_err("provider reads require readback evidence");

    assert!(error.to_string().contains("readback_ref"));
}

fn response_object(value: serde_json::Value) -> JsonObject {
    serde_json::from_value::<JsonValue>(value)
        .expect("provider response fixture must convert")
        .as_object()
        .expect("provider response fixture must be an object")
        .clone()
}

#[test]
fn operation_grammar_admits_underscored_segments() {
    for operation in ["reports.run_realtime", "sync.write_batch", "thread.reply"] {
        assert!(
            super::validate_provider_operation(operation).is_ok(),
            "{operation}"
        );
    }
    for operation in ["thread", "Thread.reply", "_sync.read", "a..b"] {
        assert!(
            super::validate_provider_operation(operation).is_err(),
            "{operation}"
        );
    }
}
