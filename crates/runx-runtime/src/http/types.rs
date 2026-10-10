use std::fmt;

use runx_contracts::JsonValue;

use super::RuntimeHttpError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RuntimeHttpHeader {
    pub name: String,
    pub value: String,
    sensitive: bool,
}

impl RuntimeHttpHeader {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            sensitive: false,
        }
    }

    pub(crate) fn is_sensitive(&self) -> bool {
        self.sensitive || sensitive_header_name(&self.name)
    }

    /// Material presented as authentication is secret regardless of its name.
    pub(crate) fn secret(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            sensitive: true,
        }
    }
}

impl fmt::Debug for RuntimeHttpHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeHttpHeader")
            .field("name", &self.name)
            .field(
                "value",
                &if self.is_sensitive() {
                    "[redacted]"
                } else {
                    self.value.as_str()
                },
            )
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RuntimeHttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<RuntimeHttpHeader>,
    pub body: Option<String>,
}

impl fmt::Debug for RuntimeHttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeHttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field(
                "body",
                &self.body.as_ref().map(|_| "[redacted body present]"),
            )
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RuntimeHttpResponse {
    pub status: u16,
    pub body: String,
    pub headers: Vec<RuntimeHttpHeader>,
    pub body_digest: String,
    pub body_bytes: usize,
}

/// One request-sensitive response available only through the private harness
/// registry. Live transports never construct or consume this type.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RuntimeHarnessHttpExchange {
    pub method: HttpMethod,
    pub url: String,
    pub body: RuntimeHarnessHttpRequestBody,
    pub response: RuntimeHttpResponse,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RuntimeHarnessHttpRequestBody {
    None,
    Json(JsonValue),
}

impl RuntimeHttpResponse {
    #[must_use]
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        let body = body.into();
        Self {
            status,
            body_digest: runx_contracts::sha256_prefixed(body.as_bytes()),
            body_bytes: body.len(),
            body,
            headers: Vec::new(),
        }
    }
}

impl fmt::Debug for RuntimeHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names = self
            .headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>();
        formatter
            .debug_struct("RuntimeHttpResponse")
            .field("status", &self.status)
            .field("body", &format_args!("{} bytes", self.body.len()))
            .field("header_names", &header_names)
            .field("body_digest", &self.body_digest)
            .field("body_bytes", &self.body_bytes)
            .finish()
    }
}

pub trait RuntimeHttpTransport {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError>;

    fn send_limited(
        &self,
        request: RuntimeHttpRequest,
        response_limit: usize,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        enforce_response_limit(self.send(request)?, response_limit)
    }

    fn send_idempotent(
        &self,
        request: RuntimeHttpRequest,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.send(request)
    }

    fn send_idempotent_limited(
        &self,
        request: RuntimeHttpRequest,
        response_limit: usize,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        enforce_response_limit(self.send_idempotent(request)?, response_limit)
    }
}

fn enforce_response_limit(
    response: RuntimeHttpResponse,
    response_limit: usize,
) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
    if response.body_bytes > response_limit {
        return Err(RuntimeHttpError::ResponseBodyTooLarge {
            limit: response_limit,
        });
    }
    Ok(response)
}

#[derive(Clone, Debug)]
pub struct ReqwestHttpTransport {
    pub(super) client: reqwest::Client,
    pub(super) allow_private_networks: bool,
    pub(super) request_timeout: std::time::Duration,
}

/// Only the selected local model endpoint is reachable through this transport.
#[derive(Clone, Debug)]
pub struct ExactLoopbackHttpTransport {
    pub(super) inner: ReqwestHttpTransport,
    pub(super) endpoint: String,
}

pub(crate) fn sensitive_header_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    normalized == "authorization"
        || normalized == "proxy-authorization"
        || normalized == "cookie"
        || normalized == "set-cookie"
        || normalized.contains("token")
        || normalized.contains("secret")
        || normalized.contains("api-key")
}
