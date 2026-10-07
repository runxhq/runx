//! The `runx mcp serve` server: runx skills served as MCP tools.
//!
//! - `types`: server options, tools, and results.
//! - `stdio_transport`: MCP's bounded newline-delimited stdio framing, shared
//!   with the connector driver's stdio client.
//! - `server`: `serve_mcp_json_rpc` and host-result projections.
//! - `server_skill`: server-side skill and graph execution.
//! - `http_server`: the server over streamable HTTP.

#[cfg(feature = "mcp-http-server")]
mod http_server;
mod server;
mod server_skill;
mod stdio_transport;
mod types;

#[cfg(feature = "mcp-http-server")]
pub use http_server::{
    DEFAULT_MCP_HTTP_LISTEN_ADDR, McpHttpServerSecurity, generate_mcp_http_bearer_token,
    serve_mcp_http_server, serve_mcp_http_server_blocking,
};
pub use server::{mcp_tool_result_from_run_result, serve_mcp_json_rpc};
#[cfg(feature = "async-http")]
pub(crate) use stdio_transport::{StdioJsonRpcTransport, TransportErrorState};
pub use types::{
    McpContent, McpServerError, McpServerExecutionOptions, McpServerOptions,
    McpServerSkillExecution, McpServerTool, McpServerToolBehavior, McpToolResult,
};
