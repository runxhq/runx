// Module rationale: the JSON-RPC dispatch loop, server
// state, tool-result builders, and host-result projections for `runx mcp
// serve` all sit on the same protocol surface.
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::thread;

use runx_contracts::{JsonObject, JsonValue};
use runx_core::policy::{ScopeGrantPolicy, missing_granted_scopes};
use tokio::sync::mpsc;

use crate::kernel::instances::{
    INJECTED_GRANT_ID_ENV, INJECTED_GRANTED_SCOPES_ENV, decode_injected_scopes,
};

use super::server_skill::{execute_mcp_server_skill, identifier_segment};
use super::stdio_transport::{StdioJsonRpcTransport, TransportErrorState};
use super::types::{
    McpContent, McpServerError, McpServerOptions, McpServerSkillExecution, McpServerTool,
    McpServerToolBehavior, McpToolResult,
};

const MAX_SERVER_REQUEST_BYTES: usize = 4 * 1024 * 1024;

pub fn serve_mcp_json_rpc(
    input: impl Read + Send + Unpin + 'static,
    output: impl Write + Send + Unpin + 'static,
    options: McpServerOptions,
) -> Result<(), McpServerError> {
    assert_unique_server_tool_names(&options.tools)?;
    serve_mcp_json_rpc_with_rmcp(input, output, options)
}

/// Project the canonical execution result onto MCP's content envelope.
/// No execution decision or receipt construction belongs in this transport.
pub fn mcp_tool_result_from_run_result(
    result: crate::execution::orchestrator::RunResult,
    receipt_dir: Option<&std::path::Path>,
) -> McpToolResult {
    let succeeded = result.succeeded();
    let pending = result.needs_resolution();
    let JsonValue::Object(mut runx) = result.output else {
        return McpToolResult {
            content: vec![McpContent {
                text: "Invalid native run result.".to_owned(),
            }],
            structured_content: None,
            is_error: true,
        };
    };
    let skill_name = runx
        .get("skill_name")
        .and_then(JsonValue::as_str)
        .unwrap_or("skill");
    let run_id = runx.get("run_id").and_then(JsonValue::as_str).unwrap_or("");
    let output = succeeded.then(|| runx.get("result").cloned().unwrap_or(JsonValue::Null));
    let text = if pending && !result.pending_requests.is_empty() {
        let receipt_option = receipt_dir
            .map(|path| {
                // Shell quoting keeps paths containing whitespace or apostrophes usable.
                format!(
                    " --receipt-dir '{}'",
                    path.to_string_lossy().replace('\'', "'\"'\"'")
                )
            })
            .unwrap_or_default();
        format!(
            "{skill_name} needs agent input at {run_id}. Resolve {} request(s), write answers.json, then run: runx resume {run_id} answers.json{receipt_option}.",
            result.pending_requests.len()
        )
    } else if pending {
        format!(
            "{skill_name} is deferred at {run_id}. Inspect receipt {} and reconcile provider state before retrying.",
            result.receipt_refs.first().map_or("n/a", String::as_str)
        )
    } else if let Some(output) = &output {
        match output {
            JsonValue::String(value) if !value.trim().is_empty() => value.clone(),
            value => serde_json::to_string(value).unwrap_or_default(),
        }
    } else {
        format!(
            "{skill_name} ended with {}. Inspect receipt {}.",
            result.disposition.map_or("invalid", |value| value.label()),
            result.receipt_refs.first().map_or("n/a", String::as_str)
        )
    };
    if pending && !result.pending_requests.is_empty() {
        let digests = result
            .pending_requests
            .iter()
            .filter_map(|request| {
                let id = request.as_object()?.get("id")?.as_str()?;
                let bytes = serde_json::to_vec(request).ok()?;
                Some((
                    id.to_owned(),
                    JsonValue::String(runx_contracts::sha256_prefixed(&bytes)),
                ))
            })
            .collect();
        runx.insert("request_digests".to_owned(), JsonValue::Object(digests));
    }
    mcp_host_tool_result(text, runx, output, !succeeded && !pending)
}

fn mcp_host_tool_result(
    text: String,
    runx: JsonObject,
    output: Option<JsonValue>,
    is_error: bool,
) -> McpToolResult {
    McpToolResult {
        content: vec![McpContent { text }],
        structured_content: Some(runx_content(runx, output)),
        is_error,
    }
}

fn runx_content(runx: JsonObject, output: Option<JsonValue>) -> JsonObject {
    let mut content = JsonObject::from([("runx".to_owned(), JsonValue::Object(runx))]);
    if let Some(output) = output {
        content.insert("output".to_owned(), output);
    }
    content
}

