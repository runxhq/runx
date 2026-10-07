use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runx_contracts::{JsonNumber, JsonObject, JsonValue, MAX_PORTABLE_INTEGER, sha256_prefixed};
use tempfile::tempdir;

use super::{ToolDispatchRequest, dispatch_tool};
use crate::adapter::{InvocationOutput, InvocationStatus};
use crate::{CredentialDelivery, RuntimeEffectRegistry, RuntimeError};

mod data_store;

#[test]
fn dispatch_refuses_local_tool_with_undeclared_required_scope()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    write_catalog_tool(
        &temp.path().join("tools/test/scope-claim"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.scope-claim",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"]
  },
  "scopes": ["example:network"]
}
"#,
        "printf '%s\n' '{\"ok\":true}'\n",
    )?;

    let output = invoke_with_declared_scopes_in_directory(
        "test.scope-claim",
        JsonObject::new(),
        JsonObject::new(),
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
        &[],
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(diagnostic(&output).contains("missing required scope declaration(s): example:network"));
    Ok(())
}

#[test]
fn dispatch_invokes_local_tool_with_declared_inputs_only() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    write_catalog_tool(
        &temp.path().join("tools/test/exact-inputs"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.exact-inputs",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "message": { "type": "string", "required": true }
  },
  "scopes": ["test.exact-inputs"]
}
"#,
        r#"raw="$(cat)"
case "$raw" in
  *persona*|*thread*) printf '%s\n' '{"error":"undeclared input reached tool"}'; exit 7 ;;
  *'"message":"hello"'*) printf '%s\n' '{"ok":true}' ;;
  *) printf '%s\n' '{"error":"declared input missing"}'; exit 8 ;;
esac
"#,
    )?;
    let mut inputs = JsonObject::new();
    inputs.insert("message".to_owned(), JsonValue::String("hello".to_owned()));
    inputs.insert(
        "persona".to_owned(),
        JsonValue::String("prompt-only".to_owned()),
    );
    inputs.insert(
        "thread".to_owned(),
        JsonValue::String("context-only".to_owned()),
    );

    let output = invoke_in_directory(
        "test.exact-inputs",
        inputs,
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    assert_eq!(
        output
            .value
            .as_object()
            .and_then(|value| value.get("ok"))
            .and_then(JsonValue::as_bool),
        Some(true)
    );
    Ok(())
}

#[test]
fn dispatch_materializes_one_declared_tool_input_map() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    write_catalog_tool(
        &temp.path().join("tools/test/materialized"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.materialized",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "artifact": { "type": "json", "required": true, "artifact": true },
    "count": { "type": "integer", "required": true },
    "message": { "type": "string", "required": true },
    "mode": { "type": "string", "required": false, "default": "safe" },
    "optional": { "type": "object", "required": false }
  },
  "scopes": ["test.materialized"]
}
"#,
        "cat\n",
    )?;
    let inputs = JsonObject::from([
        (
            "artifact".to_owned(),
            JsonValue::Object(JsonObject::from([
                (
                    "schema".to_owned(),
                    JsonValue::String("test.packet.v1".to_owned()),
                ),
                (
                    "data".to_owned(),
                    JsonValue::Object(JsonObject::from([(
                        "ready".to_owned(),
                        JsonValue::Bool(true),
                    )])),
                ),
            ])),
        ),
        ("count".to_owned(), JsonValue::Number(JsonNumber::U64(3))),
        ("message".to_owned(), JsonValue::String("static".to_owned())),
        ("optional".to_owned(), JsonValue::Null),
        (
            "undeclared".to_owned(),
            JsonValue::String("must-not-leak".to_owned()),
        ),
    ]);
    let resolved_inputs = JsonObject::from([(
        "message".to_owned(),
        JsonValue::String("resolved".to_owned()),
    )]);

    let output = invoke_with_resolved_in_directory(
        "test.materialized",
        inputs,
        resolved_inputs,
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;

    assert_eq!(
        output.status,
        InvocationStatus::Success,
        "{}",
        diagnostic(&output)
    );
    let payload = output.value;
    let object = payload.as_object().ok_or("tool output must be an object")?;
    assert_eq!(
        object.get("message").and_then(JsonValue::as_str),
        Some("resolved")
    );
    assert_eq!(object.get("mode").and_then(JsonValue::as_str), Some("safe"));
    assert_eq!(
        object
            .get("artifact")
            .and_then(JsonValue::as_object)
            .and_then(|artifact| artifact.get("ready"))
            .and_then(JsonValue::as_bool),
        Some(true)
    );
    assert!(!object.contains_key("optional"));
    assert!(!object.contains_key("undeclared"));
    Ok(())
}

