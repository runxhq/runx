#![cfg(feature = "mcp")]

#[cfg(all(feature = "mcp", feature = "cli-tool"))]
use std::fs;
use std::io::Cursor;
#[cfg(feature = "mcp")]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(feature = "mcp")]
use runx_contracts::{ClosureDisposition, ReceiptSchema};
use runx_contracts::{JsonObject, JsonValue};
#[cfg(feature = "mcp")]
use runx_runtime::RuntimeReceiptSignatureConfig;
#[cfg(feature = "mcp")]
use runx_runtime::adapters::mcp::McpServerExecutionOptions;
use runx_runtime::adapters::mcp::{
    McpContent, McpServerOptions, McpServerTool, McpServerToolBehavior, McpToolResult,
    mcp_tool_result_from_run_result, serve_mcp_json_rpc,
};
#[cfg(feature = "mcp")]
use runx_runtime::receipts::store::LocalReceiptStore;

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";
#[test]
#[cfg(feature = "mcp")]
fn mcp_server_initializes_lists_and_calls_tools() -> Result<(), Box<dyn std::error::Error>> {
    let responses = run_server(vec![
        rmcp_initialize_request(1),
        initialized_notification(),
        request(2, "tools/list", JsonObject::new()),
        request(
            3,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("echo".to_owned())),
                ("arguments".to_owned(), JsonValue::Object(JsonObject::new())),
            ]
            .into(),
        ),
    ])?;

    assert_eq!(
        path(&responses[0], &["result", "protocolVersion"]),
        Some(&JsonValue::String("2025-06-18".to_owned()))
    );
    assert_eq!(
        path(&responses[1], &["result", "tools", "0", "name"]),
        Some(&JsonValue::String("echo".to_owned()))
    );
    assert_eq!(path(&responses[1], &["result", "tools", "1", "name"]), None);
    assert_eq!(
        path(&responses[2], &["result", "content", "0", "text"]),
        Some(&JsonValue::String("hello from server".to_owned()))
    );
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_preserves_recorded_stdio_semantics() -> Result<(), Box<dyn std::error::Error>> {
    for fixture_name in ["basic-lifecycle", "error-paths"] {
        let input = frame_jsonl_fixture(fixture_name, "requests")?;
        let expected = frame_jsonl_fixture(fixture_name, "responses")?;
        let output = run_raw_output_with_options(input, server_options())?;

        assert_newline_framing(&output)?;
        assert_eq!(
            normalize_fixture_messages(sort_responses_by_id(parse_frames(&output)?)),
            normalize_fixture_messages(sort_responses_by_id(parse_frames(&expected)?)),
            "{fixture_name} MCP stdio semantics changed"
        );
    }
    Ok(())
}

#[test]
fn mcp_server_runs_rmcp_basic_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let responses = run_server(vec![
        rmcp_initialize_request(1),
        initialized_notification(),
        request(2, "tools/list", JsonObject::new()),
        request(
            3,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("echo".to_owned())),
                ("arguments".to_owned(), JsonValue::Object(JsonObject::new())),
            ]
            .into(),
        ),
    ])?;

    assert_eq!(
        path(&responses[0], &["result", "protocolVersion"]),
        Some(&JsonValue::String("2025-06-18".to_owned()))
    );
    assert_eq!(
        path(&responses[1], &["result", "tools", "0", "name"]),
        Some(&JsonValue::String("echo".to_owned()))
    );
    assert_eq!(
        path(&responses[2], &["result", "content", "0", "text"]),
        Some(&JsonValue::String("hello from server".to_owned()))
    );
    Ok(())
}

#[test]
fn mcp_server_replays_recorded_basic_lifecycle_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let input = frame_jsonl_fixture("basic-lifecycle", "requests")?;
    let responses = run_raw_with_options(input, server_options())?;

    assert_eq!(
        path(&responses[0], &["result", "protocolVersion"]),
        Some(&JsonValue::String("2025-06-18".to_owned()))
    );
    assert_eq!(
        path(&responses[1], &["result", "tools", "0", "name"]),
        Some(&JsonValue::String("echo".to_owned()))
    );
    assert_eq!(
        path(&responses[2], &["result", "content", "0", "text"]),
        Some(&JsonValue::String("hello from server".to_owned()))
    );
    Ok(())
}

