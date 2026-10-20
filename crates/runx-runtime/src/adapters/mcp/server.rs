// Module rationale: the JSON-RPC dispatch loop, server
// state, tool-result builders, and host-result projections for `runx mcp
// serve` all sit on the same protocol surface.
#[cfg(test)]
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::thread;

use runx_contracts::{JsonObject, JsonValue};
use tokio::sync::mpsc;

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
    let JsonValue::Object(runx) = result.output else {
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
        match runx.get("status").and_then(JsonValue::as_str) {
            Some("needs_approval") => format!(
                "{skill_name} needs a person's approval at {run_id}: {} request(s). The person approves with: runx resume {run_id} <approvals.json|->{receipt_option}.",
                result.pending_requests.len()
            ),
            _ => format!(
                "{skill_name} needs agent work at {run_id}: {} request(s). Work its open turns with turn_list, turn_status, turn_read, turn_call and turn_complete for run {run_id}.",
                result.pending_requests.len()
            ),
        }
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
    fn get_info(&self) -> rmcp::model::ServerConfig {
        let (package_name, package_version) = (
            self.state.options.package_name.clone(),
            self.state.options.package_version.clone(),
        );
        let info = rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_protocol_version(rmcp::model::ProtocolVersion::V_2025_06_18)
        .with_server_info(rmcp::model::Implementation::new(
            package_name,
            package_version,
        ));
        match &self.state.options.instructions {
            Some(instructions) => info.with_instructions(instructions.clone()),
            None => info,
        }
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>> + Send + '_
    {
        // From 2026-07-28 a list says how long it may be cached, and by whom.
        // A turn's tools change as it loads skills and connectors, and are
        // this client's alone: nothing caches them.
        let states_cache = context.protocol_version().is_some_and(|version| {
            version.as_str() >= rmcp::model::ProtocolVersion::V_2026_07_28.as_str()
        });
        let result = self
            .state
            .options
            .tools
            .iter()
            .map(rmcp_tool_from_server_tool)
            .collect::<Result<Vec<_>, _>>()
            .map(|tools| {
                let result = rmcp::model::ListToolsResult::with_all_items(tools);
                if states_cache {
                    result
                        .with_ttl_ms(0)
                        .with_cache_scope(rmcp::model::CacheScope::Private)
                } else {
                    result
                }
            });
        std::future::ready(result)
    }

    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::CallToolResponse, rmcp::ErrorData>> + Send + '_
    {
        let prepared = (|| {
            let arguments = match request.arguments {
                Some(arguments) => runx_json_object(arguments).map_err(rmcp_invalid_params)?,
                None => JsonObject::new(),
            };
            prepare_rmcp_tool_call(&self.state, &request.name, arguments)
        })();
        execute_rmcp_tool_call(prepared, context)
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
        effects: crate::effects::RuntimeEffectRegistry,
    },
    Turn {
        tool: super::types::McpTurnTool,
        arguments: JsonObject,
        /// The run-id prefix of the runs this server's own calls opened.
        served: String,
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
        McpServerToolBehavior::Skill(execution) => Ok(PreparedMcpToolCall::Skill {
            run_id: state.next_run_id(&execution.skill_name),
            execution,
            arguments,
            javascript: state.javascript.clone(),
            effects: state.options.effects.clone(),
        }),
        McpServerToolBehavior::Turn(tool) => Ok(PreparedMcpToolCall::Turn {
            tool,
            arguments,
            served: state.served_run_prefix(),
        }),
    }
}

async fn execute_rmcp_tool_call(
    prepared: Result<PreparedMcpToolCall, rmcp::ErrorData>,
    context: rmcp::service::RequestContext<rmcp::RoleServer>,
) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
    match prepared? {
        PreparedMcpToolCall::Fixed(result) => rmcp_call_tool_result(result).map(Into::into),
        // The host's cancellation (`notifications/cancelled`) stops the call
        // gracefully: the run admits no further step or call, a command it
        // supervises is terminated, and the call returns what it recorded.
        PreparedMcpToolCall::Turn {
            tool,
            arguments,
            served,
        } => {
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let watch = tokio::spawn({
                let stop = Arc::clone(&stop);
                async move {
                    context.ct.cancelled().await;
                    stop.store(true, std::sync::atomic::Ordering::Release);
                }
            });
            let result = off_runtime(move || {
                crate::interrupt::while_working(&stop, || {
                    super::server_turn::execute_mcp_turn_tool(&tool, arguments, &served)
                })
            })
            .await;
            watch.abort();
            rmcp_call_tool_result(result?).map(Into::into)
        }
        PreparedMcpToolCall::Skill {
            run_id,
            execution,
            arguments,
            javascript,
            effects,
        } => {
            let result = off_runtime(move || {
                execute_mcp_server_skill(&run_id, *execution, arguments, javascript, effects)
            })
            .await?;
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

/// Run a tool call's kernel work on a thread of its own. The kernel blocks on
/// runx's HTTP and MCP client runtimes, which refuse to block inside another
/// runtime's context, and a blocking-pool task runs inside this server's. So
/// a connector operation a turn or skill call makes would fail there.
async fn off_runtime<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, rmcp::ErrorData> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    thread::spawn(move || {
        let _closed = sender.send(work());
    });
    receiver
        .await
        .map_err(|_| rmcp_internal_error("MCP tool task failed"))
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

    /// Every run this server's skill calls open carries this prefix.
    pub(super) fn served_run_prefix(&self) -> String {
        format!("rx_mcp_{}_", self.session_id)
    }

    pub(super) fn next_run_id(&self, skill_name: &str) -> String {
        let sequence = self
            .next_run_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            })
            .map_or(u64::MAX, |previous| previous.saturating_add(1));
        format!(
            "{}{}_{}",
            self.served_run_prefix(),
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
                "runx mcp received duplicate tool name '{}'. Serve unique skill names only.",
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

    /// A tool call's kernel work runs outside the server's runtime, where
    /// runx's own HTTP and MCP clients can block on theirs.
    #[test]
    fn tool_calls_run_outside_the_server_runtime() -> Result<(), Box<dyn std::error::Error>> {
        let outside = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()?
            .block_on(off_runtime(|| {
                tokio::runtime::Handle::try_current().is_err()
            }))
            .map_err(|error| error.message.to_string())?;
        assert!(outside);
        Ok(())
    }

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
                state.options.effects.clone(),
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

    /// A skill server works the turn its own skill call opened, on the same
    /// connection, until the run seals.
    #[test]
    fn a_skill_server_works_the_turn_its_own_skill_call_opened()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let options = McpServerOptions::from_skill_paths_with_execution(
            &[std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../fixtures/skills/agent-task"
            ))],
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
        let tool = |name: &str| {
            state
                .options
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .map(|tool| tool.result.clone())
        };
        let Some(McpServerToolBehavior::Skill(execution)) = tool("reviewer-boundary") else {
            return Err("expected the skill tool".into());
        };
        let started = execute_mcp_server_skill(
            &state.next_run_id("reviewer-boundary"),
            *execution,
            [("prompt".to_owned(), JsonValue::String("review".to_owned()))].into(),
            state.javascript.clone(),
            state.options.effects.clone(),
        )?;
        let runx = |result: &McpToolResult| {
            result
                .structured_content
                .as_ref()
                .and_then(|content| content.get("runx"))
                .and_then(JsonValue::as_object)
                .cloned()
                .unwrap_or_default()
        };
        let run_id = runx(&started)
            .get("run_id")
            .and_then(JsonValue::as_str)
            .ok_or("the paused call names its run")?
            .to_owned();
        let Some(McpServerToolBehavior::Turn(complete)) = tool("turn_complete") else {
            return Err("expected the turn_complete tool".into());
        };
        // A turn ends with its output or a failure, never both; a call's id
        // is the host's to pin or leave out.
        let both = super::super::server_turn::execute_mcp_turn_tool(
            &complete,
            [
                ("run".to_owned(), JsonValue::String(run_id.clone())),
                ("turn".to_owned(), JsonValue::String("default".to_owned())),
                ("output".to_owned(), JsonValue::Object(JsonObject::new())),
                (
                    "failure".to_owned(),
                    JsonValue::Object(
                        [("reason".to_owned(), JsonValue::String("no".to_owned()))].into(),
                    ),
                ),
            ]
            .into(),
            &state.served_run_prefix(),
        );
        assert!(both.is_error, "{both:?}");
        let call_schema = state
            .options
            .tools
            .iter()
            .find(|tool| tool.name == "turn_call")
            .map(|tool| tool.input_schema.clone())
            .ok_or("expected the turn_call tool")?;
        assert!(
            call_schema
                .get("properties")
                .and_then(JsonValue::as_object)
                .is_some_and(|properties| properties.contains_key("call_id"))
        );
        assert!(
            !call_schema
                .get("required")
                .and_then(JsonValue::as_array)
                .is_some_and(|required| required.contains(&JsonValue::String("call_id".to_owned())))
        );
        let completed = super::super::server_turn::execute_mcp_turn_tool(
            &complete,
            [
                ("run".to_owned(), JsonValue::String(run_id)),
                ("turn".to_owned(), JsonValue::String("default".to_owned())),
                (
                    "output".to_owned(),
                    JsonValue::Object(
                        [("verdict".to_owned(), JsonValue::String("pass".to_owned()))].into(),
                    ),
                ),
            ]
            .into(),
            &state.served_run_prefix(),
        );
        assert!(!completed.is_error, "{completed:?}");
        assert_eq!(
            runx(&completed).get("status"),
            Some(&JsonValue::String("sealed".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn next_run_id_is_unique_under_concurrent_allocation() -> Result<(), String> {
        let state = Arc::new(
            McpServerState::new(McpServerOptions {
                package_name: "runx-test".to_owned(),
                package_version: "0.0.0".to_owned(),
                instructions: None,
                effects: Default::default(),
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
}
