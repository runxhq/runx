use std::fs;

use runx_contracts::{JsonObject, JsonValue};
use serde_json::{Value, json};

use crate::javascript_worker_support::{JavaScriptPackage, expected_json, success_json};

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ACCOUNT_ID: &str = "42";

#[test]
fn twitter_execution_progress_is_a_compact_contiguous_cursor()
-> Result<(), Box<dyn std::error::Error>> {
    let prepare = execution_package("twitter-execution.mjs")?;
    let finalize = execution_package("twitter-execution-result.mjs")?;
    let plan = delete_plan(120);
    let first = prepare_plan(&prepare, &plan, 0, empty_ledger())?;

    assert_eq!(field(&first, "decision"), Some(&json!("ready")));
    let batch = object_field(&first, "batch")?;
    assert_eq!(field(batch, "operation"), Some(&json!("posts.delete")));
    assert_eq!(array_field(batch, "acts")?.len(), 50);
    assert!(first.get("plan_acts").is_none());
    assert!(first.get("remaining_act_ids").is_none());

    let outcomes = (0..50)
        .map(|_| json!({ "status": "applied", "acknowledgement": { "data": { "deleted": true } } }))
        .collect::<Vec<_>>();
    let completed = finalize_plan(&finalize, &first, json!({ "outcomes": outcomes }))?;
    assert_eq!(field(&completed, "next_act_index"), Some(&json!(50)));
    assert_eq!(field(&completed, "remaining_count"), Some(&json!(70)));
    assert!(completed.get("executed_act_ids").is_none());
    assert!(completed.get("remaining_act_ids").is_none());
    let progress = object_field(&completed, "ledger_delta")?;
    assert_eq!(field(progress, "next_act_index"), Some(&json!(50)));
    assert_eq!(field(progress, "account_id"), Some(&json!(ACCOUNT_ID)));
    assert!(serde_json::to_vec(progress)?.len() < 1_024);

    let second = prepare_plan(&prepare, &plan, 1, ledger(1, progress.clone()))?;
    assert_eq!(field(&second, "start_act_index"), Some(&json!(50)));
    assert_eq!(field(&second, "account_id"), Some(&json!(ACCOUNT_ID)));
    assert_eq!(
        field(
            &array_field(object_field(&second, "batch")?, "acts")?[0],
            "act_index"
        ),
        Some(&json!(50)),
    );
    assert!(serde_json::to_vec(&second)?.len() < 100_000);
    Ok(())
}

#[test]
fn twitter_execution_settles_refused_acts_and_stops_where_the_provider_halted()
-> Result<(), Box<dyn std::error::Error>> {
    let prepare = execution_package("twitter-execution.mjs")?;
    let finalize = execution_package("twitter-execution-result.mjs")?;
    let plan = delete_plan(4);
    let prepared = prepare_plan(&prepare, &plan, 0, empty_ledger())?;
    let outcomes = json!([
        { "status": "applied", "acknowledgement": { "data": { "deleted": true } } },
        { "status": "refused", "code": "provider_rejected", "reason": "the provider returned HTTP 404", "answer": { "title": "Not Found Error" } },
        { "status": "refused", "code": "rate_limited", "reason": "the provider rate limited the request", "retry_at": "2027-01-15T08:00:00Z" },
        { "status": "unsent", "code": "rate_limited", "reason": "item 3 of 4 halted the batch" }
    ]);
    let completed = finalize_plan(&finalize, &prepared, json!({ "outcomes": outcomes }))?;

    assert_eq!(field(&completed, "decision"), Some(&json!("stopped")));
    assert_eq!(field(&completed, "next_act_index"), Some(&json!(2)));
    assert_eq!(field(&completed, "remaining_count"), Some(&json!(2)));
    let results = array_field(&completed, "results")?;
    assert_eq!(results.len(), 3);
    assert_eq!(field(&results[1], "status"), Some(&json!("failed")));
    assert_eq!(
        field(&results[1], "detail"),
        Some(&json!("Not Found Error"))
    );
    assert_eq!(
        field(object_field(&completed, "rate")?, "reset_at"),
        Some(&json!("2027-01-15T08:00:00Z"))
    );
    assert_eq!(
        field(object_field(&completed, "ledger_delta")?, "next_act_index"),
        Some(&json!(2)),
    );
    Ok(())
}

#[test]
fn twitter_thread_progress_resumes_after_the_last_confirmed_segment()
-> Result<(), Box<dyn std::error::Error>> {
    let prepare = execution_package("twitter-execution.mjs")?;
    let finalize = execution_package("twitter-execution-result.mjs")?;
    let plan = json!({
        "decision": "ready",
        "principal": "account:@example",
        "acts": [
            {
                "act_id": "thread-1",
                "kind": "thread",
                "params": { "texts": ["one", "two", "three"] }
            },
            {
                "act_id": "post-2",
                "kind": "post",
                "params": { "text": "after" }
            }
        ],
        "gates": { "human_approval_required": true }
    });
    let prepared = prepare_plan(&prepare, &plan, 0, empty_ledger())?;
    let batch = object_field(&prepared, "batch")?;
    assert_eq!(field(batch, "operation"), Some(&json!("post.create")));
    assert_eq!(field(batch, "input"), Some(&json!({ "text": "one" })));

    let completed = finalize_plan(&finalize, &prepared, json!({ "post": { "id": "tweet-1" } }))?;
    assert_eq!(field(&completed, "next_act_index"), Some(&json!(0)));
    let progress = object_field(&completed, "ledger_delta")?.clone();
    let active = object_field(&progress, "active_thread")?;
    assert_eq!(field(active, "next_segment_index"), Some(&json!(1)));
    assert_eq!(field(active, "in_reply_to"), Some(&json!("tweet-1")));

    let resumed = prepare_plan(&prepare, &plan, 1, ledger(1, progress))?;
    let batch = object_field(&resumed, "batch")?;
    assert_eq!(field(batch, "operation"), Some(&json!("post.reply")));
    assert_eq!(
        field(batch, "input"),
        Some(&json!({ "text": "two", "in_reply_to": "tweet-1" }))
    );
    Ok(())
}