#[test]
fn dispatch_rejects_missing_and_type_invalid_inputs_before_tool_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    write_catalog_tool(
        &temp.path().join("tools/test/typed"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.typed",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "message": { "type": "string", "required": true }
  },
  "scopes": ["test.typed"]
}
"#,
        "printf '%s\\n' '{\"started\":true}'\n",
    )?;

    let missing = invoke_in_directory(
        "test.typed",
        JsonObject::new(),
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;
    assert_eq!(missing.status, InvocationStatus::Failure);
    assert!(diagnostic(&missing).contains("input 'message' is required"));

    let invalid = invoke_in_directory(
        "test.typed",
        JsonObject::from([("message".to_owned(), JsonValue::Number(JsonNumber::U64(1)))]),
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;
    assert_eq!(invalid.status, InvocationStatus::Failure);
    assert!(diagnostic(&invalid).contains("must be string, received integer"));
    Ok(())
}

#[test]
fn dispatch_wraps_local_tool_outputs_for_graph_context_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    write_catalog_tool(
        &temp.path().join("tools/test/wrapped"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.wrapped",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"]
  },
  "artifacts": {
    "wrap_as": "wrapped_packet"
  },
  "scopes": ["test.wrapped"]
}
"#,
        r#"printf '%s\n' '{"schema":"test.packet.v1","data":{"message":"hello"}}'
"#,
    )?;
    let output = invoke_in_directory(
        "test.wrapped",
        JsonObject::new(),
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    let payload = output.value;
    // The tool already emits a self-described `{ schema, data }` packet, so `wrap_as`
    // exposes it as-is at a SINGLE `.data` depth rather than re-wrapping into `.data.data`.
    assert_eq!(
        json_path(&payload, &["wrapped_packet", "data", "message"]),
        Some("hello")
    );
    assert!(
        json_path(&payload, &["wrapped_packet", "data", "data", "message"]).is_none(),
        "a self-described packet must not be double-wrapped"
    );
    Ok(())
}

#[test]
fn dispatch_does_not_confuse_a_domain_data_field_with_an_artifact_envelope()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    write_catalog_tool(
        &temp.path().join("tools/test/page"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.page",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"]
  },
  "artifacts": {
    "wrap_as": "page_packet"
  },
  "scopes": ["test.page"]
}
"#,
        r#"printf '%s\n' '{"offset":0,"data":"page bytes"}'
"#,
    )?;
    let output = invoke_in_directory(
        "test.page",
        JsonObject::new(),
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    let payload = output.value;
    assert_eq!(
        json_path(&payload, &["page_packet", "data", "data"]),
        Some("page bytes")
    );
    let page = payload
        .as_object()
        .and_then(|object| object.get("page_packet"))
        .and_then(JsonValue::as_object)
        .and_then(|object| object.get("data"))
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            std::io::Error::other("the canonical envelope must contain the domain page object")
        })?;
    assert!(page.contains_key("offset"));
    Ok(())
}

#[test]
fn dispatch_wraps_local_named_emits_for_graph_context_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    write_catalog_tool(
        &temp.path().join("tools/test/named"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.named",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"]
  },
  "artifacts": {
    "named_emits": {
      "draft_pull_request": "draft_pull_request_packet"
    }
  },
  "scopes": ["test.named"]
}
"#,
        r#"printf '%s\n' '{"draft_pull_request":{"title":"hello"}}'
"#,
    )?;
    let output = invoke_in_directory(
        "test.named",
        JsonObject::new(),
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    let payload = output.value;
    assert_eq!(
        json_path(&payload, &["draft_pull_request", "data", "title"]),
        Some("hello")
    );
    Ok(())
}

// Regression: a manifest that names the SAME key in both `wrap_as` and `named_emits`
// (the data-store tools do exactly this) must wrap the payload exactly once. Before the
// idempotence fix, `wrap_as` synthesised `{ data: <flat> }` and `named_emits` re-wrapped
// it to `{ data: { data: <flat> } }`, drifting every consumer's path by one `.data`.
#[test]
fn dispatch_wraps_same_key_once_for_wrap_as_and_named_emits()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    write_catalog_tool(
        &temp.path().join("tools/test/operation"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.operation",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"]
  },
  "artifacts": {
    "named_emits": {
      "data_operation_result": "runx.data.operation_result.v1"
    },
    "wrap_as": "data_operation_result"
  },
  "scopes": ["test.operation"]
}
"#,
        r#"printf '%s\n' '{"status":"read","events":"present"}'
"#,
    )?;
    let output = invoke_in_directory(
        "test.operation",
        JsonObject::new(),
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    let payload = output.value;
    assert_eq!(
        json_path(&payload, &["data_operation_result", "data", "events"]),
        Some("present"),
        "events must resolve at a single `.data` depth"
    );
    assert!(
        json_path(
            &payload,
            &["data_operation_result", "data", "data", "events"]
        )
        .is_none(),
        "the payload must not be double-wrapped"
    );
    Ok(())
}

