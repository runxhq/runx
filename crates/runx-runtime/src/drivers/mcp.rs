//! The `mcp` driver: an operation as one tool call on the connector's MCP
//! server, over streamable HTTP or stdio.
//!
//! rmcp speaks the protocol: version negotiation, sessions, event streams and
//! recovering an expired session. Its HTTP leg is runx's guarded client, so
//! the server is reached only as that transport admits: public addresses
//! unless the operator allows private ones, and no redirects. A stdio server
//! is a declared trusted host process the runtime starts, read through the
//! same bounded framing as every runx MCP client. A session opens once per
//! server and credential; before each governed call the kernel reads the
//! tools the server serves and holds them against the manifest's pins. A
//! tool's own annotations never decide read or write: the manifest's verb
//! does.

mod stdio;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientInfo, Implementation,
};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceError};
use runx_contracts::connector::{
    Digest, McpImplementation, TemplateInputs, mcp_tool_digest, stable_digest,
};
use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};

use crate::credentials::SecretTaint;
use crate::http::{ReqwestHttpTransport, RuntimeHttpHeader};
use crate::kernel::operation::AdmittedCall;
use crate::kernel::operation::confirm::{Check, Driver, Observed, Unperformed};
use stdio::{StdioServerProcess, start_stdio_server};

/// Open sessions, one per server, credential and headers; in the harness,
/// the case's stand-in for every server.
#[derive(Default)]
pub(crate) struct Sessions {
    open: Mutex<HashMap<String, Arc<Session>>>,
    stand_in: Option<Arc<[StandInCall]>>,
}

/// A tool call the harness answers in place of a server: the exact tool,
/// arguments and headers, and the result the server would give.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StandInCall {
    pub tool: String,
    pub arguments: JsonValue,
    pub headers: BTreeMap<String, String>,
    pub result: JsonObject,
}

/// One initialized session on a server.
pub(crate) struct Session {
    peer: Peer,
    deadline: Duration,
}

/// Why a stdio session did not open, and whether its server had started
/// first, and so received its environment.
#[derive(Debug)]
pub(crate) struct StdioOpenFailure {
    pub(crate) reason: String,
    pub(crate) started: bool,
}

enum Peer {
    Served {
        service: Box<RunningService<RoleClient, ClientInfo>>,
        /// A stdio server's process, stopped when the session is dropped.
        _process: Option<StdioServerProcess>,
    },
    /// The harness, standing in for exactly the server the manifest pins.
    StandIn {
        calls: Arc<[StandInCall]>,
        headers: BTreeMap<String, String>,
    },
}

/// A stdio server as the runtime starts it: its command line, working
/// directory and exact environment, credential included.
#[derive(serde::Serialize)]
pub(crate) struct StdioServer {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

impl StdioServer {
    /// Start the server and initialize a session on it within `deadline`.
    /// `started` is set once the process runs.
    async fn session(&self, deadline: Duration, started: &AtomicBool) -> Result<Session, String> {
        let (process, transport) = start_stdio_server(
            &self.command,
            &self.args,
            self.cwd.clone(),
            self.env.clone(),
        )?;
        started.store(true, Ordering::SeqCst);
        Ok(Session {
            peer: Peer::Served {
                service: Box::new(connect(transport).await?),
                _process: Some(process),
            },
            deadline,
        })
    }
}

impl Session {
    /// Whether a server process the runtime started serves this session.
    pub(crate) fn runs_process(&self) -> bool {
        matches!(
            self.peer,
            Peer::Served {
                _process: Some(_),
                ..
            }
        )
    }
}

impl Sessions {
    /// Whether the harness answers every call in place of a server.
    pub(crate) fn stands_in(&self) -> bool {
        self.stand_in.is_some()
    }

    /// Sessions the harness answers from `calls` instead of any server.
    pub(crate) fn standing_in(calls: Vec<StandInCall>) -> Self {
        Self {
            open: Mutex::default(),
            stand_in: Some(calls.into()),
        }
    }

