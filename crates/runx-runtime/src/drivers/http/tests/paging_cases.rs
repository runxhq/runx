#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use runx_contracts::connector::{HttpImplementation, OperationAccess};
use runx_contracts::{JsonObject, JsonValue};

use crate::drivers::http::{Egress, Http, MAX_PAGES};
use crate::http::{
    HttpMethod, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Driver, Observed, Unperformed, at_path};

use super::response;

fn spec() -> Result<HttpImplementation, String> {
    serde_json::from_value(serde_json::json!({
        "allowed_hosts": ["api.example.test"],
        "requests": [{
            "id": "query", "each": {"$argument": "/queries"},
            "http_method": "POST", "url": "https://api.example.test/query",
            "body": {"$item": ""},
            "pages": {"items": "data", "next": {"cursor": "next", "param": "/page_token", "in": "body"}}
        }],
        "decode": [{"op": "auto_json"}],
        "result_paths": {"rows": "acknowledgement"}
    }))
    .map_err(|error| error.to_string())
}

fn run(
    transport: &(dyn RuntimeHttpTransport + Send + Sync),
    spec: &HttpImplementation,
) -> Result<Observed, Unperformed> {
    let arguments = serde_json::from_value::<JsonValue>(
        serde_json::json!({"queries": [{"name": "first"}, {"name": "second"}]}),
    )
    .expect("test arguments");
    let (parameters, env) = (JsonObject::new(), BTreeMap::new());
    let call = AdmittedCall::for_tests(&arguments, &parameters, &env, OperationAccess::Read);
    Http::new(Egress::Direct(transport), &call, spec).perform(None)
}

#[derive(Default)]
struct BodyPages {
    requests: Mutex<Vec<serde_json::Value>>,
}