#[test]
fn mcp_server_handles_many_calls_in_one_streaming_session() -> Result<(), Box<dyn std::error::Error>>
{
    let mut requests = vec![
        rmcp_initialize_request(1),
        initialized_notification(),
        request(2, "tools/list", JsonObject::new()),
    ];
    for index in 0..96 {
        requests.push(request(
            100 + index,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("echo".to_owned())),
                (
                    "arguments".to_owned(),
                    JsonValue::Object(
                        [(
                            "index".to_owned(),
                            JsonValue::Number(runx_contracts::JsonNumber::I64(index)),
                        )]
                        .into(),
                    ),
                ),
            ]
            .into(),
        ));
    }

    let responses = run_server(requests)?;

    assert_eq!(responses.len(), 98);
    assert_eq!(
        path(&responses[0], &["result", "protocolVersion"]),
        Some(&JsonValue::String("2025-06-18".to_owned()))
    );
    assert_eq!(
        path(&responses[1], &["result", "tools", "0", "name"]),
        Some(&JsonValue::String("echo".to_owned()))
    );
    for response in responses.iter().skip(2) {
        assert_no_json_rpc_error(response);
        assert_eq!(
            path(response, &["result", "content", "0", "text"]),
            Some(&JsonValue::String("hello from server".to_owned()))
        );
    }
    Ok(())
}

