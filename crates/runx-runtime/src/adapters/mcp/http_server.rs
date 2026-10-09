// Module rationale: streamable HTTP serving keeps bearer auth,
// loopback binding, hyper service adaptation, and transport tests in one module
// while the MCP HTTP front is still a single gated feature.
//! Expose the governed MCP server over streamable HTTP/SSE.
//!
//! rmcp's [`StreamableHttpService`] is a `tower` service that carries the same
//! governed [`RmcpProofServer`] the stdio path uses; this drives it over a TCP
//! listener with hyper. Governance (admission, grants, receipt sealing) lives in
//! the server, not the transport, so the HTTP surface seals exactly like the stdio
//! surface. Each session gets a fresh governed server from the same options.
use std::convert::Infallible;
use std::future::Future;
use std::net::{SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use ring::rand::{SecureRandom, SystemRandom};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tower_service::Service;

use super::server::RmcpProofServer;
use super::types::{McpServerError, McpServerOptions};

pub const DEFAULT_MCP_HTTP_LISTEN_ADDR: &str = "127.0.0.1:8080";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpHttpServerSecurity {
    pub bearer_token: String,
    pub allow_non_loopback: bool,
}

impl McpHttpServerSecurity {
    #[must_use]
    pub fn loopback_only(bearer_token: String) -> Self {
        Self {
            bearer_token,
            allow_non_loopback: false,
        }
    }
}

pub fn generate_mcp_http_bearer_token() -> Result<String, McpServerError> {
    let rng = SystemRandom::new();
    let mut token = [0_u8; 32];
    rng.fill(&mut token)
        .map_err(|_error| McpServerError::new("MCP HTTP bearer token generation failed."))?;
    Ok(runx_contracts::hex_lower(&token))
}

/// Blocking entry point for the CLI: build a runtime and serve until exit.
/// Mirrors the stdio server's `block_on_rmcp_server`.
pub fn serve_mcp_http_server_blocking(
    listen_addr: &str,
    options: McpServerOptions,
    security: McpHttpServerSecurity,
) -> Result<(), McpServerError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| {
            McpServerError::new(format!(
                "MCP HTTP server runtime initialization failed: {error}"
            ))
        })?
        .block_on(serve_mcp_http_server(listen_addr, options, security))
}

/// Serve the governed MCP server over streamable HTTP at `listen_addr` until the
/// process exits (the accept loop only returns on a listener error).
pub async fn serve_mcp_http_server(
    listen_addr: &str,
    options: McpServerOptions,
    security: McpHttpServerSecurity,
) -> Result<(), McpServerError> {
    let bind_addr = checked_listen_addr(listen_addr, security.allow_non_loopback)?;
    let listener = TcpListener::bind(bind_addr).await.map_err(|error| {
        McpServerError::new(format!("MCP HTTP bind {listen_addr} failed: {error}"))
    })?;
    serve_mcp_http_listener(listener, options, security).await
}

/// Drive the streamable-HTTP service over an already-bound listener. Split from
/// [`serve_mcp_http_server`] so tests can bind an ephemeral port and read it back.
pub(crate) async fn serve_mcp_http_listener(
    listener: TcpListener,
    options: McpServerOptions,
    security: McpHttpServerSecurity,
) -> Result<(), McpServerError> {
    validate_http_security(&security)?;
    if !security.allow_non_loopback {
        let local_addr = listener
            .local_addr()
            .map_err(|error| McpServerError::new(format!("MCP HTTP local addr failed: {error}")))?;
        if !local_addr.ip().is_loopback() {
            return Err(McpServerError::new(format!(
                "MCP HTTP listen address {local_addr} is not loopback; pass --http-allow-non-loopback to opt in."
            )));
        }
    }
    let service = StreamableHttpService::new(
        move || RmcpProofServer::from_options(options.clone()).map_err(std::io::Error::other),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let service = BearerAuthService::new(service, security.bearer_token);
    loop {
        let (stream, _peer) = listener
            .accept()
            .await
            .map_err(|error| McpServerError::new(format!("MCP HTTP accept failed: {error}")))?;
        let io = TokioIo::new(stream);
        let hyper_service = TowerToHyperService::new(service.clone());
        tokio::spawn(async move {
            // Per-connection errors (client disconnects, malformed frames) are
            // isolated to the connection task; they must not stop the server.
            let _ = http1::Builder::new()
                .serve_connection(io, hyper_service)
                .await;
        });
    }
}

type BoxHttpResponse = Response<BoxBody<Bytes, Infallible>>;
type BoxHttpFuture =
    Pin<Box<dyn Future<Output = Result<BoxHttpResponse, Infallible>> + Send + 'static>>;

#[derive(Clone)]
struct BearerAuthService<S> {
    inner: S,
    bearer_token: String,
}

impl<S> BearerAuthService<S> {
    fn new(inner: S, bearer_token: String) -> Self {
        Self {
            inner,
            bearer_token,
        }
    }
}

impl<S, B> Service<Request<B>> for BearerAuthService<S>
where
    S: Service<Request<B>, Response = BoxHttpResponse, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = BoxHttpResponse;
    type Error = Infallible;
    type Future = BoxHttpFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        if !request_has_bearer_token(&request, &self.bearer_token) {
            return Box::pin(async { Ok(unauthorized_response()) });
        }
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await })
    }
}