fn serve_mcp_json_rpc_with_rmcp(
    input: impl Read + Send + Unpin + 'static,
    output: impl Write + Send + Unpin + 'static,
    options: McpServerOptions,
) -> Result<(), McpServerError> {
    block_on_rmcp_server(input, output, options)
}

fn block_on_rmcp_server<R, W>(
    input: R,
    output: W,
    options: McpServerOptions,
) -> Result<(), McpServerError>
where
    R: Read + Send + Unpin + 'static,
    W: Write + Send + Unpin + 'static,
{
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| {
            McpServerError::new(format!("MCP server runtime initialization failed: {error}"))
        })?
        .block_on(serve_mcp_json_rpc_with_rmcp_async(input, output, options))
}

async fn serve_mcp_json_rpc_with_rmcp_async<R, W>(
    input: R,
    output: W,
    options: McpServerOptions,
) -> Result<(), McpServerError>
where
    R: Read + Send + Unpin + 'static,
    W: Write + Send + Unpin + 'static,
{
    let error_state = TransportErrorState::default();
    let transport = StdioJsonRpcTransport::new(
        ChannelAsyncRead::spawn(input),
        BlockingAsyncWrite::new(output),
        MAX_SERVER_REQUEST_BYTES,
        error_state.clone(),
    );
    let service = RmcpProofServer::from_options(options)?;
    let running = rmcp::serve_server(service, transport)
        .await
        .map_err(|error| {
            McpServerError::new(format!(
                "MCP rmcp server initialization failed: {}",
                error_state.take().unwrap_or_else(|| error.to_string())
            ))
        })?;
    let wait_result = running.waiting().await;
    if let Some(message) = error_state.take() {
        return Err(McpServerError::new(format!(
            "MCP rmcp server task failed: {message}"
        )));
    }
    wait_result
        .map(|_reason| ())
        .map_err(|error| McpServerError::new(format!("MCP rmcp server task failed: {error}")))
}

pub(super) struct RmcpProofServer {
    state: McpServerState,
}

impl RmcpProofServer {
    /// Build a fresh governed server from options. Used by both the stdio path
    /// and the streamable-HTTP service factory.
    pub(super) fn from_options(options: McpServerOptions) -> Result<Self, McpServerError> {
        Ok(Self {
            state: McpServerState::new(options)?,
        })
    }
}

impl rmcp::ServerHandler for RmcpProofServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        let (package_name, package_version) = (
            self.state.options.package_name.clone(),
            self.state.options.package_version.clone(),
        );
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_protocol_version(rmcp::model::ProtocolVersion::V_2025_06_18)
        .with_server_info(rmcp::model::Implementation::new(
            package_name,
            package_version,
        ))
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>> + Send + '_
    {
        let result = self
            .state
            .options
            .tools
            .iter()
            .map(rmcp_tool_from_server_tool)
            .collect::<Result<Vec<_>, _>>()
            .map(rmcp::model::ListToolsResult::with_all_items);
        std::future::ready(result)
    }

    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::CallToolResponse, rmcp::ErrorData>> + Send + '_
    {
        let prepared = (|| {
            let arguments = match request.arguments {
                Some(arguments) => runx_json_object(arguments).map_err(rmcp_invalid_params)?,
                None => JsonObject::new(),
            };
            prepare_rmcp_tool_call(&self.state, &request.name, arguments)
        })();
        execute_rmcp_tool_call(prepared)
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.state
            .options
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .cloned()
            .and_then(|tool| rmcp_tool_from_server_tool(&tool).ok())
    }
}

enum PreparedMcpToolCall {
    Fixed(McpToolResult),
    Skill {
        run_id: String,
        execution: Box<McpServerSkillExecution>,
        arguments: JsonObject,
        javascript: crate::adapters::javascript::JavaScriptAdapter,
    },
}

fn prepare_rmcp_tool_call(
    state: &McpServerState,
    name: &str,
    arguments: JsonObject,
) -> Result<PreparedMcpToolCall, rmcp::ErrorData> {
    let Some(tool) = state
        .options
        .tools
        .iter()
        .find(|tool| tool.name == name)
        .cloned()
    else {
        return Err(rmcp::ErrorData::new(
            rmcp::model::ErrorCode::METHOD_NOT_FOUND,
            format!("tool not found: {name}"),
            None,
        ));
    };
    match tool.result {
        McpServerToolBehavior::Fixed(result) => Ok(PreparedMcpToolCall::Fixed(result)),
        McpServerToolBehavior::Skill(execution) => {
            admit_mcp_tool_scopes(&tool.name, &tool.required_scopes, &execution.env)?;
            Ok(PreparedMcpToolCall::Skill {
                run_id: state.next_run_id(&execution.skill_name),
                execution,
                arguments,
                javascript: state.javascript.clone(),
            })
        }
    }
}