    /// The session on `url` under the presented credential and headers, over runx's guarded
    /// `http`.
    pub(crate) fn open(
        &self,
        http: &ReqwestHttpTransport,
        url: &str,
        credential: Option<&RuntimeHttpHeader>,
        headers: &BTreeMap<String, String>,
    ) -> Result<Arc<Session>, String> {
        let deadline = http.request_timeout();
        if let Some(stand_in) = self.stand_in(headers, deadline) {
            return Ok(stand_in);
        }
        let identity = serde_json::to_vec(&(
            url,
            credential.map(|header| (&header.name, sha256_prefixed(header.value.as_bytes()))),
            headers,
        ))
        .map_err(|error| error.to_string())?;
        let key = format!("http\0{}", sha256_prefixed(&identity));
        self.reuse_or(key, deadline, async move {
            let transport = http
                .mcp(url, credential, headers)
                .map_err(|error| error.to_string())?;
            Session::initialize(transport, deadline).await
        })
    }

    /// The session on the stdio `server`, started once per command line and
    /// environment.
    pub(crate) fn open_stdio(
        &self,
        server: &StdioServer,
        deadline: Duration,
    ) -> Result<Arc<Session>, StdioOpenFailure> {
        if let Some(stand_in) = self.stand_in(&BTreeMap::new(), deadline) {
            return Ok(stand_in);
        }
        let identity = serde_json::to_vec(server).map_err(|error| StdioOpenFailure {
            reason: error.to_string(),
            started: false,
        })?;
        let key = format!("stdio\0{}", sha256_prefixed(&identity));
        let started = AtomicBool::new(false);
        self.reuse_or(key, deadline, server.session(deadline, &started))
            .map_err(|reason| StdioOpenFailure {
                reason,
                started: started.load(Ordering::SeqCst),
            })
    }

    fn stand_in(
        &self,
        headers: &BTreeMap<String, String>,
        deadline: Duration,
    ) -> Option<Arc<Session>> {
        self.stand_in.as_ref().map(|calls| {
            Arc::new(Session {
                peer: Peer::StandIn {
                    calls: Arc::clone(calls),
                    headers: headers.clone(),
                },
                deadline,
            })
        })
    }

    fn reuse_or(
        &self,
        key: String,
        deadline: Duration,
        start: impl Future<Output = Result<Session, String>>,
    ) -> Result<Arc<Session>, String> {
        if let Some(session) = self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return Ok(Arc::clone(session));
        }
        let session = Arc::new(block_on(deadline, start)??);
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, Arc::clone(&session));
        Ok(session)
    }
}

/// A tool a server serves: its name, the digest a connector pins it by, and
/// its definition as served.
pub(crate) type ServedTool = (String, Digest, JsonObject);

/// Every tool the server at `url` serves, read through one fresh session over
/// runx's guarded `http`, for an author to pin. Discovery presents the
/// operator's credential, so nothing it returns repeats it: a failure's
/// reason is scrubbed as a governed call's is, and a definition that carries
/// the credential is refused, never pinned.
pub(crate) fn served_tools(
    http: &ReqwestHttpTransport,
    url: &str,
    bearer: Option<&str>,
) -> Result<Vec<ServedTool>, String> {
    let taint = SecretTaint::from_values(bearer);
    block_on(http.request_timeout(), async move {
        let credential = bearer
            .map(|token| RuntimeHttpHeader::secret("authorization", format!("Bearer {token}")));
        let service = connect(
            http.mcp(url, credential.as_ref(), &BTreeMap::new())
                .map_err(|error| error.to_string())?,
        )
        .await?;
        let tools = list(&service).await;
        service.cancel().await.map_err(|error| error.to_string())?;
        tools
    })
    .and_then(|tools| tools)
    .map_err(|reason| taint.redact_output_text(reason))
    .and_then(|tools| unrepeated(tools, &taint))
}