#[test]
fn dispatch_resolves_unbound_local_data_source_to_durable_sqlite_adapter()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut inputs = JsonObject::new();
    inputs.insert(
        "data_source_ref".to_owned(),
        JsonValue::String("local://runx-data-store/test".to_owned()),
    );
    inputs.insert(
        "resource".to_owned(),
        JsonValue::String("board_events".to_owned()),
    );
    inputs.insert(
        "aggregate_id".to_owned(),
        JsonValue::String("posting-1".to_owned()),
    );

    let output = invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    let payload = output.value;
    assert_eq!(
        json_path(
            &payload,
            &[
                "data_operation_result",
                "data",
                "provider_evidence",
                "adapter"
            ]
        ),
        Some("data.sqlite")
    );
    Ok(())
}

#[test]
fn dispatch_resolves_configured_data_source_binding() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join(".runx"))?;
    fs::write(
        temp.path().join(".runx/data-sources.json"),
        r#"{
  "data_sources": {
    "tenant://acme/board": {
      "adapter": "test.bound",
      "profile": "prod-board",
      "resources": {
        "board_events": { "kind": "event_stream" }
      }
    }
  }
}
"#,
    )?;
    let result = empty_projection_result("tenant://acme/board", "test.bound")?;
    let runner = format!(
        r#"raw="$(cat)"
case "$raw" in
  *'"adapter":"test.bound"'*|*'"adapter": "test.bound"'*)
    case "$raw" in
      *'"profile":"prod-board"'*|*'"profile": "prod-board"'*)
        case "$raw" in
          *'"operation":"read_projection"'*|*'"operation": "read_projection"'*) printf '%s\n' '{result}' ;;
          *) printf 'missing exact operation: %s\n' "$raw" >&2; exit 10 ;;
        esac
        ;;
      *) printf 'missing profile: %s\n' "$raw" >&2; exit 9 ;;
    esac
    ;;
  *) printf 'missing configured binding: %s\n' "$raw" >&2; exit 8 ;;
esac
"#,
    );
    write_catalog_tool(
        &temp.path().join("tools/test/bound"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.bound",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "operation": { "type": "string", "required": true },
    "data_source_ref": { "type": "string", "required": true },
    "data_source_binding": { "type": "json", "required": true }
  },
  "scopes": ["runx:data:read"]
}
"#,
        &runner,
    )?;
    let inputs = read_projection_inputs("tenant://acme/board");
    let mut env = tool_root_env(temp.path());
    env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );

    let output = invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        env,
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    let payload = output.value;
    assert_eq!(
        json_path(
            &payload,
            &[
                "data_operation_result",
                "data",
                "provider_evidence",
                "adapter"
            ]
        ),
        Some("test.bound")
    );
    Ok(())
}

#[test]
fn dispatch_applies_the_native_data_contract_to_tool_backed_adapters()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join(".runx"))?;
    fs::write(
        temp.path().join(".runx/data-sources.json"),
        r#"{"data_sources":{"tenant://acme/events":{"adapter":"test.append","profile":"events-prod"}}}"#,
    )?;
    let result = committed_append_result("tenant://acme/events", "test.append")?;
    let runner = format!(
        r#"raw="$(cat)"
case "$raw" in
  *'"operation":"append_event"'*|*'"operation": "append_event"'*) ;;
  *) printf 'missing operation: %s\n' "$raw" >&2; exit 8 ;;
esac
case "$raw" in
  *'"observed_at":"2026-01-01T00:00:00.000Z"'*|*'"observed_at": "2026-01-01T00:00:00.000Z"'*) ;;
  *) printf 'missing runtime observation: %s\n' "$raw" >&2; exit 9 ;;
esac
case "$raw" in
  *'"resource":"board_events"'*|*'"resource": "board_events"'*) ;;
  *) printf 'missing core-prepared resource: %s\n' "$raw" >&2; exit 10 ;;
esac
case "$raw" in
  *'"event_type":"posting.created"'*|*'"event_type": "posting.created"'*) ;;
  *) printf 'missing core-derived event type: %s\n' "$raw" >&2; exit 11 ;;
esac
case "$raw" in
  *'"event_digest":"sha256:'*|*'"event_digest": "sha256:'*) ;;
  *) printf 'missing core-derived event digest: %s\n' "$raw" >&2; exit 12 ;;
esac
case "$raw" in
  *'"idempotency_key":"posting-1:create:v1"'*|*'"idempotency_key": "posting-1:create:v1"'*) printf '%s\n' '{result}' ;;
  *) printf 'missing core-prepared idempotency: %s\n' "$raw" >&2; exit 13 ;;