fn admit_mcp_tool_scopes(
    tool_name: &str,
    required_scopes: &[String],
    env: &BTreeMap<String, String>,
) -> Result<(), rmcp::ErrorData> {
    if required_scopes.is_empty() {
        return Ok(());
    }
    let Some(grant_id) = env
        .get(INJECTED_GRANT_ID_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Err(rmcp_invalid_params(format!(
            "MCP tool '{tool_name}' requires scopes [{}], but no operator provider grant id was supplied in {INJECTED_GRANT_ID_ENV}",
            required_scopes.join(", ")
        )));
    };
    let granted_scopes = env
        .get(INJECTED_GRANTED_SCOPES_ENV)
        .map(|value| {
            decode_injected_scopes(value).ok_or_else(|| {
                rmcp_invalid_params(format!(
                    "{INJECTED_GRANTED_SCOPES_ENV} must be a JSON array of non-blank scopes"
                ))
            })
        })
        .transpose()?
        .unwrap_or_default();
    let missing = missing_granted_scopes(
        required_scopes,
        &granted_scopes,
        ScopeGrantPolicy::Delegated,
    );
    if missing.is_empty() {
        return Ok(());
    }
    Err(rmcp_invalid_params(format!(
        "MCP tool '{tool_name}' requires scopes [{}], but operator grant '{}' only provides [{}]",
        required_scopes.join(", "),
        grant_id,
        granted_scopes.join(", ")
    )))
}

async fn execute_rmcp_tool_call(
    prepared: Result<PreparedMcpToolCall, rmcp::ErrorData>,
) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
    match prepared? {
        PreparedMcpToolCall::Fixed(result) => rmcp_call_tool_result(result).map(Into::into),
        PreparedMcpToolCall::Skill {
            run_id,
            execution,
            arguments,
            javascript,
        } => {
            let result = tokio::task::spawn_blocking(move || {
                execute_mcp_server_skill(&run_id, *execution, arguments, javascript)
            })
            .await
            .map_err(|error| rmcp_internal_error(format!("MCP tool task failed: {error}")))?;
            match result {
                Ok(result) => rmcp_call_tool_result(result).map(Into::into),
                Err(error) => Err(rmcp_internal_error(error.to_string())),
            }
        }
    }
}

fn rmcp_tool_from_server_tool(tool: &McpServerTool) -> Result<rmcp::model::Tool, rmcp::ErrorData> {
    Ok(rmcp::model::Tool::new(
        tool.name.clone(),
        tool.description.clone(),
        Arc::new(rmcp_json_object(tool.input_schema.clone())?),
    ))
}

fn rmcp_call_tool_result(
    result: McpToolResult,
) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
    let content = result
        .content
        .into_iter()
        .map(|entry| rmcp::model::ContentBlock::text(entry.text))
        .collect();
    let mut call_result = if result.is_error {
        rmcp::model::CallToolResult::error(content)
    } else {
        rmcp::model::CallToolResult::success(content)
    };
    call_result.structured_content = result
        .structured_content
        .map(|content| serde_json::to_value(content).map_err(rmcp_internal_error))
        .transpose()?;
    Ok(call_result)
}

fn rmcp_json_object(value: JsonObject) -> Result<rmcp::model::JsonObject, rmcp::ErrorData> {
    serde_json::to_value(JsonValue::Object(value))
        .map_err(rmcp_internal_error)?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            rmcp_internal_error("MCP tool input schema did not serialize to a JSON object.")
        })
}

fn runx_json_object(value: rmcp::model::JsonObject) -> Result<JsonObject, serde_json::Error> {
    serde_json::to_vec(&value).and_then(|bytes| serde_json::from_slice(&bytes))
}

fn rmcp_invalid_params(error: impl std::fmt::Display) -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params(error.to_string(), None)
}

fn rmcp_internal_error(error: impl std::fmt::Display) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(error.to_string(), None)
}

struct ChannelAsyncRead {
    receiver: mpsc::Receiver<Result<Vec<u8>, std::io::Error>>,
    pending: Vec<u8>,
    offset: usize,
}