#[test]
#[cfg(all(feature = "mcp", feature = "cli-tool"))]
fn mcp_server_concurrent_call_completes_while_slow_skill_runs()
-> Result<(), Box<dyn std::error::Error>> {
    let slow_skill = tempfile::tempdir()?;
    fs::write(
        slow_skill.path().join("SKILL.md"),
        r#"---
name: slow-cli
description: Slow skill used to prove MCP dispatch concurrency.
---

Slow fixture.
"#,
    )?;
    fs::write(
        slow_skill.path().join("X.yaml"),
        r#"skill: slow-cli
version: "0.1.0"
runners:
  default:
    default: true
    type: cli-tool
    command: sh
    args:
      - ./run.sh
    timeout_seconds: 5
    inputs: {}
"#,
    )?;
    fs::write(
        slow_skill.path().join("run.sh"),
        "#!/bin/sh\nsleep 0.2\nprintf '%s\\n' '{\"slow\":true}'\n",
    )?;
    let mut options = McpServerOptions::from_skill_paths_with_execution(
        &[slow_skill.path().to_path_buf()],
        "runx-cli",
        "0.0.0",
        mcp_server_execution_options(None)?,
    )?;
    options.tools.push(fixed_tool("fast"));

    let input = [
        frame(&rmcp_initialize_request(0))?,
        frame(&initialized_notification())?,
        frame(&request(
            1,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("slow-cli".to_owned())),
                ("arguments".to_owned(), JsonValue::Object(JsonObject::new())),
            ]
            .into(),
        ))?,
        frame(&request(
            2,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("fast".to_owned())),
                ("arguments".to_owned(), JsonValue::Object(JsonObject::new())),
            ]
            .into(),
        ))?,
    ]
    .concat();

    let responses = parse_frames(&run_raw_output_with_options(input, options)?)?;
    let fast_position = responses
        .iter()
        .position(|response| response_id(response) == Some(2))
        .ok_or("missing fast tool response")?;
    let slow_position = responses
        .iter()
        .position(|response| response_id(response) == Some(1))
        .ok_or("missing slow tool response")?;

    assert!(
        fast_position < slow_position,
        "fast tool response should complete before slow skill response: {responses:?}"
    );
    assert_eq!(
        path(
            &responses[fast_position],
            &["result", "content", "0", "text"]
        ),
        Some(&JsonValue::String("hello from server".to_owned()))
    );
    assert_eq!(
        path(
            &responses[slow_position],
            &["result", "structuredContent", "runx", "status"]
        ),
        Some(&JsonValue::String("sealed".to_owned()))
    );
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_skill_tool_execution_returns_completed_runx_structured_content()
-> Result<(), Box<dyn std::error::Error>> {
    let responses = run_server_with_options(
        vec![request(
            1,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("mcp-echo".to_owned())),
                (
                    "arguments".to_owned(),
                    JsonValue::Object(
                        [(
                            "message".to_owned(),
                            JsonValue::String("hello from mcp server".to_owned()),
                        )]
                        .into(),
                    ),
                ),
            ]
            .into(),
        )],
        skill_server_options()?,
    )?;

    assert_no_json_rpc_error(&responses[0]);
    assert_eq!(
        path(
            &responses[0],
            &["result", "structuredContent", "runx", "status"]
        ),
        Some(&JsonValue::String("sealed".to_owned())),
        "unexpected MCP server skill response: {:#?}",
        responses[0]
    );
    assert_result_not_error(&responses[0]);
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_single_skill_call_writes_sealed_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let receipt_root = tempfile::tempdir()?;
    let responses = run_server_with_options(
        vec![request(
            1,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("mcp-echo".to_owned())),
                (
                    "arguments".to_owned(),
                    JsonValue::Object(
                        [(
                            "message".to_owned(),
                            JsonValue::String("receipt proof".to_owned()),
                        )]
                        .into(),
                    ),
                ),
            ]
            .into(),
        )],
        skill_server_options_with_receipt_dir(receipt_root.path().to_path_buf())?,
    )?;

    assert_no_json_rpc_error(&responses[0]);
    assert_eq!(
        path(
            &responses[0],
            &["result", "structuredContent", "runx", "status"]
        ),
        Some(&JsonValue::String("sealed".to_owned())),
        "unexpected MCP server skill response: {:#?}",
        responses[0]
    );
    let JsonValue::String(receipt_id) = path(
        &responses[0],
        &["result", "structuredContent", "runx", "receipt_id"],
    )
    .ok_or("missing runx receipt id")?
    else {
        return Err("runx receipt id must be a string".into());
    };

    let signature_config =
        RuntimeReceiptSignatureConfig::from_env(&crate::support::test_signing_env())?;
    let receipt = LocalReceiptStore::new(receipt_root.path())
        .read_exact_with_policy(receipt_id, signature_config.signature_policy())?;
    assert_ne!(receipt.created_at, FIXTURE_CREATED_AT);
    assert_eq!(receipt.schema, ReceiptSchema::V1);
    assert_eq!(receipt.seal.disposition, ClosureDisposition::Closed);
    assert_eq!(receipt.id, *receipt_id);
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_missing_required_skill_input_refuses_without_false_continuation()
-> Result<(), Box<dyn std::error::Error>> {
    let responses = run_server_with_options(
        vec![request(
            1,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("mcp-echo".to_owned())),
                ("arguments".to_owned(), JsonValue::Object(JsonObject::new())),
            ]
            .into(),
        )],
        skill_server_options()?,
    )?;

    let response = serde_json::to_string(&responses[0])?;
    assert!(response.contains("required"), "{response}");
    assert!(!response.contains("runx resume"), "{response}");
    assert_eq!(
        path(&responses[0], &["result", "isError"]),
        Some(&JsonValue::Bool(true))
    );
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_never_treats_configured_model_credentials_as_managed_agent_consent()
-> Result<(), Box<dyn std::error::Error>> {
    let mut execution = mcp_server_execution_options(None)?;
    execution.env.extend([
        ("RUNX_AGENT_PROVIDER".to_owned(), "anthropic".to_owned()),
        ("RUNX_AGENT_MODEL".to_owned(), "claude-test".to_owned()),
        ("RUNX_AGENT_API_KEY".to_owned(), "test-secret".to_owned()),
    ]);
    let options = McpServerOptions::from_skill_paths_with_execution(
        &[repo_root()?.join("fixtures/skills/agent-task")],
        "runx-cli",
        "0.0.0",
        execution,
    )?;
    let responses = run_server_with_options(
        vec![request(
            1,
            "tools/call",
            [
                (
                    "name".to_owned(),
                    JsonValue::String("reviewer-boundary".to_owned()),
                ),
                (
                    "arguments".to_owned(),
                    JsonValue::Object(
                        [("prompt".to_owned(), JsonValue::String("review".to_owned()))].into(),
                    ),
                ),
            ]
            .into(),
        )],
        options,
    )?;

    assert_no_json_rpc_error(&responses[0]);
    assert_eq!(
        path(
            &responses[0],
            &["result", "structuredContent", "runx", "status"]
        ),
        Some(&JsonValue::String("needs_agent".to_owned()))
    );
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_graph_approval_pauses_with_request() -> Result<(), Box<dyn std::error::Error>> {
    let responses = run_server_with_options(
        vec![request(
            1,
            "tools/call",
            [
                (
                    "name".to_owned(),
                    JsonValue::String("mcp-approval-graph".to_owned()),
                ),
                ("arguments".to_owned(), JsonValue::Object(JsonObject::new())),
            ]
            .into(),
        )],
        approval_graph_server_options()?,
    )?;

    assert_no_json_rpc_error(&responses[0]);
    assert_eq!(
        path(
            &responses[0],
            &["result", "structuredContent", "runx", "status"]
        ),
        Some(&JsonValue::String("needs_agent".to_owned()))
    );
    assert_eq!(
        path(
            &responses[0],
            &[
                "result",
                "structuredContent",
                "runx",
                "requests",
                "0",
                "kind"
            ],
        ),
        Some(&JsonValue::String("approval".to_owned()))
    );
    assert_eq!(
        path(
            &responses[0],
            &[
                "result",
                "structuredContent",
                "runx",
                "requests",
                "0",
                "gate",
                "id"
            ],
        ),
        Some(&JsonValue::String("mcp-approval".to_owned()))
    );
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_reports_duplicate_tool_names() -> Result<(), Box<dyn std::error::Error>> {
    let options = McpServerOptions {
        package_name: "runx-cli".to_owned(),
        package_version: "0.0.0".to_owned(),
        tools: vec![fixed_tool("dup"), fixed_tool("dup")],
    };

    let error = match serve_mcp_json_rpc(Cursor::new(Vec::new()), Vec::new(), options) {
        Ok(()) => return Err("duplicate tool names fail before serving".into()),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "runx mcp serve received duplicate tool name 'dup'. Serve unique skill names only."
    );
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_json_rpc_errors_match_lifecycle_contract() -> Result<(), Box<dyn std::error::Error>> {
    let responses = run_server(vec![
        request(1, "unknown/method", JsonObject::new()),
        request(2, "tools/call", JsonObject::new()),
        request(
            3,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("missing".to_owned())),
                ("arguments".to_owned(), JsonValue::Object(JsonObject::new())),
            ]
            .into(),
        ),
        request(
            4,
            "tools/call",
            [
                ("name".to_owned(), JsonValue::String("echo".to_owned())),
                (
                    "arguments".to_owned(),
                    JsonValue::String("not an object".to_owned()),
                ),
            ]
            .into(),
        ),
    ])?;

    assert_error(&responses[0], -32601, "unknown/method");
    assert_error(&responses[1], -32601, "tools/call");
    assert_error(&responses[2], -32601, "tool not found: missing");
    assert_error(&responses[3], -32601, "tools/call");
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_rejects_oversized_requests() -> Result<(), Box<dyn std::error::Error>> {
    let input = vec![b' '; 4_194_305];
    let error = match serve_mcp_json_rpc(Cursor::new(input), Vec::new(), server_options()) {
        Ok(()) => return Err("oversized request fails".into()),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "MCP rmcp server initialization failed: MCP message exceeded size limit."
    );
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_parse_error_is_transport_error() -> Result<(), Box<dyn std::error::Error>> {
    let error = match run_raw(b"{\n".to_vec()) {
        Ok(_) => return Err("malformed JSON fails at the transport boundary".into()),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("MCP rmcp server initialization failed: EOF while parsing an object"),
        "{error}"
    );
    Ok(())
}

#[test]
#[cfg(feature = "mcp")]
fn mcp_server_mid_session_transport_error_keeps_recorded_diagnostic()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = [
        frame(&rmcp_initialize_request(1))?,
        frame(&initialized_notification())?,
    ]
    .concat();
    input.extend_from_slice(b"{\n");

    let error = match run_raw(input) {
        Ok(_) => return Err("mid-session malformed JSON fails at the transport boundary".into()),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("MCP rmcp server task failed: EOF while parsing an object"),
        "{error}"
    );
    Ok(())
}

#[test]
fn mcp_server_uses_typed_disposition_even_when_json_claims_success()
-> Result<(), Box<dyn std::error::Error>> {
    for disposition in [
        ClosureDisposition::Failed,
        ClosureDisposition::Declined,
        ClosureDisposition::Blocked,
        ClosureDisposition::Killed,
        ClosureDisposition::TimedOut,
    ] {
        let result = runx_runtime::execution::orchestrator::RunResult {
            status: runx_runtime::execution::orchestrator::RunStatus::Sealed,
            disposition: Some(disposition),
            output: JsonValue::Object(runx_status("succeeded")),
            receipt_refs: vec!["receipt-1".to_owned()],
            child_receipt_refs: Vec::new(),
            pending_requests: Vec::new(),
            diagnostics: Vec::new(),
        };
        assert!(mcp_tool_result_from_run_result(result, None).is_error);
    }

    // Readback-pending graphs are sealed Deferred without an agent request.
    // They need reconciliation, not a fresh mutation or an invented resume file.
    let deferred = mcp_tool_result_from_run_result(
        runx_runtime::execution::orchestrator::RunResult {
            status: runx_runtime::execution::orchestrator::RunStatus::Failed,
            disposition: Some(ClosureDisposition::Deferred),
            output: JsonValue::Object(runx_status("failed")),
            receipt_refs: vec!["receipt-deferred".to_owned()],
            child_receipt_refs: Vec::new(),
            pending_requests: Vec::new(),
            diagnostics: Vec::new(),
        },
        None,
    );
    assert!(!deferred.is_error);
    assert!(deferred.content[0].text.contains("receipt-deferred"));
    assert!(
        deferred.content[0]
            .text
            .contains("reconcile provider state before retrying")
    );
    assert!(!deferred.content[0].text.contains("answers.json"));
    let structured = deferred.structured_content.ok_or("typed run result")?;
    assert!(!structured.contains_key("output"));
    let metadata = structured
        .get("runx")
        .and_then(|value| value.as_object())
        .ok_or("runx metadata")?;
    assert!(!metadata.contains_key("request_digests"));
    Ok(())
}

fn run_server(requests: Vec<JsonValue>) -> Result<Vec<JsonValue>, Box<dyn std::error::Error>> {
    run_server_with_options(requests, server_options())
}

fn run_server_with_options(
    requests: Vec<JsonValue>,
    mut options: McpServerOptions,
) -> Result<Vec<JsonValue>, Box<dyn std::error::Error>> {
    let receipts = tempfile::tempdir()?;
    for tool in &mut options.tools {
        if let McpServerToolBehavior::Skill(execution) = &mut tool.result
            && execution.receipt_dir.is_none()
        {
            execution.receipt_dir = Some(receipts.path().to_path_buf());
        }
    }
    let prepend_handshake = !matches!(request_method(requests.first()), Some("initialize"));
    let mut framed_requests = Vec::new();
    if prepend_handshake {
        framed_requests.push(rmcp_initialize_request(0));
        framed_requests.push(initialized_notification());
    }
    framed_requests.extend(requests);
    let input = framed_requests
        .iter()
        .map(frame)
        .collect::<Result<Vec<_>, _>>()?
        .concat();
    let mut responses = run_raw_with_options(input, options)?;
    if prepend_handshake && !responses.is_empty() {
        responses.remove(0);
    }
    Ok(responses)
}

#[cfg(feature = "mcp")]
fn run_raw(input: Vec<u8>) -> Result<Vec<JsonValue>, Box<dyn std::error::Error>> {
    run_raw_with_options(input, server_options())
}

fn run_raw_with_options(
    input: Vec<u8>,
    options: McpServerOptions,
) -> Result<Vec<JsonValue>, Box<dyn std::error::Error>> {
    Ok(sort_responses_by_id(parse_frames(
        &run_raw_output_with_options(input, options)?,
    )?))
}

fn run_raw_output_with_options(
    input: Vec<u8>,
    options: McpServerOptions,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = Arc::new(Mutex::new(Vec::new()));
    serve_mcp_json_rpc(
        Cursor::new(input),
        SharedTestOutput::new(Arc::clone(&output)),
        options,
    )?;
    output
        .lock()
        .map(|bytes| bytes.clone())
        .map_err(|_| "MCP test output lock failed".into())
}

#[cfg(feature = "mcp")]
fn skill_server_options() -> Result<McpServerOptions, Box<dyn std::error::Error>> {
    Ok(McpServerOptions::from_skill_paths_with_execution(
        &[repo_root()?.join("fixtures/skills/mcp-echo")],
        "runx-cli",
        "0.0.0",
        mcp_server_execution_options(None)?,
    )?)
}

#[cfg(feature = "mcp")]
fn skill_server_options_with_receipt_dir(
    receipt_dir: PathBuf,
) -> Result<McpServerOptions, Box<dyn std::error::Error>> {
    Ok(McpServerOptions::from_skill_paths_with_execution(
        &[repo_root()?.join("fixtures/skills/mcp-echo")],
        "runx-cli",
        "0.0.0",
        mcp_server_execution_options(Some(receipt_dir))?,
    )?)
}

#[cfg(feature = "mcp")]
fn approval_graph_server_options() -> Result<McpServerOptions, Box<dyn std::error::Error>> {
    Ok(McpServerOptions::from_skill_paths_with_execution(
        &[repo_root()?.join("fixtures/skills/mcp-approval-graph")],
        "runx-cli",
        "0.0.0",
        mcp_server_execution_options(None)?,
    )?)
}

#[cfg(feature = "mcp")]
fn mcp_server_execution_options(
    receipt_dir: Option<PathBuf>,
) -> Result<McpServerExecutionOptions, Box<dyn std::error::Error>> {
    let mut env = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
    env.insert(
        "RUNX_CWD".to_owned(),
        repo_root()?.to_string_lossy().into_owned(),
    );
    env.extend(crate::support::test_signing_env());
    Ok(McpServerExecutionOptions {
        runner: None,
        receipt_dir,
        env,
    })
}

#[cfg(feature = "mcp")]
fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

fn server_options() -> McpServerOptions {
    McpServerOptions {
        package_name: "runx-cli".to_owned(),
        package_version: "0.0.0".to_owned(),
        tools: vec![fixed_tool("echo")],
    }
}

fn fixed_tool(name: &str) -> McpServerTool {
    McpServerTool {
        name: name.to_owned(),
        description: "fixture tool".to_owned(),
        input_schema: [
            ("type".to_owned(), JsonValue::String("object".to_owned())),
            (
                "properties".to_owned(),
                JsonValue::Object(JsonObject::new()),
            ),
            ("required".to_owned(), JsonValue::Array(Vec::new())),
            ("additionalProperties".to_owned(), JsonValue::Bool(false)),
        ]
        .into(),
        required_scopes: Vec::new(),
        result: McpServerToolBehavior::Fixed(McpToolResult {
            content: vec![McpContent {
                text: "hello from server".to_owned(),
            }],
            structured_content: Some(runx_content("completed")),
            is_error: false,
        }),
    }
}

fn request(id: i64, method: &str, params: JsonObject) -> JsonValue {
    JsonValue::Object(
        [
            ("jsonrpc".to_owned(), JsonValue::String("2.0".to_owned())),
            (
                "id".to_owned(),
                JsonValue::Number(runx_contracts::JsonNumber::I64(id)),
            ),
            ("method".to_owned(), JsonValue::String(method.to_owned())),
            ("params".to_owned(), JsonValue::Object(params)),
        ]
        .into(),
    )
}

fn request_method(request: Option<&JsonValue>) -> Option<&str> {
    let JsonValue::Object(record) = request? else {
        return None;
    };
    match record.get("method") {
        Some(JsonValue::String(method)) => Some(method.as_str()),
        _ => None,
    }
}

fn rmcp_initialize_request(id: i64) -> JsonValue {
    request(
        id,
        "initialize",
        [
            (
                "protocolVersion".to_owned(),
                JsonValue::String("2025-06-18".to_owned()),
            ),
            (
                "capabilities".to_owned(),
                JsonValue::Object(JsonObject::new()),
            ),
            (
                "clientInfo".to_owned(),
                JsonValue::Object(
                    [
                        ("name".to_owned(), JsonValue::String("runx-test".to_owned())),
                        ("version".to_owned(), JsonValue::String("0.0.0".to_owned())),
                    ]
                    .into(),
                ),
            ),
        ]
        .into(),
    )
}

fn initialized_notification() -> JsonValue {
    JsonValue::Object(
        [
            ("jsonrpc".to_owned(), JsonValue::String("2.0".to_owned())),
            (
                "method".to_owned(),
                JsonValue::String("notifications/initialized".to_owned()),
            ),
            ("params".to_owned(), JsonValue::Object(JsonObject::new())),
        ]
        .into(),
    )
}

fn frame(message: &JsonValue) -> Result<Vec<u8>, serde_json::Error> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    Ok(line)
}

#[cfg(feature = "mcp")]
fn frame_jsonl_fixture(
    fixture_name: &str,
    kind: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = repo_root()?.join(format!(
        "fixtures/runtime/adapters/mcp/wire-contract/{fixture_name}.{kind}.jsonl"
    ));
    let mut framed = Vec::new();
    for line in std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.is_empty())
    {
        let _: JsonValue = serde_json::from_str(line)?;
        framed.extend(line.as_bytes());
        framed.push(b'\n');
    }
    Ok(framed)
}