esac
"#,
    );
    write_catalog_tool(
        &temp.path().join("tools/test/append"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.append",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "operation": { "type": "string", "required": true },
    "data_source_binding": { "type": "json", "required": true }
  },
  "artifacts": { "wrap_as": "adapter_owned_packet" },
  "scopes": ["runx:data:append"]
}
"#,
        &runner,
    )?;
    let inputs = JsonObject::from([
        (
            "data_source_ref".to_owned(),
            JsonValue::String("tenant://acme/events".to_owned()),
        ),
        (
            "resource".to_owned(),
            JsonValue::String("board_events".to_owned()),
        ),
        (
            "aggregate_id".to_owned(),
            JsonValue::String("posting-1".to_owned()),
        ),
        (
            "expected_version".to_owned(),
            JsonValue::Number(JsonNumber::U64(0)),
        ),
        (
            "idempotency_key".to_owned(),
            JsonValue::String("posting-1:create:v1".to_owned()),
        ),
        (
            "event".to_owned(),
            JsonValue::Object(JsonObject::from([
                (
                    "type".to_owned(),
                    JsonValue::String("posting.created".to_owned()),
                ),
                ("value".to_owned(), JsonValue::Number(JsonNumber::U64(1))),
            ])),
        ),
    ]);
    let mut env = tool_root_env(temp.path());
    env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );

    let output = invoke_in_directory("data.append_event", inputs, temp.path().to_path_buf(), env)?;

    assert_eq!(
        output.status,
        InvocationStatus::Success,
        "{}",
        diagnostic(&output)
    );
    let payload = output.value;
    assert_eq!(
        json_path(&payload, &["data_operation_result", "data", "status"]),
        Some("committed")
    );
    assert!(
        json_path(
            &payload,
            &["data_operation_result", "data", "data", "status"]
        )
        .is_none(),
        "the canonical artifact owner must wrap exactly once"
    );
    assert!(
        payload
            .as_object()
            .is_some_and(|object| !object.contains_key("adapter_owned_packet")),
        "adapter artifact metadata must not override the native data contract"
    );
    Ok(())
}

#[test]
fn external_data_results_use_the_core_budget_not_the_generic_cli_limit()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join(".runx"))?;
    fs::write(
        temp.path().join(".runx/data-sources.json"),
        r#"{"data_sources":{"tenant://acme/large":{"adapter":"test.large-result"}}}"#,
    )?;
    let result = projection_result_with_provider_padding(
        "tenant://acme/large",
        "test.large-result",
        1024 * 1024,
    )?;
    let runner = format!("printf '%s\\n' '{result}'\n");
    write_catalog_tool(
        &temp.path().join("tools/test/large-result"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.large-result",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"]
  },
  "inputs": {},
  "scopes": ["runx:data:read"]
}
"#,
        &runner,
    )?;
    let mut env = tool_root_env(temp.path());
    env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );

    let output = invoke_in_directory(
        "data.read_projection",
        read_projection_inputs("tenant://acme/large"),
        temp.path().to_path_buf(),
        env,
    )?;

    assert_eq!(
        output.status,
        InvocationStatus::Success,
        "{}",
        diagnostic(&output)
    );
    assert!(serde_json::to_vec(&output.value)?.len() > 1024 * 1024);
    Ok(())
}

#[test]
fn data_operation_versions_are_limited_by_the_portable_json_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let env = tool_root_env(temp.path());
    let append = JsonObject::from([
        (
            "data_source_ref".to_owned(),
            JsonValue::String("local://portable-version".to_owned()),
        ),
        (
            "resource".to_owned(),
            JsonValue::String("events".to_owned()),
        ),
        (
            "aggregate_id".to_owned(),
            JsonValue::String("stream-1".to_owned()),
        ),
        (
            "expected_version".to_owned(),
            JsonValue::Number(JsonNumber::U64(MAX_PORTABLE_INTEGER)),
        ),
        (
            "idempotency_key".to_owned(),
            JsonValue::String("stream-1:event-1".to_owned()),
        ),
        (
            "event".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "type".to_owned(),
                JsonValue::String("stream.created".to_owned()),
            )])),
        ),
    ]);
    let append_output = invoke_in_directory(
        "data.append_event",
        append,
        temp.path().to_path_buf(),
        env.clone(),
    )?;
    assert_eq!(append_output.status, InvocationStatus::Failure);
    assert!(diagnostic(&append_output).contains("expected_version must be less"));

    let read = JsonObject::from([
        (
            "data_source_ref".to_owned(),
            JsonValue::String("local://portable-version".to_owned()),
        ),
        (
            "resource".to_owned(),
            JsonValue::String("events".to_owned()),
        ),
        (
            "aggregate_id".to_owned(),
            JsonValue::String("stream-1".to_owned()),
        ),
        ("limit".to_owned(), JsonValue::Number(JsonNumber::U64(1))),
        (
            "after_version".to_owned(),
            JsonValue::Number(JsonNumber::U64(MAX_PORTABLE_INTEGER + 1)),
        ),
    ]);
    let read_output =
        invoke_in_directory("data.read_events", read, temp.path().to_path_buf(), env)?;
    assert_eq!(read_output.status, InvocationStatus::Failure);
    assert!(diagnostic(&read_output).contains("after_version must not exceed"));
    Ok(())
}