/// `tools`, refused when any definition repeats the presented credential.
fn unrepeated(tools: Vec<ServedTool>, taint: &SecretTaint) -> Result<Vec<ServedTool>, String> {
    match tools.iter().find(|(_, _, definition)| {
        let served = JsonValue::Object(definition.clone());
        taint.redact_value(served.clone()) != served
    }) {
        Some((name, _, _)) => Err(format!(
            "the MCP server's tool {} repeats the credential presented to it, so runx pins none of its tools",
            taint.redact_text(name.as_str())
        )),
        None => Ok(tools),
    }
}

/// Every tool the stdio `server` serves, read through one session on a
/// process started for it, for an author to pin.
pub(crate) fn served_stdio_tools(
    server: &StdioServer,
    deadline: Duration,
) -> Result<Vec<ServedTool>, String> {
    block_on(deadline, async move {
        let (process, transport) = start_stdio_server(
            &server.command,
            &server.args,
            server.cwd.clone(),
            server.env.clone(),
        )?;
        let service = connect(transport).await?;
        let tools = list(&service).await;
        service.cancel().await.map_err(|error| error.to_string())?;
        drop(process);
        tools
    })?
}

async fn connect<T, E, A>(transport: T) -> Result<RunningService<RoleClient, ClientInfo>, String>
where
    T: rmcp::transport::IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("runx", env!("CARGO_PKG_VERSION")),
    );
    rmcp::serve_client(info, transport)
        .await
        .map_err(|error| format!("the MCP server did not initialize: {error}"))
}

async fn list(service: &RunningService<RoleClient, ClientInfo>) -> Result<Vec<ServedTool>, String> {
    let tools = service
        .peer()
        .list_all_tools()
        .await
        .map_err(|error| format!("the MCP server did not list its tools: {error}"))?;
    tools
        .iter()
        .map(|tool| match JsonValue::from_serialize(tool) {
            Ok(JsonValue::Object(definition)) => mcp_tool_digest(&definition)
                .map(|digest| (tool.name.to_string(), digest, definition))
                .map_err(|error| error.to_string()),
            _ => Err(format!(
                "the server served tool {} in an unreadable shape",
                tool.name
            )),
        })
        .collect()
}