fn parse_frames(bytes: &[u8]) -> Result<Vec<JsonValue>, Box<dyn std::error::Error>> {
    assert_newline_framing(bytes)?;
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(Into::into))
        .collect()
}

/// MCP stdio: every message is one line of compact JSON, newline terminated.
fn assert_newline_framing(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err("last MCP message is not newline terminated".into());
    }
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let _: JsonValue = serde_json::from_slice(line)?;
    }
    Ok(())
}

fn normalize_fixture_messages(messages: Vec<JsonValue>) -> Vec<JsonValue> {
    messages.into_iter().map(normalize_fixture_value).collect()
}

fn sort_responses_by_id(mut messages: Vec<JsonValue>) -> Vec<JsonValue> {
    messages.sort_by_key(response_sort_key);
    messages
}

fn response_sort_key(message: &JsonValue) -> i128 {
    response_id(message).map_or(i128::MAX, i128::from)
}

fn response_id(message: &JsonValue) -> Option<i64> {
    match path(message, &["id"]) {
        Some(JsonValue::Number(runx_contracts::JsonNumber::I64(value))) => Some(*value),
        Some(JsonValue::Number(runx_contracts::JsonNumber::U64(value))) => {
            i64::try_from(*value).ok()
        }
        _ => None,
    }
}