fn checked_listen_addr(
    listen_addr: &str,
    allow_non_loopback: bool,
) -> Result<SocketAddr, McpServerError> {
    let candidates = listen_addr.to_socket_addrs().map_err(|error| {
        McpServerError::new(format!(
            "MCP HTTP listen address {listen_addr} is invalid: {error}"
        ))
    })?;
    let addrs = candidates.collect::<Vec<_>>();
    let Some(bind_addr) = addrs.first().copied() else {
        return Err(McpServerError::new(format!(
            "MCP HTTP listen address {listen_addr} did not resolve."
        )));
    };
    if !allow_non_loopback && addrs.iter().any(|addr| !addr.ip().is_loopback()) {
        return Err(McpServerError::new(format!(
            "MCP HTTP listen address {listen_addr} is not loopback; pass --http-allow-non-loopback to opt in."
        )));
    }
    Ok(bind_addr)
}

fn validate_http_security(security: &McpHttpServerSecurity) -> Result<(), McpServerError> {
    if security.bearer_token.is_empty() {
        return Err(McpServerError::new(
            "MCP HTTP bearer token must not be empty.",
        ));
    }
    Ok(())
}

fn request_has_bearer_token<B>(request: &Request<B>, expected_token: &str) -> bool {
    let Some(header) = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(token) = header.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.as_bytes(), expected_token.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(left_byte ^ right_byte);
    }
    diff == 0
}

fn unauthorized_response() -> BoxHttpResponse {
    let body = Full::new(Bytes::from_static(b"MCP HTTP bearer token required.\n")).boxed();
    match Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(WWW_AUTHENTICATE, "Bearer")
        .body(body)
    {
        Ok(response) => response,
        Err(_error) => Response::new(Full::<Bytes>::default().boxed()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn io_err(context: &str, error: impl std::fmt::Display) -> McpServerError {
        McpServerError::new(format!("{context}: {error}"))
    }

    fn test_options() -> McpServerOptions {
        McpServerOptions {
            package_name: "http-server-test".to_owned(),
            package_version: "0.0.0".to_owned(),
            instructions: None,
            effects: Default::default(),
            tools: Vec::new(),
        }
    }

    fn test_security() -> McpHttpServerSecurity {
        McpHttpServerSecurity::loopback_only("test-http-token".to_owned())
    }

    #[test]
    fn rejects_http_without_bearer_token() -> Result<(), McpServerError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| io_err("building the test runtime", error))?;
        runtime.block_on(async {
            // Bind first, then hand the listener to the server, so the client can
            // connect without racing the accept loop.
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|error| io_err("binding the test listener", error))?;
            let addr = listener
                .local_addr()
                .map_err(|error| io_err("reading the bound addr", error))?;
            tokio::spawn(serve_mcp_http_listener(
                listener,
                test_options(),
                test_security(),
            ));

            let mut stream = tokio::net::TcpStream::connect(addr)
                .await
                .map_err(|error| io_err("connecting to the server", error))?;
            let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
            let request = format!(
                "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(|error| io_err("writing the request", error))?;

            let mut buffer = vec![0_u8; 4096];
            let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                .await
                .map_err(|error| io_err("response timed out", error))?
                .map_err(|error| io_err("reading the response", error))?;
            let response = String::from_utf8_lossy(&buffer[..read]);
            let head = &response[..response.len().min(600)];
            assert!(
                response.starts_with("HTTP/1.1 401"),
                "the governed MCP server must reject missing bearer auth; got: {head}"
            );
            assert!(
                response.to_ascii_lowercase().contains("www-authenticate"),
                "the rejection must advertise bearer auth; got: {head}"
            );
            Ok(())
        })
    }

    #[test]
    fn serves_an_mcp_initialize_over_http_with_bearer_token() -> Result<(), McpServerError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| io_err("building the test runtime", error))?;
        runtime.block_on(async {
            // Bind first, then hand the listener to the server, so the client can
            // connect without racing the accept loop.
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|error| io_err("binding the test listener", error))?;
            let addr = listener
                .local_addr()
                .map_err(|error| io_err("reading the bound addr", error))?;
            tokio::spawn(serve_mcp_http_listener(
                listener,
                test_options(),
                test_security(),
            ));

            let mut stream = tokio::net::TcpStream::connect(addr)
                .await
                .map_err(|error| io_err("connecting to the server", error))?;
            let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
            let request = format!(
                "POST / HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-http-token\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(|error| io_err("writing the request", error))?;

            let mut buffer = vec![0_u8; 4096];
            let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                .await
                .map_err(|error| io_err("response timed out", error))?
                .map_err(|error| io_err("reading the response", error))?;
            let response = String::from_utf8_lossy(&buffer[..read]);
            let head = &response[..response.len().min(600)];
            assert!(
                response.starts_with("HTTP/1.1 200"),
                "the governed MCP server must answer authorized http initialize with 200; got: {head}"
            );
            let lower = response.to_ascii_lowercase();
            assert!(
                lower.contains("protocolversion")
                    || lower.contains("serverinfo")
                    || lower.contains("mcp-session-id")
                    || lower.contains("\"result\""),
                "the response must carry an mcp initialize result; got: {head}"
            );
            Ok(())
        })
    }

    #[test]
    fn rejects_non_loopback_listen_without_opt_in() -> Result<(), McpServerError> {
        let error = checked_listen_addr("0.0.0.0:8080", false)
            .err()
            .ok_or_else(|| McpServerError::new("non-loopback address was accepted"))?;
        assert!(
            error.to_string().contains("--http-allow-non-loopback"),
            "non-loopback rejection must point at explicit opt-in; got: {error}"
        );
        assert!(checked_listen_addr("0.0.0.0:8080", true).is_ok());
        assert!(checked_listen_addr(DEFAULT_MCP_HTTP_LISTEN_ADDR, false).is_ok());
        Ok(())
    }
}
