use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use tempfile::tempdir;

use super::{ToolDispatchRequest, dispatch_tool};
use crate::adapter::{InvocationOutput, InvocationStatus};
use crate::{CredentialDelivery, RuntimeEffectRegistry, RuntimeError};

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