impl ChannelAsyncRead {
    fn spawn<R>(mut input: R) -> Self
    where
        R: Read + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(8);
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match input.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(read) => {
                        if sender.blocking_send(Ok(buffer[..read].to_vec())).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ignored = sender.blocking_send(Err(error));
                        return;
                    }
                }
            }
        });
        Self {
            receiver,
            pending: Vec::new(),
            offset: 0,
        }
    }

    fn copy_pending(&mut self, buf: &mut tokio::io::ReadBuf<'_>) -> bool {
        if self.offset >= self.pending.len() {
            return false;
        }
        let remaining = self.pending.len() - self.offset;
        let copied = remaining.min(buf.remaining());
        buf.put_slice(&self.pending[self.offset..self.offset + copied]);
        self.offset += copied;
        if self.offset >= self.pending.len() {
            self.pending.clear();
            self.offset = 0;
        }
        true
    }
}

impl tokio::io::AsyncRead for ChannelAsyncRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        loop {
            if self.copy_pending(buf) {
                return Poll::Ready(Ok(()));
            }
            match self.receiver.poll_recv(cx) {
                Poll::Ready(Some(Ok(bytes))) if bytes.is_empty() => continue,
                Poll::Ready(Some(Ok(bytes))) => {
                    self.pending = bytes;
                    self.offset = 0;
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct BlockingAsyncWrite<W> {
    inner: W,
}

impl<W> BlockingAsyncWrite<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W> tokio::io::AsyncWrite for BlockingAsyncWrite<W>
where
    W: Write + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Poll::Ready(self.inner.write(buf))
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(self.inner.flush())
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(self.inner.flush())
    }
}

#[derive(Debug)]
pub(super) struct McpServerState {
    options: McpServerOptions,
    session_id: String,
    next_run_sequence: AtomicU64,
    javascript: crate::adapters::javascript::JavaScriptAdapter,
}

impl McpServerState {
    fn new(options: McpServerOptions) -> Result<Self, McpServerError> {
        use ring::rand::{SecureRandom, SystemRandom};
        let mut namespace = [0_u8; 16];
        SystemRandom::new()
            .fill(&mut namespace)
            .map_err(|_| McpServerError::new("MCP session identity generation failed"))?;
        Ok(Self {
            options,
            session_id: runx_contracts::hex_lower(&namespace),
            next_run_sequence: AtomicU64::new(0),
            javascript: crate::adapters::javascript::JavaScriptAdapter::with_max_concurrency(
                runx_contracts::javascript_worker::MAX_WORKER_POOL_SIZE,
            ),
        })
    }

    pub(super) fn next_run_id(&self, skill_name: &str) -> String {
        let sequence = self
            .next_run_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            })
            .map_or(u64::MAX, |previous| previous.saturating_add(1));
        format!(
            "rx_mcp_{}_{}_{}",
            self.session_id,
            identifier_segment(skill_name),
            sequence
        )
    }
}