impl RuntimeHttpTransport for BodyPages {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "https://api.example.test/query");
        let body: serde_json::Value =
            serde_json::from_str(request.body.as_deref().expect("POST body")).expect("JSON body");
        self.requests
            .lock()
            .expect("requests lock")
            .push(body.clone());
        let query = body.pointer("/queries/0").unwrap_or(&body);
        Ok(match (query["name"].as_str(), query.get("page_token")) {
            (Some("first"), None) => response(200, r#"{"data":[1],"next":2}"#),
            (Some("first"), Some(token)) if token == &serde_json::json!(2) => {
                response(200, r#"{"data":[2]}"#)
            }
            (Some("second"), None) => response(200, r#"{"data":[3],"next":2}"#),
            (Some("second"), Some(token)) if token == &serde_json::json!(2) => {
                response(200, r#"{"data":[4],"next":"two"}"#)
            }
            (Some("second"), Some(token)) if token == "two" => response(200, r#"{"data":[5]}"#),
            _ => response(500, r#"{"error":"unexpected test cursor"}"#),
        })
    }
}

#[test]
fn post_cursors_keep_their_type_and_each_item_has_its_own_cursor() -> Result<(), String> {
    let transport = BodyPages::default();
    let read = run(&transport, &spec()?).map_err(Unperformed::reason)?;
    let expected = serde_json::from_value::<JsonValue>(serde_json::json!([
        {"items":[1,2],"next":null,"started":true,"complete":true,"error":null},
        {"items":[3,4,5],"next":null,"started":true,"complete":true,"error":null}
    ]))
    .map_err(|error| error.to_string())?;
    assert_eq!(read.value, expected);
    assert_eq!(read.evidence.len(), 5);
    let sent = transport.requests.lock().expect("requests");
    assert_eq!(
        sent[1]["page_token"],
        serde_json::json!(2),
        "a numeric body cursor must not become a string"
    );
    assert!(
        sent[2].get("page_token").is_none(),
        "the second item must start independently"
    );
    assert_eq!(
        sent[3]["page_token"],
        serde_json::json!(2),
        "cycle state must be independent for each item"
    );
    assert_eq!(sent[4]["page_token"], "two");
    Ok(())
}

#[test]
fn a_body_cursor_can_address_an_object_inside_an_existing_array() -> Result<(), String> {
    let mut descriptor = serde_json::to_value(spec()?).map_err(|error| error.to_string())?;
    descriptor["requests"][0]["body"] = serde_json::json!({"queries":[{"$item":""}]});
    descriptor["requests"][0]["pages"]["next"]["param"] =
        serde_json::json!("/queries/0/page_token");
    let nested: HttpImplementation =
        serde_json::from_value(descriptor).map_err(|error| error.to_string())?;
    let transport = BodyPages::default();
    let read = run(&transport, &nested).map_err(Unperformed::reason)?;
    assert_eq!(
        at_path(&read.value, "1.complete"),
        Some(&JsonValue::Bool(true))
    );
    let sent = transport.requests.lock().expect("requests");
    assert_eq!(
        sent[1].pointer("/queries/0/page_token"),
        Some(&serde_json::json!(2))
    );
    assert_eq!(
        sent[4].pointer("/queries/0/page_token"),
        Some(&serde_json::json!("two"))
    );
    Ok(())
}

struct Endless {
    calls: AtomicUsize,
}
impl RuntimeHttpTransport for Endless {
    fn send(&self, _request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        let next = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(response(
            200,
            &format!(r#"{{"data":[{next}],"next":{next}}}"#),
        ))
    }
}

#[test]
fn repeated_items_share_the_request_budget_and_untouched_work_is_not_empty_success()
-> Result<(), String> {
    let transport = Endless {
        calls: AtomicUsize::new(0),
    };
    let read = run(&transport, &spec()?).map_err(Unperformed::reason)?;
    assert_eq!(transport.calls.load(Ordering::SeqCst), MAX_PAGES);
    assert_eq!(
        at_path(&read.value, "0.items")
            .and_then(JsonValue::as_array)
            .map(Vec::len),
        Some(MAX_PAGES)
    );
    assert_eq!(
        at_path(&read.value, "0.complete"),
        Some(&JsonValue::Bool(false))
    );
    assert_eq!(
        at_path(&read.value, "1.started"),
        Some(&JsonValue::Bool(false))
    );
    assert_eq!(
        at_path(&read.value, "1.complete"),
        Some(&JsonValue::Bool(false))
    );
    assert_eq!(
        at_path(&read.value, "1.items"),
        Some(&JsonValue::Array(Vec::new()))
    );
    Ok(())
}

struct CursorAnswer {
    answer: &'static str,
    calls: AtomicUsize,
}
impl RuntimeHttpTransport for CursorAnswer {
    fn send(&self, _request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(response(200, self.answer))
    }
}

#[test]
fn malformed_and_repeated_cursors_fail_their_item_without_echoing_opaque_tokens()
-> Result<(), String> {
    for (answer, calls_per_item, reason) in [
        (
            r#"{"data":[1],"next":{"unexpected":"object"}}"#,
            1,
            "not a scalar",
        ),
        (
            r#"{"data":[1],"next":"opaque-do-not-expose"}"#,
            2,
            "repeated a page cursor",
        ),
    ] {
        let transport = CursorAnswer {
            answer,
            calls: AtomicUsize::new(0),
        };
        let read = run(&transport, &spec()?).map_err(Unperformed::reason)?;
        // Each item fails on its own: the second is still read.
        for index in 0..2 {
            let message = at_path(&read.value, &format!("{index}.error.message"))
                .and_then(JsonValue::as_str)
                .ok_or("a malformed cursor fails its item")?;
            assert!(message.contains(reason), "{message}");
            assert!(!message.contains("opaque-do-not-expose"));
            assert_eq!(
                at_path(&read.value, &format!("{index}.complete")),
                Some(&JsonValue::Bool(false))
            );
        }
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2 * calls_per_item);
        // A read that isn't repeated has no item to settle, so it fails whole.
        let single = CursorAnswer {
            answer,
            calls: AtomicUsize::new(0),
        };
        let failed = match unrepeated(&single) {
            Err(error) => error,
            Ok(_) => return Err("invalid provider cursor was accepted".to_owned()),
        };
        let message = failed.reason();
        assert!(message.contains(reason), "{message}");
        assert!(!message.contains("opaque-do-not-expose"));
        assert_eq!(single.calls.load(Ordering::SeqCst), calls_per_item);
    }
    Ok(())
}

/// The same paged read, sent once rather than per item.
fn unrepeated(
    transport: &(dyn RuntimeHttpTransport + Send + Sync),
) -> Result<Observed, Unperformed> {
    let spec: HttpImplementation = serde_json::from_value(serde_json::json!({
        "allowed_hosts": ["api.example.test"],
        "requests": [{
            "id": "query", "http_method": "POST", "url": "https://api.example.test/query",
            "body": {"name": "first"},
            "pages": {"items": "data", "next": {"cursor": "next", "param": "/page_token", "in": "body"}}
        }],
        "decode": [{"op": "auto_json"}],
        "result_paths": {"rows": "acknowledgement"}
    }))
    .map_err(|error| Unperformed::NotSent {
        code: "invalid_request",
        reason: error.to_string(),
    })?;
    run(transport, &spec)
}

/// Each query by name: `first` answers two pages, `second` fails as told on
/// its first page or after it, `third` answers one page.
struct Settling {
    second: &'static str,
    status: u16,
    reset: Option<&'static str>,
    requests: Mutex<Vec<String>>,
}

impl RuntimeHttpTransport for Settling {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        let body: serde_json::Value =
            serde_json::from_str(request.body.as_deref().expect("POST body")).expect("JSON body");
        let name = body["name"].as_str().unwrap_or_default().to_owned();
        let token = body.get("page_token").cloned();
        self.requests.lock().expect("requests lock").push(format!(
            "{name}:{}",
            token.as_ref().map(ToString::to_string).unwrap_or_default()
        ));
        let failed = || {
            let mut failed = response(self.status, r#"{"message":"unavailable"}"#);
            if let Some(reset) = self.reset {
                failed.headers.push(crate::http::RuntimeHttpHeader::new(
                    "x-ratelimit-reset",
                    reset,
                ));
            }
            failed
        };
        Ok(match (name.as_str(), token) {
            ("first", None) => response(200, r#"{"data":[1],"next":2}"#),
            ("first", Some(_)) => response(200, r#"{"data":[2]}"#),
            ("second", None) if self.second == "first page" => failed(),
            ("second", None) => response(200, r#"{"data":[3],"next":2}"#),
            ("second", Some(_)) => failed(),
            ("third", None) => response(200, r#"{"data":[5]}"#),
            _ => response(500, r#"{"error":"unexpected test request"}"#),
        })
    }
}

fn settle(transport: &Settling) -> Result<JsonValue, String> {
    let mut descriptor = serde_json::to_value(spec()?).map_err(|error| error.to_string())?;
    descriptor["rate_limit"] = serde_json::json!({"reset_header": "x-ratelimit-reset"});
    let spec: HttpImplementation =
        serde_json::from_value(descriptor).map_err(|error| error.to_string())?;
    let arguments = serde_json::from_value::<JsonValue>(serde_json::json!({
        "queries": [{"name": "first"}, {"name": "second"}, {"name": "third"}]
    }))
    .map_err(|error| error.to_string())?;
    let (parameters, env) = (JsonObject::new(), BTreeMap::new());
    let call = AdmittedCall::for_tests(&arguments, &parameters, &env, OperationAccess::Read);
    Http::new(Egress::Direct(transport), &call, &spec)
        .perform(None)
        .map(|read| read.value)
        .map_err(Unperformed::reason)
}

fn json(value: serde_json::Value) -> Result<JsonValue, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

#[test]
fn a_failed_item_reports_its_error_and_the_rest_go_on() -> Result<(), String> {
    // The second query's provider refuses it outright.
    let refused = Settling {
        second: "first page",
        status: 404,
        reset: None,
        requests: Mutex::default(),
    };
    let read = settle(&refused)?;
    assert_eq!(
        at_path(&read, "0"),
        Some(&json(serde_json::json!(
            {"items":[1,2],"next":null,"started":true,"complete":true,"error":null}
        ))?)
    );
    assert_eq!(
        at_path(&read, "1"),
        Some(&json(serde_json::json!({
            "items":[],"next":null,"started":true,"complete":false,
            "error":{"code":"provider_rejected","message":"the provider returned HTTP 404"}
        }))?)
    );
    assert_eq!(
        at_path(&read, "2"),
        Some(&json(serde_json::json!(
            {"items":[5],"next":null,"started":true,"complete":true,"error":null}
        ))?)
    );
    // A failure after a page keeps what was read and where to go on from.
    let broken = Settling {
        second: "later page",
        status: 502,
        reset: None,
        requests: Mutex::default(),
    };
    let read = settle(&broken)?;
    assert_eq!(
        at_path(&read, "1"),
        Some(&json(serde_json::json!({
            "items":[3],"next":2,"started":true,"complete":false,
            "error":{"code":"invalid_response","message":"the provider returned HTTP 502"}
        }))?)
    );
    assert_eq!(at_path(&read, "2.complete"), Some(&JsonValue::Bool(true)));
    Ok(())
}

#[test]
fn a_rate_limit_leaves_the_rest_unstarted() -> Result<(), String> {
    let limited = Settling {
        second: "later page",
        status: 429,
        reset: Some("1800000000"),
        requests: Mutex::default(),
    };
    let read = settle(&limited)?;
    assert_eq!(at_path(&read, "0.complete"), Some(&JsonValue::Bool(true)));
    assert_eq!(
        at_path(&read, "1"),
        Some(&json(serde_json::json!({
            "items":[3],"next":2,"started":true,"complete":false,
            "error":{
                "code":"rate_limited",
                "message":"the provider rate limited the request until 2027-01-15T08:00:00Z",
                "retry_at":"2027-01-15T08:00:00Z"
            }
        }))?)
    );
    // Never "no matches": the third query is unstarted, and was never sent.
    assert_eq!(
        at_path(&read, "2"),
        Some(&json(serde_json::json!(
            {"items":[],"next":null,"started":false,"complete":false,"error":null}
        ))?)
    );
    let sent = limited.requests.lock().expect("requests");
    assert_eq!(*sent, ["first:", "first:2", "second:", "second:2"]);
    Ok(())
}

#[test]
fn empty_page_keeps_its_continuation_and_does_not_claim_complete() -> Result<(), String> {
    let transport = CursorAnswer {
        answer: r#"{"next":"later"}"#,
        calls: AtomicUsize::new(0),
    };
    let read = run(&transport, &spec()?).map_err(Unperformed::reason)?;
    assert_eq!(
        at_path(&read.value, "0.items"),
        Some(&JsonValue::Array(Vec::new()))
    );
    assert_eq!(
        at_path(&read.value, "0.next"),
        Some(&JsonValue::String("later".to_owned()))
    );
    assert_eq!(
        at_path(&read.value, "0.started"),
        Some(&JsonValue::Bool(true))
    );
    assert_eq!(
        at_path(&read.value, "0.complete"),
        Some(&JsonValue::Bool(false))
    );
    Ok(())
}

#[test]
fn a_resumed_query_cursor_replaces_the_value_embedded_in_the_url() -> Result<(), String> {
    #[derive(Default)]
    struct QueryPages {
        requests: Mutex<Vec<String>>,
    }
    impl RuntimeHttpTransport for QueryPages {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
            let mut requests = self.requests.lock().expect("requests");
            requests.push(request.url);
            Ok(if requests.len() == 1 {
                response(200, r#"{"data":[1],"next":"second"}"#)
            } else {
                response(200, r#"{"data":[2]}"#)
            })
        }
    }
    let spec: HttpImplementation = serde_json::from_value(serde_json::json!({
        "allowed_hosts":["api.example.test"],
        "requests":[{"id":"query","http_method":"GET","url":"https://api.example.test/query?cursor=first&keep=yes","pages":{"items":"data","next":{"cursor":"next","param":"cursor"}}}],
        "decode":[{"op":"auto_json"}],"result_paths":{"rows":"acknowledgement"}
    })).map_err(|error|error.to_string())?;
    let transport = QueryPages::default();
    run(&transport, &spec).map_err(Unperformed::reason)?;
    let requests = transport.requests.lock().expect("requests");
    assert_eq!(requests.len(), 2);
    for (index, request) in requests.iter().enumerate() {
        let url = url::Url::parse(request).map_err(|error| error.to_string())?;
        let cursors = url
            .query_pairs()
            .filter(|(name, _)| name == "cursor")
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>();
        assert_eq!(cursors, vec![if index == 0 { "first" } else { "second" }]);
        assert!(
            url.query_pairs()
                .any(|(name, value)| name == "keep" && value == "yes")
        );
    }
    Ok(())
}
