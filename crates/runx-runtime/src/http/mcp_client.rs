// Module rationale: rmcp's reqwest client reads a POST's JSON body whole with no
// ceiling, so a remote MCP server could answer one call with unbounded bytes.
// This client sends each POST itself and reads its body through the same
// bounded reader as every other response; it follows rmcp 3.3.0's POST
// semantics, which the exact version pin holds still. GET streams and session
// deletes stay rmcp's, whose events are bounded by the transport's
// `max_sse_event_size`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use reqwest::header::{ACCEPT, HeaderName, HeaderValue, WWW_AUTHENTICATE};
use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, ErrorData, JsonRpcMessage, ServerJsonRpcMessage,
};
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE as EVENT_STREAM, HEADER_SESSION_ID as SESSION_HEADER,
    JSON_MIME_TYPE as JSON,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::SseStream;

use super::read_response_body;

/// The guarded client, with every MCP response body bounded by `limit`.
#[derive(Clone)]
pub(crate) struct BoundedMcpClient {
    pub(super) client: reqwest::Client,
    pub(super) limit: usize,
}

type Result<T> = std::result::Result<T, StreamableHttpError<reqwest::Error>>;

impl StreamableHttpClient for BoundedMcpClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse> {
        self.post(uri, message, session_id, auth_header, custom_headers)
            .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        _max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse> {
        self.post(uri, message, session_id, auth_header, custom_headers)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<()> {
        self.client
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse> {
        self.client
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                self.limit,
            )
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxedSseResponse> {
        self.client
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }
}

impl BoundedMcpClient {
    async fn post(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse> {
        let mut request = self
            .client
            .post(uri.as_ref())
            .header(ACCEPT, format!("{EVENT_STREAM}, {JSON}"));
        if let Some(token) = auth_header {
            request = request.bearer_auth(token);
        }
        for (name, value) in custom_headers {
            request = request.header(name, value);
        }
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(SESSION_HEADER, session_id.as_ref());
        }
        let response = request.json(&message).send().await?;
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) && let Some(challenge) = response.headers().get(WWW_AUTHENTICATE)
        {
            let challenge = challenge
                .to_str()
                .map_err(|_| unexpected("invalid www-authenticate header value".to_owned()))?
                .to_owned();
            return Err(if status == reqwest::StatusCode::UNAUTHORIZED {
                StreamableHttpError::AuthRequired(AuthRequiredError::new(challenge))
            } else {
                StreamableHttpError::InsufficientScope(InsufficientScopeError::new(challenge, None))
            });
        }
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
        let session = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let empty = response.content_length() == Some(0);
        let body = read_response_body(response, self.limit)
            .await
            .map_err(|error| unexpected(error.to_string()))?;
        // An empty success to a message that awaits no answer is accepted, as
        // some servers send 200 where the protocol says 202.
        if status.is_success() && empty && !matches!(message, ClientJsonRpcMessage::Request(_)) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let is = |kind: &str| {
            content_type
                .as_deref()
                .is_some_and(|value| value.starts_with(kind))
        };
        if !status.is_success() {
            let text = String::from_utf8_lossy(&body);
            if is(JSON)
                && let Ok(error @ JsonRpcMessage::Error(_)) =
                    serde_json::from_slice::<ServerJsonRpcMessage>(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(error, session));
            }
            if let Some(refused) = refused_discovery(&message, session_was_attached, status, &text)
            {
                return Ok(refused);
            }
            return Err(unexpected(format!("HTTP {status}: {text}")));
        }
        if is(EVENT_STREAM) {
            let events = SseStream::new(Full::new(Bytes::from(body)));
            return Ok(StreamableHttpPostResponse::Sse(Box::pin(events), session));
        }
        if is(JSON) {
            // A body that is no JSON-RPC message, such as a 200 to a
            // notification, is accepted, as rmcp does.
            return Ok(
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    Ok(message) => StreamableHttpPostResponse::Json(message, session),
                    Err(_) => StreamableHttpPostResponse::Accepted,
                },
            );
        }
        Err(StreamableHttpError::UnexpectedContentType(content_type))
    }
}

/// A server that predates `server/discover` refuses it with a 4xx; the client
/// then falls back to `initialize`, so the refusal answers as an error.
fn refused_discovery(
    message: &ClientJsonRpcMessage,
    session_was_attached: bool,
    status: reqwest::StatusCode,
    body: &str,
) -> Option<StreamableHttpPostResponse> {
    if session_was_attached
        || !status.is_client_error()
        || matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        )
    {
        return None;
    }
    let ClientJsonRpcMessage::Request(request) = message else {
        return None;
    };
    if !matches!(request.request, ClientRequest::DiscoverRequest(_)) {
        return None;
    }
    let error = ErrorData::invalid_request(
        format!("server/discover rejected with HTTP {status}: {body}"),
        None,
    );
    Some(StreamableHttpPostResponse::Json(
        ServerJsonRpcMessage::error(error, Some(request.id.clone())),
        None,
    ))
}

fn unexpected(message: String) -> StreamableHttpError<reqwest::Error> {
    StreamableHttpError::UnexpectedServerResponse(Cow::Owned(message))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage};
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
    };

    use super::BoundedMcpClient;
    use crate::http::{ReqwestHttpTransport, http_runtime};

    /// The answer a one-shot server gives a ping, streamed with no length.
    fn answer(body: &'static str) -> Result<String, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        std::thread::spawn(move || -> std::io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer)?;
            stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
            )?;
            stream.write_all(body.as_bytes())
        });
        Ok(format!("http://{address}/mcp"))
    }

    #[test]
    fn a_remote_answer_longer_than_the_limit_is_refused() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = BoundedMcpClient {
            client: ReqwestHttpTransport::with_private_network_access_for_tests()?.client,
            limit: 64,
        };
        let ping = || {
            serde_json::from_str::<ClientJsonRpcMessage>(
                r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            )
        };
        let post = |url: String, message: ClientJsonRpcMessage| {
            http_runtime().map(|runtime| {
                runtime.block_on(client.post_message(
                    url.into(),
                    message,
                    None,
                    None,
                    HashMap::new(),
                ))
            })
        };

        let small = post(answer(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)?, ping()?)?;
        assert!(matches!(
            small,
            Ok(StreamableHttpPostResponse::Json(
                JsonRpcMessage::Response(_),
                _
            ))
        ));
        let large = post(
            answer(
                r#"{"jsonrpc":"2.0","id":1,"result":{"padding":"this answer is longer than sixty-four bytes"}}"#,
            )?,
            ping()?,
        )?;
        assert!(
            matches!(&large, Err(StreamableHttpError::UnexpectedServerResponse(reason)) if reason.contains("64")),
            "{large:?}"
        );
        Ok(())
    }
}