fn assert_unique_server_tool_names(tools: &[McpServerTool]) -> Result<(), McpServerError> {
    let mut seen = std::collections::BTreeSet::new();
    for tool in tools {
        if !seen.insert(tool.name.as_str()) {
            return Err(McpServerError::new(format!(
                "runx mcp serve received duplicate tool name '{}'. Serve unique skill names only.",
                tool.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn canonical_skill_calls_share_the_server_worker_without_sharing_javascript_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let skill = root.path().join("echo");
        std::fs::create_dir_all(&skill)?;
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: echo\ndescription: echo fixture.\n---\n# Echo\n",
        )?;
        std::fs::write(
            skill.join("run.yaml"),
            "skill: echo\nrunners:\n  main:\n    default: true\n    inputs:\n      value: { type: integer, required: true }\n    steps:\n      - id: main\n        tool: { module: echo.mjs }\n        inputs:\n          value: $input.value\n        outputs: { value: integer, prior: integer }\n",
        )?;
        std::fs::write(
            skill.join("echo.mjs"),
            "export default ({value}) => { const prior = globalThis.prior ?? 0; globalThis.prior = value; return {value, prior}; };\n",
        )?;
        let options = McpServerOptions::from_skill_paths_with_execution(
            &[skill],
            "runx-test",
            "0.0.0",
            super::super::types::McpServerExecutionOptions {
                receipt_dir: Some(root.path().join("receipts")),
                env: [
                    ("RUNX_CWD".to_owned(), root.path().display().to_string()),
                    (
                        "RUNX_HOME".to_owned(),
                        root.path().join("home").display().to_string(),
                    ),
                ]
                .into(),
                ..Default::default()
            },
        )?;
        let state = McpServerState::new(options)?;
        let McpServerToolBehavior::Skill(execution) = &state.options.tools[0].result else {
            return Err("expected executable skill".into());
        };
        for value in 1..=5 {
            let result = execute_mcp_server_skill(
                &state.next_run_id("echo"),
                *execution.clone(),
                [(
                    "value".to_owned(),
                    JsonValue::Number(runx_contracts::JsonNumber::I64(value)),
                )]
                .into(),
                state.javascript.clone(),
            )?;
            assert!(!result.is_error, "{result:?}");
            let content = result.structured_content.ok_or("missing native result")?;
            assert_eq!(
                content.get("output"),
                Some(&JsonValue::Object(
                    [
                        (
                            "value".to_owned(),
                            JsonValue::Number(runx_contracts::JsonNumber::I64(value))
                        ),
                        (
                            "prior".to_owned(),
                            JsonValue::Number(runx_contracts::JsonNumber::I64(0))
                        ),
                    ]
                    .into()
                ))
            );
        }
        assert_eq!(state.javascript.session_stats().spawned_process_count, 1);
        Ok(())
    }

    #[test]
    fn next_run_id_is_unique_under_concurrent_allocation() -> Result<(), String> {
        let state = Arc::new(
            McpServerState::new(McpServerOptions {
                package_name: "runx-test".to_owned(),
                package_version: "0.0.0".to_owned(),
                tools: Vec::new(),
            })
            .map_err(|error| error.to_string())?,
        );
        let worker_count = 8;
        let ids_per_worker = 64;
        let barrier = Arc::new(Barrier::new(worker_count));

        let handles = (0..worker_count)
            .map(|_| {
                let state = Arc::clone(&state);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (0..ids_per_worker)
                        .map(|_| state.next_run_id("skill.alpha"))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();

        let mut ids = Vec::new();
        for handle in handles {
            let worker_ids = handle
                .join()
                .map_err(|_| "run-id worker panicked".to_owned())?;
            ids.extend(worker_ids);
        }
        let ids = ids.into_iter().collect::<BTreeSet<_>>();

        assert_eq!(ids.len(), worker_count * ids_per_worker);
        for sequence in 1..=(worker_count * ids_per_worker) {
            assert!(ids.contains(&format!(
                "rx_mcp_{}_skill_alpha_{sequence}",
                state.session_id
            )));
        }
        Ok(())
    }

    #[test]
    fn scoped_mcp_tool_requires_operator_grant_before_dispatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let required_scopes = vec!["repo.read".to_owned(), "issues.write".to_owned()];
        let result = admit_mcp_tool_scopes("github-write", &required_scopes, &BTreeMap::new());
        assert!(
            matches!(&result, Err(error) if error.message.contains("no operator provider grant id")),
            "scoped MCP tool must deny without operator grant id; got: {result:?}"
        );

        let mut env = BTreeMap::from([
            (INJECTED_GRANT_ID_ENV.to_owned(), "grant-gh".to_owned()),
            (
                INJECTED_GRANTED_SCOPES_ENV.to_owned(),
                crate::encode_injected_scopes(&["repo.read".to_owned()]).ok_or("scopes")?,
            ),
        ]);
        let result = admit_mcp_tool_scopes("github-write", &required_scopes, &env);
        assert!(
            matches!(&result, Err(error) if error.message.contains("issues.write")),
            "scoped MCP tool must deny missing granted scopes before dispatch; got: {result:?}"
        );

        env.insert(
            INJECTED_GRANTED_SCOPES_ENV.to_owned(),
            crate::encode_injected_scopes(&["repo.read".to_owned(), "issues.write".to_owned()])
                .ok_or("scopes")?,
        );
        let result = admit_mcp_tool_scopes("github-write", &required_scopes, &env);
        assert!(
            result.is_ok(),
            "matching operator grant scopes should admit the MCP tool; got: {result:?}"
        );

        env.insert(
            INJECTED_GRANTED_SCOPES_ENV.to_owned(),
            crate::encode_injected_scopes(&["repo:*".to_owned()]).ok_or("scopes")?,
        );
        assert!(admit_mcp_tool_scopes("github-read", &["repo:read".to_owned()], &env).is_ok());
        assert!(
            admit_mcp_tool_scopes("github-admin", &["repo:admin:keys".to_owned()], &env).is_err()
        );
        Ok(())
    }
}