impl Session {
    /// Initialize over `transport`.
    async fn initialize<T, E, A>(transport: T, deadline: Duration) -> Result<Self, String>
    where
        T: rmcp::transport::IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        Ok(Self {
            peer: Peer::Served {
                service: Box::new(connect(transport).await?),
                _process: None,
            },
            deadline,
        })
    }

    /// The digest of every tool the server serves now, read afresh so a pin
    /// is held against the definition the call will reach. The harness's
    /// stand-in serves exactly what the manifest pins, so it has none to
    /// compare.
    pub(crate) fn served(&self) -> Result<Option<BTreeMap<String, Digest>>, String> {
        let Peer::Served { service, .. } = &self.peer else {
            return Ok(None);
        };
        Ok(Some(
            block_on(self.deadline, async move { list(service).await })??
                .into_iter()
                .map(|(name, digest, _)| (name, digest))
                .collect(),
        ))
    }

    /// Call `tool` with `arguments`. A protocol error means the tool never
    /// ran; an error the tool itself reports may follow a partial write, so
    /// for a mutation it leaves the outcome unknown.
    fn call(
        &self,
        tool: &str,
        arguments: &JsonValue,
        mutation: bool,
    ) -> Result<Observed, Unperformed> {
        let Ok(serde_json::Value::Object(wire_arguments)) = serde_json::to_value(arguments) else {
            return Err(Unperformed::NotSent {
                code: "invalid_request",
                reason: format!("the arguments for tool {tool} are not an object"),
            });
        };
        let answer = match &self.peer {
            Peer::Served { service, .. } => {
                let peer = service.peer().clone();
                let request =
                    CallToolRequestParams::new(tool.to_owned()).with_arguments(wire_arguments);
                // One `tools/call`: a server's request for more input, or a
                // task it starts, is not answered with another call.
                let answered =
                    block_on(self.deadline, async move { peer.call_tool_once(request).await })
                        .map_err(|reason| Unperformed::MaybeSent {
                            code: "transport_failed",
                            reason,
                        })?;
                let answer = match answered {
                    Ok(CallToolResponse::Complete(answer)) => answer,
                    // Input the server requires, a task it started, or any
                    // other answer short of a result.
                    Ok(_) => {
                        let reason = format!("tool {tool} did not answer in one call");
                        return Err(match mutation {
                            true => Unperformed::MaybeSent {
                                code: "tool_incomplete",
                                reason,
                            },
                            false => Unperformed::Refused {
                                code: "tool_incomplete",
                                reason,
                                answer: None,
                            },
                        });
                    }
                    Err(ServiceError::McpError(error)) => {
                        return Err(Unperformed::Refused {
                            code: "tool_error",
                            reason: format!("tool {tool} failed: {}", error.message),
                            answer: None,
                        });
                    }
                    Err(error) => {
                        return Err(Unperformed::MaybeSent {
                            code: "transport_failed",
                            reason: error.to_string(),
                        });
                    }
                };
                let Ok(JsonValue::Object(answer)) = JsonValue::from_serialize(&answer) else {
                    return Err(Unperformed::MaybeSent {
                        code: "invalid_response",
                        reason: format!("tool {tool} answered in an unreadable shape"),
                    });
                };
                answer
            }
            Peer::StandIn { calls, headers } => calls
                .iter()
                .find(|call| {
                    call.tool == tool && &call.arguments == arguments && &call.headers == headers
                })
                .map(|call| call.result.clone())
                .ok_or_else(|| Unperformed::NotSent {
                    code: "invalid_request",
                    reason: format!(
                        "the harness has no MCP call for tool {tool} with arguments {} and headers {}",
                        serde_json::to_string(arguments).unwrap_or_default(),
                        serde_json::to_string(headers).unwrap_or_default(),
                    ),
                })?,
        };
        if answer.get("isError").and_then(JsonValue::as_bool) == Some(true) {
            let reason = format!("tool {tool} reported an error: {}", tool_text(&answer));
            return Err(match mutation {
                true => Unperformed::MaybeSent {
                    code: "tool_error",
                    reason,
                },
                false => Unperformed::Refused {
                    code: "tool_error",
                    reason,
                    answer: Some(JsonValue::Object(answer)),
                },
            });
        }
        let value = tool_value(answer);
        let evidence = stable_digest(&value).map_err(|error| Unperformed::MaybeSent {
            code: "invalid_response",
            reason: error.to_string(),
        })?;
        Ok(Observed {
            value,
            evidence: vec![evidence.as_str().to_owned()],
        })
    }
}

pub(crate) struct Mcp<'a> {
    pub session: &'a Session,
    pub call: &'a AdmittedCall<'a>,
    pub spec: &'a McpImplementation,
}

impl Driver for Mcp<'_> {
    fn perform(&self, _item: Option<usize>) -> Result<Observed, Unperformed> {
        let arguments = self
            .spec
            .arguments
            .render(&self.inputs(None))
            .map_err(|error| Unperformed::NotSent {
                code: "invalid_request",
                reason: error.to_string(),
            })?;
        self.session
            .call(self.spec.tool.as_str(), &arguments, self.call.mutation())
    }

    fn check(&self, check: Check, write: &JsonValue) -> Result<Observed, String> {
        let (
            Check::Readback {
                readback: 0,
                item: None,
            },
            Some(readback),
        ) = (check, &self.spec.readback)
        else {
            return Err(format!("the implementation declares no {check:?}"));
        };
        let responses = BTreeMap::from([(
            "write".to_owned(),
            match write {
                JsonValue::Object(object) => object.clone(),
                other => JsonObject::from([("value".to_owned(), other.clone())]),
            },
        )]);
        let arguments = readback
            .arguments
            .render(&self.inputs(Some(&responses)))
            .map_err(|error| error.to_string())?;
        self.session
            .call(readback.tool.as_str(), &arguments, false)
            .map_err(Unperformed::reason)
    }

    fn result(
        &self,
        write: &JsonValue,
        _confirmed: &[JsonValue],
    ) -> Result<(JsonValue, Option<String>), String> {
        // A write is identified by the key it was sent with, which is stable
        // for its intent. Its answer cannot identify it: two different writes
        // may be answered alike.
        let operation_id = match self.call.mutation() {
            true => Some(format!(
                "mcp:{}",
                self.call
                    .idempotency_key
                    .ok_or("a write is sent with an idempotency key")?
            )),
            false => None,
        };
        Ok((write.clone(), operation_id))
    }
}