/// A batch runs only as the plan's principal, and every batch of a plan as
/// the account the first one ran as.
#[test]
fn twitter_execution_binds_only_the_plans_account_and_the_one_it_pinned()
-> Result<(), Box<dyn std::error::Error>> {
    let prepare = execution_package("twitter-execution.mjs")?;
    let finalize = execution_package("twitter-execution-result.mjs")?;
    let plan = delete_plan(60);
    let first = prepare_plan(&prepare, &plan, 0, empty_ledger())?;

    let call = bind(
        &prepare,
        &first,
        json!({ "id": ACCOUNT_ID, "username": "Example" }),
    )?;
    let reason = field(object_field(&call, "approval")?, "reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(reason.contains("as @Example (account 42)"), "{reason}");
    assert_eq!(field(&call, "account_id"), Some(&json!(ACCOUNT_ID)));
    let other = bind(&prepare, &first, json!({ "id": "43", "username": "other" }))
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(other.contains("is not the plan's principal"), "{other}");

    let outcomes = (0..50)
        .map(|_| json!({ "status": "applied", "acknowledgement": { "data": { "deleted": true } } }))
        .collect::<Vec<_>>();
    let completed = finalize_plan(&finalize, &first, json!({ "outcomes": outcomes }))?;
    let second = prepare_plan(
        &prepare,
        &plan,
        1,
        ledger(1, object_field(&completed, "ledger_delta")?.clone()),
    )?;
    // The handle now names another account: the credential was rebound.
    let moved = bind(
        &prepare,
        &second,
        json!({ "id": "43", "username": "example" }),
    )
    .err()
    .map(|error| error.to_string())
    .unwrap_or_default();
    assert!(moved.contains("earlier batches"), "{moved}");
    Ok(())
}

fn bind(
    package: &JavaScriptPackage,
    plan: &Value,
    account: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = package.invoke_export(
        "bindExecution",
        object(json!({ "execution_plan": plan, "account": account }))?,
    )?;
    output_object(success_json(&output)?, "twitter_call")
}

fn execution_package(entry: &str) -> Result<JavaScriptPackage, Box<dyn std::error::Error>> {
    let root = "../../skills/twitter";
    let source = fs::read_to_string(format!("{root}/{entry}"))?;
    let batch = fs::read_to_string(format!("{root}/twitter-execution-batch.mjs"))?;
    let values = fs::read_to_string(format!("{root}/twitter-execution-values.mjs"))?;
    JavaScriptPackage::with_modules(
        &source,
        [
            ("twitter-execution-batch.mjs", batch.as_str()),
            ("twitter-execution-values.mjs", values.as_str()),
        ],
    )
}

fn prepare_plan(
    package: &JavaScriptPackage,
    plan: &Value,
    expected_version: u64,
    ledger: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = package.invoke_export(
        "prepareExecution",
        object(json!({
            "plan_json": plan,
            "plan_digest": DIGEST,
            "digest_result": { "digest": DIGEST },
            "execution_ledger": ledger,
            "expected_version": expected_version,
            "idempotency_key": format!("twitter:{DIGEST}:v{}", expected_version + 1),
            "max_acts": 50
        }))?,
    )?;
    output_object(success_json(&output)?, "twitter_execution_plan")
}

fn finalize_plan(
    package: &JavaScriptPackage,
    plan: &Value,
    result: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = package.invoke_export(
        "finalizeExecution",
        object(json!({
            "execution_plan": plan,
            "provider_operation": { "status": "success", "result": result },
            "account_id": ACCOUNT_ID
        }))?,
    )?;
    output_object(success_json(&output)?, "twitter_execution")
}

fn delete_plan(count: usize) -> Value {
    let acts = (0..count)
        .map(|index| {
            json!({
                "act_id": format!("delete-{index}"),
                "kind": "delete_post",
                "params": { "post_id": index.to_string() }
            })
        })
        .collect::<Vec<_>>();
    json!({
        "decision": "ready",
        "principal": "account:@example",
        "acts": acts,
        "gates": { "human_approval_required": true }
    })
}

fn empty_ledger() -> Value {
    json!({ "after_version": 0, "events": [] })
}

fn ledger(version: u64, event: Value) -> Value {
    json!({ "after_version": version, "events": [{ "version": version, "event": event }] })
}

fn object(value: Value) -> Result<JsonObject, Box<dyn std::error::Error>> {
    expected_json(value)
        .as_object()
        .cloned()
        .ok_or_else(|| "expected JSON object".into())
}

fn output_object(value: JsonValue, field_name: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let value = serde_json::to_value(value)?;
    value
        .get(field_name)
        .cloned()
        .ok_or_else(|| format!("output omitted {field_name}").into())
}

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name)
}

fn object_field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, Box<dyn std::error::Error>> {
    value
        .get(name)
        .filter(|value| value.is_object())
        .ok_or_else(|| format!("output omitted object field {name}").into())
}

fn array_field<'a>(
    value: &'a Value,
    name: &str,
) -> Result<&'a [Value], Box<dyn std::error::Error>> {
    value
        .get(name)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("output omitted array field {name}").into())
}