#[test]
fn dispatch_rejects_external_data_results_that_violate_the_native_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join(".runx"))?;
    fs::write(
        temp.path().join(".runx/data-sources.json"),
        r#"{"data_sources":{"tenant://acme/board":{"adapter":"test.invalid-result"}}}"#,
    )?;
    let wrong_result = empty_projection_result("tenant://other/board", "test.invalid-result")?;
    let runner = format!("printf '%s\\n' '{wrong_result}'\n");
    write_catalog_tool(
        &temp.path().join("tools/test/invalid-result"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.invalid-result",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"]
  },
  "inputs": {},
  "scopes": ["runx:data:read"]
}
"#,
        &runner,
    )?;
    let mut env = tool_root_env(temp.path());
    env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );

    let output = invoke_in_directory(
        "data.read_projection",
        read_projection_inputs("tenant://acme/board"),
        temp.path().to_path_buf(),
        env,
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(
        output.value == JsonValue::Null,
        "invalid provider claims must not escape"
    );
    assert!(diagnostic(&output).contains("provider changed data_source_ref"));
    Ok(())
}

#[test]
fn dispatch_prefers_configured_local_data_source_over_default()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join(".runx"))?;
    fs::write(
        temp.path().join(".runx/data-sources.json"),
        r#"{
  "data_sources": {
    "local://runx-data-store/configured": {
      "adapter": "test.local-bound",
      "profile": "configured-local"
    }
  }
}
"#,
    )?;
    let result = empty_projection_result("local://runx-data-store/configured", "test.local-bound")?;
    let runner = format!(
        r#"raw="$(cat)"
case "$raw" in
  *'"adapter":"test.local-bound"'*|*'"adapter": "test.local-bound"'*) printf '%s\n' '{result}' ;;
  *) printf 'missing configured local binding: %s\n' "$raw" >&2; exit 8 ;;
esac
"#,
    );
    write_catalog_tool(
        &temp.path().join("tools/test/local-bound"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.local-bound",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "data_source_ref": { "type": "string", "required": true },
    "data_source_binding": { "type": "json", "required": true }
  },
  "scopes": ["runx:data:read"]
}
"#,
        &runner,
    )?;
    let inputs = read_projection_inputs("local://runx-data-store/configured");
    let mut env = tool_root_env(temp.path());
    env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );

    let output = invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        env,
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    let payload = output.value;
    assert_eq!(
        json_path(
            &payload,
            &[
                "data_operation_result",
                "data",
                "provider_evidence",
                "adapter"
            ]
        ),
        Some("test.local-bound")
    );
    Ok(())
}

#[test]
fn dispatch_resolves_relative_data_sources_env_from_workspace_root()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join("config"))?;
    fs::write(
        temp.path().join("config/data-sources.json"),
        r#"{
  "data_sources": {
    "tenant://acme/ledger": {
      "adapter": "test.env-bound",
      "profile": "ledger-prod"
    }
  }
}
"#,
    )?;
    let result = empty_projection_result("tenant://acme/ledger", "test.env-bound")?;
    let runner = format!(
        r#"raw="$(cat)"
case "$raw" in
  *'"adapter":"test.env-bound"'*|*'"adapter": "test.env-bound"'*) printf '%s\n' '{result}' ;;
  *) printf 'missing env binding: %s\n' "$raw" >&2; exit 8 ;;
esac
"#,
    );
    write_catalog_tool(
        &temp.path().join("tools/test/env-bound"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.env-bound",
  "source": {
    "type": "cli-tool",
    "command": "/bin/sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "data_source_ref": { "type": "string", "required": true },
    "data_source_binding": { "type": "json", "required": true }
  },
  "scopes": ["runx:data:read"]
}
"#,
        &runner,
    )?;
    let inputs = read_projection_inputs("tenant://acme/ledger");
    let mut env = tool_root_env(temp.path());
    env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );
    env.insert(
        "RUNX_DATA_SOURCES".to_owned(),
        "config/data-sources.json".to_owned(),
    );

    let output = invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        env,
    )?;

    assert_eq!(output.status, InvocationStatus::Success);
    let payload = output.value;
    assert_eq!(
        json_path(
            &payload,
            &[
                "data_operation_result",
                "data",
                "provider_evidence",
                "adapter"
            ]
        ),
        Some("test.env-bound")
    );
    Ok(())
}

#[test]
fn dispatch_fails_closed_for_invalid_data_sources_env_json()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let inputs = read_projection_inputs("tenant://acme/board");
    let mut env = tool_root_env(temp.path());
    env.insert("RUNX_DATA_SOURCES".to_owned(), "{not-json".to_owned());

    let output = invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        env,
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(diagnostic(&output).contains("not valid JSON"));
    Ok(())
}