fn normalize_fixture_value(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Array(values) => {
            JsonValue::Array(values.into_iter().map(normalize_fixture_value).collect())
        }
        JsonValue::Object(record) => JsonValue::Object(
            record
                .into_iter()
                .filter_map(|(key, value)| {
                    if key == "isError" && value == JsonValue::Bool(false) {
                        None
                    } else {
                        Some((key, normalize_fixture_value(value)))
                    }
                })
                .collect(),
        ),
        value => value,
    }
}

fn path<'a>(value: &'a JsonValue, path: &[&str]) -> Option<&'a JsonValue> {
    let mut current = value;
    for segment in path {
        current = match current {
            JsonValue::Object(record) => record.get(*segment)?,
            JsonValue::Array(values) => values.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

#[cfg(feature = "mcp")]
fn assert_error(message: &JsonValue, code: i64, text: &str) {
    assert_eq!(
        path(message, &["error", "code"]),
        Some(&JsonValue::Number(runx_contracts::JsonNumber::I64(code)))
    );
    assert_eq!(
        path(message, &["error", "message"]),
        Some(&JsonValue::String(text.to_owned()))
    );
}

#[cfg(feature = "mcp")]
fn assert_no_json_rpc_error(message: &JsonValue) {
    assert_eq!(
        path(message, &["error"]),
        None,
        "unexpected JSON-RPC error: {message:?}"
    );
}

#[cfg(feature = "mcp")]
fn assert_result_not_error(message: &JsonValue) {
    assert!(
        matches!(
            path(message, &["result", "isError"]),
            None | Some(JsonValue::Bool(false))
        ),
        "unexpected MCP tool error result: {message:?}"
    );
}

fn runx_status(status: &str) -> JsonObject {
    [("status".to_owned(), JsonValue::String(status.to_owned()))].into()
}

fn runx_content(status: &str) -> JsonObject {
    [("runx".to_owned(), JsonValue::Object(runx_status(status)))].into()
}

#[derive(Clone)]
struct SharedTestOutput {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl SharedTestOutput {
    fn new(bytes: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { bytes }
    }
}

impl std::io::Write for SharedTestOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut bytes = self
            .bytes
            .lock()
            .map_err(|_| std::io::Error::other("MCP test output lock failed"))?;
        bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