impl Mcp<'_> {
    fn inputs<'a>(
        &'a self,
        responses: Option<&'a BTreeMap<String, JsonObject>>,
    ) -> TemplateInputs<'a> {
        TemplateInputs {
            arguments: Some(self.call.arguments),
            parameters: Some(self.call.parameters),
            responses,
            idempotency_key: self.call.idempotency_key,
            ..TemplateInputs::default()
        }
    }
}

/// Drive `future` on the runtime that owns the guarded client's connections,
/// within `deadline`.
fn block_on<T>(deadline: Duration, future: impl Future<Output = T>) -> Result<T, String> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err("an MCP call cannot block inside an async runtime".to_owned());
    }
    crate::http::http_runtime()
        .map_err(|error| error.to_string())?
        .block_on(async { tokio::time::timeout(deadline, future).await })
        .map_err(|_| {
            format!(
                "the MCP server did not answer within {}ms",
                deadline.as_millis()
            )
        })
}

/// A tool result as one value: its structured content, else its one JSON
/// text content, else its content.
fn tool_value(mut answer: JsonObject) -> JsonValue {
    if let Some(structured @ JsonValue::Object(_)) = answer.remove("structuredContent") {
        return structured;
    }
    let content = answer
        .remove("content")
        .unwrap_or(JsonValue::Array(Vec::new()));
    if let Some([JsonValue::Object(item)]) = content.as_array().map(Vec::as_slice)
        && item.get("type").and_then(JsonValue::as_str) == Some("text")
        && let Some(value @ JsonValue::Object(_)) = item
            .get("text")
            .and_then(JsonValue::as_str)
            .and_then(|text| serde_json::from_str::<JsonValue>(text).ok())
    {
        return value;
    }
    JsonValue::Object(JsonObject::from([("content".to_owned(), content)]))
}