#[test]
fn dispatch_fails_closed_for_missing_required_data_sources_file()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let inputs = read_projection_inputs("tenant://acme/board");
    let mut env = tool_root_env(temp.path());
    env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );
    env.insert(
        "RUNX_DATA_SOURCES".to_owned(),
        "missing/data-sources.json".to_owned(),
    );

    let output = invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        env,
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    let message = diagnostic(&output);
    assert!(message.contains("Failed to read data source config"));
    assert!(message.contains("missing/data-sources.json"));
    Ok(())
}

#[test]
fn dispatch_fails_closed_for_unbound_non_local_data_source()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let inputs = read_projection_inputs("tenant://missing/board");

    let output = invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        tool_root_env(temp.path()),
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    let message = diagnostic(&output);
    assert!(message.contains("tenant://missing/board"));
    assert!(message.contains(".runx/data-sources.json"));
    Ok(())
}

#[test]
fn dispatch_fails_closed_for_data_source_binding_without_adapter()
-> Result<(), Box<dyn std::error::Error>> {
    let output = invoke_data_source_with_inline_binding(
        "tenant://acme/board",
        r#"{"data_sources":{"tenant://acme/board":{"profile":"missing-adapter"}}}"#,
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(diagnostic(&output).contains("missing adapter"));
    Ok(())
}

#[test]
fn dispatch_fails_closed_for_recursive_data_source_adapter()
-> Result<(), Box<dyn std::error::Error>> {
    let output = invoke_data_source_with_inline_binding(
        "tenant://acme/board",
        r#"{"data_sources":{"tenant://acme/board":{"adapter":"data.read_projection"}}}"#,
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(diagnostic(&output).contains("cannot bind to operation capability"));
    Ok(())
}

#[test]
fn dispatch_fails_closed_for_non_namespaced_data_source_adapter()
-> Result<(), Box<dyn std::error::Error>> {
    let output = invoke_data_source_with_inline_binding(
        "tenant://acme/board",
        r#"{"data_sources":{"tenant://acme/board":{"adapter":"postgres"}}}"#,
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert!(diagnostic(&output).contains("must be a namespaced tool ref"));
    Ok(())
}

#[test]
fn dispatch_rejects_secret_material_in_data_source_binding()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    fs::create_dir_all(temp.path().join(".runx"))?;
    fs::write(
        temp.path().join(".runx/data-sources.json"),
        r#"{
  "data_sources": {
    "tenant://acme/board": {
      "adapter": "test.bound",
      "api_key": "raw-secret-value"
    }
  }
}
"#,
    )?;
    let inputs = read_projection_inputs("tenant://acme/board");
    let mut env = tool_root_env(temp.path());
    env.insert(
        "RUNX_CWD".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );

    let output = invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        env,
    )?;

    assert_eq!(output.status, InvocationStatus::Failure);
    let message = diagnostic(&output);
    assert!(message.contains("api_key"));
    assert!(message.contains("credential profile"));
    Ok(())
}

fn invoke_data_source_with_inline_binding(
    data_source_ref: &str,
    config: &str,
) -> Result<InvocationOutput, Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let inputs = read_projection_inputs(data_source_ref);
    let mut env = tool_root_env(temp.path());
    env.insert("RUNX_DATA_SOURCES".to_owned(), config.to_owned());

    Ok(invoke_in_directory(
        "data.read_projection",
        inputs,
        temp.path().to_path_buf(),
        env,
    )?)
}

fn diagnostic(output: &InvocationOutput) -> String {
    output.failure_message().unwrap_or_default()
}

fn invoke_in_directory(
    tool_ref: &str,
    inputs: JsonObject,
    skill_directory: PathBuf,
    env: BTreeMap<String, String>,
) -> Result<InvocationOutput, RuntimeError> {
    invoke_with_resolved_in_directory(tool_ref, inputs, JsonObject::new(), skill_directory, env)
}

fn invoke_with_resolved_in_directory(
    tool_ref: &str,
    inputs: JsonObject,
    resolved_inputs: JsonObject,
    skill_directory: PathBuf,
    env: BTreeMap<String, String>,
) -> Result<InvocationOutput, RuntimeError> {
    let scopes = crate::tool_catalogs::native::required_scopes(tool_ref).map_or_else(
        || vec![tool_ref.to_owned()],
        |scopes| scopes.iter().map(|scope| (*scope).to_owned()).collect(),
    );
    invoke_with_declared_scopes_in_directory(
        tool_ref,
        inputs,
        resolved_inputs,
        skill_directory,
        env,
        &scopes,
    )
}

