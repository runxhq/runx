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
        {"items":[1,2],"next":null,"started":true,"complete":true},
        {"items":[3,4,5],"next":null,"started":true,"complete":true}
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
fn malformed_and_repeated_cursors_fail_without_echoing_opaque_tokens() -> Result<(), String> {
    for (answer, expected_calls, reason) in [
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
        let failed = match run(&transport, &spec()?) {
            Err(error) => error,
            Ok(_) => return Err("invalid provider cursor was accepted".to_owned()),
        };
        let message = failed.reason();
        assert!(message.contains(reason), "{message}");
        assert!(!message.contains("opaque-do-not-expose"));
        assert_eq!(transport.calls.load(Ordering::SeqCst), expected_calls);
    }
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