fn tool_text(answer: &JsonObject) -> String {
    answer
        .get("content")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_object()?.get("text").and_then(JsonValue::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::{
        CallToolResponse, CallToolResult, ContentBlock, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerInfo, Tool,
    };
    use rmcp::service::RequestContext;
    use rmcp::{ErrorData, RoleServer};

    use super::*;

    /// A server serving `echo`, which answers its arguments as structured
    /// content, and `fail`, which reports a tool error.
    #[derive(Clone)]
    struct Fixture;

    impl rmcp::ServerHandler for Fixture {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
            let schema = Arc::new(serde_json::Map::from_iter([(
                "type".to_owned(),
                serde_json::Value::String("object".to_owned()),
            )]));
            std::future::ready(Ok(ListToolsResult::with_all_items(vec![
                Tool::new("echo", "Echo the arguments.", Arc::clone(&schema)),
                Tool::new("fail", "Always fail.", schema),
            ])))
        }

        fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<CallToolResponse, ErrorData>> + Send + '_ {
            let result = match request.name.as_ref() {
                "echo" => CallToolResult::structured(serde_json::Value::Object(
                    request.arguments.unwrap_or_default(),
                )),
                _ => CallToolResult::error(vec![ContentBlock::text("the fixture refuses")]),
            };
            std::future::ready(Ok(result.into()))
        }
    }

    fn session() -> Result<Session, String> {
        let deadline = Duration::from_secs(5);
        block_on(deadline, async move {
            let (client, server) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                if let Ok(running) = rmcp::serve_server(Fixture, server).await {
                    let _ = running.waiting().await;
                }
            });
            Session::initialize(client, deadline).await
        })?
    }

    #[test]
    fn a_write_is_identified_by_its_key_never_by_its_answer() -> Result<(), String> {
        let session = session()?;
        let spec = serde_json::from_value::<McpImplementation>(serde_json::json!({
            "tool": "echo",
            "arguments": {"$argument": ""},
            "tool_digest": format!("sha256:{}", "0".repeat(64)),
        }))
        .map_err(|error| error.to_string())?;
        let arguments = JsonValue::Object(JsonObject::new());
        let parameters = JsonObject::new();
        let env = BTreeMap::new();
        let call = AdmittedCall::for_tests(
            &arguments,
            &parameters,
            &env,
            runx_contracts::connector::OperationAccess::Mutate,
        );
        let driver = Mcp {
            session: &session,
            call: &call,
            spec: &spec,
        };
        let accepted = JsonValue::Object(JsonObject::from([(
            "status".to_owned(),
            JsonValue::String("accepted".to_owned()),
        )]));
        let (_, operation_id) = driver.result(&accepted, &[])?;
        // The call's key, which no other write shares; never the answer,
        // which another write may receive word for word.
        assert_eq!(operation_id.as_deref(), Some("mcp:key-1"));
        Ok(())
    }

    #[test]
    fn a_session_pins_what_is_served_and_calls_through_rmcp() -> Result<(), String> {
        calls_through(&session()?)
    }

    /// The whole wire: rmcp's streamable HTTP server on loopback, reached
    /// through runx's guarded HTTP client, with sessions and event streams.
    #[cfg(feature = "mcp-http-server")]
    #[test]
    fn a_session_runs_over_streamable_http_through_the_guarded_client() -> Result<(), String> {
        use hyper_util::rt::TokioIo;
        use hyper_util::service::TowerToHyperService;
        use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
        use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

        let runtime = crate::http::http_runtime().map_err(|error| error.to_string())?;
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let observed_headers = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&observed_headers);
        runtime.spawn(async move {
            let service = StreamableHttpService::new(
                || Ok(Fixture),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default(),
            );
            while let Ok((stream, _)) = listener.accept().await {
                let service = TowerToHyperService::new(service.clone());
                let captured = Arc::clone(&captured);
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            if let Ok(mut captured) = captured.lock() {
                                captured.push(
                                    request
                                        .headers()
                                        .get("x-fixture-auth")
                                        .and_then(|value| value.to_str().ok())
                                        .map(str::to_owned),
                                );
                            }
                            let service = service.clone();
                            async move { hyper::service::Service::call(&service, request).await }
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let transport = crate::http::ReqwestHttpTransport::with_private_network_access()
            .map_err(|error| error.to_string())?;
        let sessions = Sessions::default();
        let url = format!("http://{address}/mcp");
        let credential = RuntimeHttpHeader::secret("x-fixture-auth", "presented-token");
        let session = sessions.open(&transport, &url, Some(&credential), &BTreeMap::new())?;
        assert!(Arc::ptr_eq(
            &session,
            &sessions.open(&transport, &url, Some(&credential), &BTreeMap::new())?
        ));
        let replacement = RuntimeHttpHeader::secret("x-fixture-auth", "different-account-token");
        assert!(!Arc::ptr_eq(
            &session,
            &sessions.open(&transport, &url, Some(&replacement), &BTreeMap::new())?
        ));
        let served = served_tools(&transport, &url, None)?;
        assert_eq!(
            served
                .iter()
                .map(|(name, digest, _)| (name.clone(), digest.clone()))
                .collect::<BTreeMap<_, _>>(),
            session.served()?.unwrap_or_default()
        );
        assert!(served.iter().all(|(name, _, definition)| {
            definition.get("name").and_then(JsonValue::as_str) == Some(name.as_str())
        }));
        calls_through(&session)?;
        let captured = observed_headers.lock().map_err(|error| error.to_string())?;
        assert!(
            captured
                .iter()
                .any(|value| value.as_deref() == Some("presented-token"))
        );
        assert!(
            captured
                .iter()
                .any(|value| value.as_deref() == Some("different-account-token"))
        );
        Ok(())
    }
    /// Discovery presents the operator's bearer; a server that answers
    /// initialization with an error repeating it never puts it in the reason
    /// an author reads.
    #[cfg(feature = "mcp-http-server")]
    #[test]
    fn discovery_never_repeats_the_presented_bearer() -> Result<(), String> {
        use http_body_util::Full;
        use hyper_util::rt::TokioIo;

        const TOKEN: &str = "presented-discovery-token";
        let runtime = crate::http::http_runtime().map_err(|error| error.to_string())?;
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        runtime.spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(
                        |request: hyper::Request<hyper::body::Incoming>| async move {
                            let presented = request
                                .headers()
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_owned();
                            let body = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": 0,
                                "error": {"code": -32600, "message": format!("rejected {presented}")},
                            });
                            hyper::Response::builder()
                                .header("content-type", "application/json")
                                .body(Full::new(bytes::Bytes::from(body.to_string())))
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let transport = crate::http::ReqwestHttpTransport::with_private_network_access()
            .map_err(|error| error.to_string())?;
        let reason = served_tools(&transport, &format!("http://{address}/mcp"), Some(TOKEN))
            .err()
            .ok_or("the server refused initialization")?;
        // The server's message reached the reason, with the bearer scrubbed.
        assert!(
            reason.contains("rejected Bearer [redacted-credential]"),
            "{reason}"
        );
        assert!(!reason.contains(TOKEN), "{reason}");
        Ok(())
    }

    /// A served definition that carries the presented credential is refused
    /// whole, so no manifest is written with it.
    #[test]
    fn a_definition_repeating_the_credential_pins_nothing() -> Result<(), String> {
        let taint = SecretTaint::from_values(Some("presented-discovery-token"));
        let tool = |description: &str| -> Result<ServedTool, String> {
            let definition = JsonObject::from([
                ("name".to_owned(), JsonValue::String("lookup".to_owned())),
                (
                    "description".to_owned(),
                    JsonValue::String(description.to_owned()),
                ),
            ]);
            let digest = mcp_tool_digest(&definition).map_err(|error| error.to_string())?;
            Ok(("lookup".to_owned(), digest, definition))
        };
        assert!(unrepeated(vec![tool("reads a record")?], &taint).is_ok());
        let refused = unrepeated(vec![tool("reads with presented-discovery-token")?], &taint)
            .err()
            .ok_or("a tainted definition was pinned")?;
        assert!(refused.contains("lookup"), "{refused}");
        assert!(!refused.contains("presented-discovery-token"), "{refused}");
        Ok(())
    }

    /// A stdio session reports whether a server process ran, so a receipt
    /// records the credential only when a process received it: never for the
    /// harness's stand-in or a server that failed to start, always for one
    /// that started and then failed to open its session.
    #[test]
    fn a_stdio_session_says_whether_its_server_started() -> Result<(), String> {
        let server = |command: &str, args: &[&str]| StdioServer {
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: std::env::temp_dir(),
            env: BTreeMap::from([("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())]),
        };
        let deadline = Duration::from_secs(20);

        let stand_in = Sessions::standing_in(Vec::new())
            .open_stdio(&server("node", &[]), deadline)
            .map_err(|failure| failure.reason)?;
        assert!(!stand_in.runs_process());

        let missing = Sessions::default()
            .open_stdio(&server("/nonexistent/runx-mcp-server", &[]), deadline)
            .err()
            .ok_or("a missing command opens no session")?;
        assert!(!missing.started, "{}", missing.reason);

        let exits = Sessions::default()
            .open_stdio(&server("node", &["-e", "process.exit(3)"]), deadline)
            .err()
            .ok_or("a server that exits opens no session")?;
        assert!(exits.started, "{}", exits.reason);
        Ok(())
    }

    /// A stdio server the runtime starts: pinned and called like any other,
    /// with exactly the environment it was started with.
    #[test]
    fn a_stdio_server_runs_as_a_started_process() -> Result<(), String> {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixture = manifest_dir.join("../../fixtures/connectors/mcp-echo/stdio-server.mjs");
        let server = StdioServer {
            command: "node".to_owned(),
            args: vec![fixture.to_string_lossy().into_owned()],
            cwd: manifest_dir.to_path_buf(),
            env: BTreeMap::from([
                ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
                ("FIXTURE_TOKEN".to_owned(), "delivered".to_owned()),
            ]),
        };
        let deadline = Duration::from_secs(20);
        let sessions = Sessions::default();
        let open = || {
            sessions
                .open_stdio(&server, deadline)
                .map_err(|failure| failure.reason)
        };
        let session = open()?;
        assert!(session.runs_process());
        assert!(Arc::ptr_eq(&session, &open()?));
        let served = served_stdio_tools(&server, deadline)?;
        assert_eq!(
            served
                .iter()
                .map(|(name, digest, _)| (name.clone(), digest.clone()))
                .collect::<BTreeMap<_, _>>(),
            session.served()?.unwrap_or_default()
        );
        let text = |observed: Observed| {
            serde_json::to_value(&observed.value).unwrap_or_default()["content"][0]["text"]
                .as_str()
                .map(str::to_owned)
        };
        let echo = JsonValue::Object(JsonObject::from([(
            "message".to_owned(),
            JsonValue::String("hello".to_owned()),
        )]));
        assert_eq!(
            text(
                session
                    .call("echo", &echo, false)
                    .map_err(Unperformed::reason)?
            ),
            Some("hello".to_owned())
        );
        let variable = |name: &str| {
            JsonValue::Object(JsonObject::from([(
                "name".to_owned(),
                JsonValue::String(name.to_owned()),
            )]))
        };
        assert_eq!(
            text(
                session
                    .call("env", &variable("FIXTURE_TOKEN"), false)
                    .map_err(Unperformed::reason)?
            ),
            Some("delivered".to_owned())
        );
        assert_eq!(
            text(
                session
                    .call("env", &variable("HOME"), false)
                    .map_err(Unperformed::reason)?
            ),
            Some(String::new()),
            "the server sees only the environment it was started with"
        );
        Ok(())
    }

    #[test]
    fn a_literal_private_address_is_refused_before_any_session() -> Result<(), String> {
        let transport = crate::http::ReqwestHttpTransport::for_provider_operation(false)
            .map_err(|error| error.to_string())?;
        let refused =
            Sessions::default().open(&transport, "https://127.0.0.1/mcp", None, &BTreeMap::new());
        assert!(refused.is_err());
        assert!(served_tools(&transport, "https://10.0.0.1/mcp", None).is_err());
        Ok(())
    }

    fn calls_through(session: &Session) -> Result<(), String> {
        assert_eq!(
            session
                .served()?
                .unwrap_or_default()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["echo", "fail"]
        );

        let arguments = JsonValue::Object(JsonObject::from([(
            "id".to_owned(),
            JsonValue::String("m-1".to_owned()),
        )]));
        let echoed = session
            .call("echo", &arguments, true)
            .map_err(Unperformed::reason)?;
        assert_eq!(echoed.value, arguments);
        assert_eq!(echoed.evidence.len(), 1);

        match session.call("fail", &arguments, false) {
            Err(Unperformed::Refused { code, reason, .. }) => {
                assert_eq!(code, "tool_error");
                assert!(reason.contains("the fixture refuses"), "{reason}");
            }
            other => {
                return Err(format!(
                    "a read's tool error is a refusal, got {:?}",
                    other.map(|observed| observed.value)
                ));
            }
        }
        // A tool may report an error after it wrote: the outcome is unknown.
        match session.call("fail", &arguments, true) {
            Err(Unperformed::MaybeSent { code, .. }) => assert_eq!(code, "tool_error"),
            other => {
                return Err(format!(
                    "a write's tool error leaves the outcome unknown, got {:?}",
                    other.map(|observed| observed.value)
                ));
            }
        }
        Ok(())
    }
}