fn invoke_with_declared_scopes_in_directory(
    tool_ref: &str,
    inputs: JsonObject,
    resolved_inputs: JsonObject,
    skill_directory: PathBuf,
    env: BTreeMap<String, String>,
    scopes: &[String],
) -> Result<InvocationOutput, RuntimeError> {
    let credential_delivery = CredentialDelivery::none();
    let effects = RuntimeEffectRegistry::default();
    let javascript = crate::adapters::javascript::JavaScriptAdapter::default();
    let local_artifacts = crate::services::LocalArtifactService::default();
    dispatch_tool(
        ToolDispatchRequest {
            tool_ref: Cow::Borrowed(tool_ref),
            inputs: Cow::Owned(inputs),
            resolved_inputs: Cow::Owned(resolved_inputs),
            scopes,
            env: &env,
            skill_directory: &skill_directory,
            credential_delivery: &credential_delivery,
            local_artifacts: &local_artifacts,
            javascript: &javascript,
            skill_name: "tool-dispatch-test",
            allow_explicit_manifest_path: true,
            effect_admission: None,
        },
        &effects,
        "2026-01-01T00:00:00Z",
        Instant::now(),
    )
}

fn write_catalog_tool(
    tool_dir: &Path,
    manifest: &str,
    runner: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(tool_dir)?;
    fs::write(tool_dir.join("manifest.json"), manifest)?;
    fs::write(tool_dir.join("run.sh"), runner)?;
    Ok(())
}

fn tool_root_env(root: &Path) -> BTreeMap<String, String> {
    let mut env = process_env();
    env.insert(
        "RUNX_TOOL_ROOTS".to_owned(),
        root.join("tools").to_string_lossy().into_owned(),
    );
    env
}

fn read_projection_inputs(data_source_ref: &str) -> JsonObject {
    JsonObject::from([
        (
            "data_source_ref".to_owned(),
            JsonValue::String(data_source_ref.to_owned()),
        ),
        (
            "resource".to_owned(),
            JsonValue::String("board_events".to_owned()),
        ),
        (
            "aggregate_id".to_owned(),
            JsonValue::String("posting-1".to_owned()),
        ),
    ])
}

fn empty_projection_result(
    data_source_ref: &str,
    adapter: &str,
) -> Result<String, serde_json::Error> {
    let projection = JsonObject::from([
        (
            "aggregate_id".to_owned(),
            JsonValue::String("posting-1".to_owned()),
        ),
        (
            "resource".to_owned(),
            JsonValue::String("board_events".to_owned()),
        ),
        ("version".to_owned(), JsonValue::Number(JsonNumber::U64(0))),
        (
            "event_count".to_owned(),
            JsonValue::Number(JsonNumber::U64(0)),
        ),
        ("last_event_ref".to_owned(), JsonValue::Null),
        ("last_event_type".to_owned(), JsonValue::Null),
        ("last_event_digest".to_owned(), JsonValue::Null),
    ]);
    let empty_projection_digest = digest(&JsonValue::Object(JsonObject::from([
        ("version".to_owned(), JsonValue::Number(JsonNumber::U64(0))),
        ("event_digest".to_owned(), JsonValue::Null),
    ])))?;
    let result = JsonValue::Object(JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String("runx.data.operation_result.v1".to_owned()),
        ),
        (
            "data_source_ref".to_owned(),
            JsonValue::String(data_source_ref.to_owned()),
        ),
        (
            "provider".to_owned(),
            JsonValue::String(format!("{adapter}-fixture")),
        ),
        (
            "operation".to_owned(),
            JsonValue::String("read_projection".to_owned()),
        ),
        (
            "resource".to_owned(),
            JsonValue::String("board_events".to_owned()),
        ),
        (
            "aggregate_id".to_owned(),
            JsonValue::String("posting-1".to_owned()),
        ),
        ("status".to_owned(), JsonValue::String("read".to_owned())),
        (
            "before_version".to_owned(),
            JsonValue::Number(JsonNumber::U64(0)),
        ),
        (
            "after_version".to_owned(),
            JsonValue::Number(JsonNumber::U64(0)),
        ),
        ("idempotency_key".to_owned(), JsonValue::Null),
        ("event_ref".to_owned(), JsonValue::Null),
        ("event_digest".to_owned(), JsonValue::Null),
        (
            "result_digest".to_owned(),
            JsonValue::String(digest(&JsonValue::Object(projection.clone()))?),
        ),
        (
            "projection_digest".to_owned(),
            JsonValue::String(empty_projection_digest),
        ),
        ("projection".to_owned(), JsonValue::Object(projection)),
        ("events".to_owned(), JsonValue::Array(Vec::new())),
        ("rows".to_owned(), JsonValue::Array(Vec::new())),
        ("redactions".to_owned(), JsonValue::Array(Vec::new())),
        ("stop_conditions".to_owned(), JsonValue::Array(Vec::new())),
        (
            "provider_evidence".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "adapter".to_owned(),
                JsonValue::String(adapter.to_owned()),
            )])),
        ),
    ]));
    serde_json::to_string(&result)
}

fn projection_result_with_provider_padding(
    data_source_ref: &str,
    adapter: &str,
    padding_bytes: usize,
) -> Result<String, serde_json::Error> {
    let mut result: JsonValue =
        serde_json::from_str(&empty_projection_result(data_source_ref, adapter)?)?;
    if let JsonValue::Object(result) = &mut result
        && let Some(JsonValue::Object(evidence)) = result.get_mut("provider_evidence")
    {
        evidence.insert(
            "diagnostic".to_owned(),
            JsonValue::String("x".repeat(padding_bytes)),
        );
    }
    serde_json::to_string(&result)
}

fn committed_append_result(
    data_source_ref: &str,
    adapter: &str,
) -> Result<String, serde_json::Error> {
    let event = JsonObject::from([
        (
            "type".to_owned(),
            JsonValue::String("posting.created".to_owned()),
        ),
        ("value".to_owned(), JsonValue::Number(JsonNumber::U64(1))),
    ]);
    let event_digest = digest(&JsonValue::Object(event.clone()))?;
    let event_ref = "board_events:posting-1:1";
    let record = JsonValue::Object(JsonObject::from([
        (
            "event_ref".to_owned(),
            JsonValue::String(event_ref.to_owned()),
        ),
        ("version".to_owned(), JsonValue::Number(JsonNumber::U64(1))),
        (
            "event_type".to_owned(),
            JsonValue::String("posting.created".to_owned()),
        ),
        ("event".to_owned(), JsonValue::Object(event)),
        (
            "event_digest".to_owned(),
            JsonValue::String(event_digest.clone()),
        ),
        (
            "idempotency_key".to_owned(),
            JsonValue::String("posting-1:create:v1".to_owned()),
        ),
        (
            "committed_at".to_owned(),
            JsonValue::String("2026-01-01T00:00:00.000Z".to_owned()),
        ),
    ]));
    let empty_projection_digest = digest(&JsonValue::Object(JsonObject::from([
        ("version".to_owned(), JsonValue::Number(JsonNumber::U64(0))),
        ("event_digest".to_owned(), JsonValue::Null),
    ])))?;
    let projection_digest = digest(&JsonValue::Object(JsonObject::from([
        ("version".to_owned(), JsonValue::Number(JsonNumber::U64(1))),
        (
            "previous_projection_digest".to_owned(),
            JsonValue::String(empty_projection_digest),
        ),
        (
            "event_digest".to_owned(),
            JsonValue::String(event_digest.clone()),
        ),
    ])))?;
    let result = JsonValue::Object(JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String("runx.data.operation_result.v1".to_owned()),
        ),
        (
            "data_source_ref".to_owned(),
            JsonValue::String(data_source_ref.to_owned()),
        ),
        (
            "provider".to_owned(),
            JsonValue::String(format!("{adapter}-fixture")),
        ),
        (
            "operation".to_owned(),
            JsonValue::String("append_event".to_owned()),
        ),
        (
            "resource".to_owned(),
            JsonValue::String("board_events".to_owned()),
        ),
        (
            "aggregate_id".to_owned(),
            JsonValue::String("posting-1".to_owned()),
        ),
        (
            "status".to_owned(),
            JsonValue::String("committed".to_owned()),
        ),
        (
            "before_version".to_owned(),
            JsonValue::Number(JsonNumber::U64(0)),
        ),
        (
            "after_version".to_owned(),
            JsonValue::Number(JsonNumber::U64(1)),
        ),
        (
            "idempotency_key".to_owned(),
            JsonValue::String("posting-1:create:v1".to_owned()),
        ),
        (
            "event_ref".to_owned(),
            JsonValue::String(event_ref.to_owned()),
        ),
        ("event_digest".to_owned(), JsonValue::String(event_digest)),
        (
            "result_digest".to_owned(),
            JsonValue::String(digest(&record)?),
        ),
        (
            "projection_digest".to_owned(),
            JsonValue::String(projection_digest),
        ),
        ("events".to_owned(), JsonValue::Array(Vec::new())),
        ("rows".to_owned(), JsonValue::Array(Vec::new())),
        ("redactions".to_owned(), JsonValue::Array(Vec::new())),
        ("stop_conditions".to_owned(), JsonValue::Array(Vec::new())),
        (
            "provider_evidence".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "adapter".to_owned(),
                JsonValue::String(adapter.to_owned()),
            )])),
        ),
    ]));
    serde_json::to_string(&result)
}

fn digest(value: &JsonValue) -> Result<String, serde_json::Error> {
    serde_json::to_vec(value).map(|bytes| sha256_prefixed(&bytes))
}

fn json_path<'a>(value: &'a JsonValue, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for segment in path {
        let JsonValue::Object(object) = current else {
            return None;
        };
        current = object.get(*segment)?;
    }
    match current {
        JsonValue::String(value) => Some(value),
        _ => None,
    }
}

fn process_env() -> BTreeMap<String, String> {
    [
        "PATH",
        "HOME",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
    .collect()
}
