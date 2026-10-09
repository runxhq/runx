// Module rationale: the runtime HTTP transport keeps
// reqwest wiring, SSRF guards, response limits, and security-focused unit tests
// in one review unit.
mod types;

#[cfg(feature = "mcp")]
use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use url::Url;

pub(crate) use self::types::RuntimeHarnessHttpExchange;
pub(crate) use self::types::RuntimeHarnessHttpRequestBody;
pub use self::types::{
    ExactLoopbackHttpTransport, HttpMethod, ReqwestHttpTransport, RuntimeHttpHeader,
    RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};

const DEFAULT_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MANAGED_AGENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
/// Provider operations may contain one adapter-owned synchronous budget of up
/// to 120 seconds. Keep the control-plane envelope above that budget so the
/// caller never abandons a still-running, idempotently recoverable effect.
pub(crate) const PROVIDER_OPERATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_SAFE_READ_ATTEMPTS: usize = 3;
const DEFAULT_SAFE_READ_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_SAFE_READ_RETRY_DELAY: Duration = Duration::from_secs(2);
static HTTP_CLIENT_RUNTIME: RetryableCell<tokio::runtime::Runtime> = RetryableCell::new();
const HTTP_CLIENT_PROFILE_COUNT: usize = 6;
/// rmcp's streamable HTTP client, carried by the guarded client.
#[cfg(feature = "mcp")]
pub(crate) type McpTransport = rmcp::transport::StreamableHttpClientTransport<reqwest::Client>;

static HTTP_CLIENTS: [RetryableCell<reqwest::Client>; HTTP_CLIENT_PROFILE_COUNT] =
    [const { RetryableCell::new() }; HTTP_CLIENT_PROFILE_COUNT];

struct RetryableCell<T> {
    value: OnceLock<T>,
    initialize: Mutex<()>,
}

impl<T> RetryableCell<T> {
    const fn new() -> Self {
        Self {
            value: OnceLock::new(),
            initialize: Mutex::new(()),
        }
    }

    fn get_or_try_init<E>(&self, initialize: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
        if let Some(value) = self.value.get() {
            return Ok(value);
        }
        let _guard = self
            .initialize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(value) = self.value.get() {
            return Ok(value);
        }
        let value = initialize()?;
        Ok(self.value.get_or_init(|| value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum TransportProfile {
    PublicStandard = 0,
    PrivateStandard = 1,
    PublicPatient = 2,
    PublicProviderOperation = 3,
    PrivateProviderOperation = 4,
    ExactLoopbackAgent = 5,
}

#[derive(Clone, Copy)]
struct TransportConfig {
    request_timeout: Duration,
    connect_timeout: Duration,
    allow_private_networks: bool,
    no_proxy: bool,
}

impl TransportProfile {
    const fn config(self) -> TransportConfig {
        match self {
            Self::PublicStandard => TransportConfig {
                request_timeout: DEFAULT_HTTP_REQUEST_TIMEOUT,
                connect_timeout: DEFAULT_HTTP_CONNECT_TIMEOUT,
                allow_private_networks: false,
                no_proxy: false,
            },
            Self::PrivateStandard => TransportConfig {
                request_timeout: DEFAULT_HTTP_REQUEST_TIMEOUT,
                connect_timeout: DEFAULT_HTTP_CONNECT_TIMEOUT,
                allow_private_networks: true,
                no_proxy: false,
            },
            Self::PublicPatient => TransportConfig {
                request_timeout: MANAGED_AGENT_REQUEST_TIMEOUT,
                connect_timeout: DEFAULT_HTTP_CONNECT_TIMEOUT,
                allow_private_networks: false,
                no_proxy: false,
            },
            Self::PublicProviderOperation => TransportConfig {
                request_timeout: PROVIDER_OPERATION_REQUEST_TIMEOUT,
                connect_timeout: DEFAULT_HTTP_CONNECT_TIMEOUT,
                allow_private_networks: false,
                no_proxy: false,
            },
            Self::PrivateProviderOperation => TransportConfig {
                request_timeout: PROVIDER_OPERATION_REQUEST_TIMEOUT,
                connect_timeout: DEFAULT_HTTP_CONNECT_TIMEOUT,
                allow_private_networks: true,
                no_proxy: false,
            },
            Self::ExactLoopbackAgent => TransportConfig {
                request_timeout: MANAGED_AGENT_REQUEST_TIMEOUT,
                connect_timeout: DEFAULT_HTTP_CONNECT_TIMEOUT,
                allow_private_networks: true,
                no_proxy: true,
            },
        }
    }

    fn cache(self) -> &'static RetryableCell<reqwest::Client> {
        &HTTP_CLIENTS[self as usize]
    }
}

impl ReqwestHttpTransport {
    pub fn new() -> Result<Self, RuntimeHttpError> {
        Self::from_profile(TransportProfile::PublicStandard)
    }

    fn from_profile(profile: TransportProfile) -> Result<Self, RuntimeHttpError> {
        let config = profile.config();
        let client = profile
            .cache()
            .get_or_try_init(|| build_http_client(config))
            .map_err(|message| RuntimeHttpError::Transport { message })?
            .clone();
        Ok(Self {
            client,
            allow_private_networks: config.allow_private_networks,
            request_timeout: config.request_timeout,
        })
    }

    #[cfg(test)]
    fn uncached(
        request_timeout: Duration,
        connect_timeout: Duration,
        allow_private_networks: bool,
    ) -> Result<Self, RuntimeHttpError> {
        let client = build_http_client(TransportConfig {
            request_timeout,
            connect_timeout,
            allow_private_networks,
            no_proxy: false,
        })
        .map_err(|message| RuntimeHttpError::Transport { message })?;
        Ok(Self {
            client,
            allow_private_networks,
            request_timeout,
        })
    }

    /// Build a transport that may reach private or loopback networks. This is the
    /// explicit, opt-in escape from the default SSRF/private-network block; callers
    /// must require an operator-declared opt-in before choosing it, never as a
    /// default.
    pub fn with_private_network_access() -> Result<Self, RuntimeHttpError> {
        Self::from_profile(TransportProfile::PrivateStandard)
    }

    /// Build the model-provider transport for managed-agent calls. These calls can
    /// legitimately take longer than the generic governed HTTP timeout while the
    /// provider thinks and emits tool use, but they still keep the same public-DNS
    /// guard and short connect timeout.
    pub fn for_managed_agent() -> Result<Self, RuntimeHttpError> {
        Self::from_profile(TransportProfile::PublicPatient)
    }

    /// Construct a local model transport whose request URL is checked again at
    /// every send. It never inherits an HTTP proxy or follows redirects.
    pub fn for_exact_loopback_agent(
        endpoint: &str,
    ) -> Result<ExactLoopbackHttpTransport, RuntimeHttpError> {
        validate_exact_loopback_agent_endpoint(endpoint)?;
        Ok(ExactLoopbackHttpTransport {
            inner: Self::from_profile(TransportProfile::ExactLoopbackAgent)?,
            endpoint: endpoint.to_owned(),
        })
    }

    /// Build the transport used only for authenticated provider operations.
    /// The adapter owns its narrower operation deadline; this envelope prevents
    /// the control-plane request from timing out first and creating an unknown
    /// outcome for an effect that may still settle successfully.
    pub fn for_provider_operation(allow_private_networks: bool) -> Result<Self, RuntimeHttpError> {
        Self::from_profile(if allow_private_networks {
            TransportProfile::PrivateProviderOperation
        } else {
            TransportProfile::PublicProviderOperation
        })
    }

    /// The deadline of each request the guarded client sends.
    #[cfg(feature = "mcp")]
    pub(crate) const fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    /// An MCP client transport to `url` carried by the guarded client, for
    /// rmcp to drive its own requests. The URL is held to the same network
    /// policy as every other request first: the DNS guard never sees a
    /// literal address. It starts a task, so it is built on the HTTP runtime.
    #[cfg(feature = "mcp")]
    pub(crate) fn mcp(
        &self,
        url: &str,
        bearer: Option<&str>,
        headers: &BTreeMap<String, String>,
    ) -> Result<McpTransport, RuntimeHttpError> {
        validate_http_url(url, self.allow_private_networks)?;
        let custom_headers = headers
            .iter()
            .map(|(name, value)| {
                Ok((
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                        RuntimeHttpError::InvalidHeaderName {
                            name: name.clone(),
                            message: error.to_string(),
                        }
                    })?,
                    reqwest::header::HeaderValue::from_str(value).map_err(|error| {
                        RuntimeHttpError::InvalidHeaderValue {
                            name: name.clone(),
                            message: error.to_string(),
                        }
                    })?,
                ))
            })
            .collect::<Result<std::collections::HashMap<_, _>, RuntimeHttpError>>()?;
        let config =
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                url.to_owned(),
            )
            .custom_headers(custom_headers);
        let config = match bearer {
            Some(token) => config.auth_header(token.to_owned()),
            None => config,
        };
        Ok(McpTransport::with_client(self.client.clone(), config))
    }

    #[cfg(test)]
    fn with_private_network_access_for_tests() -> Result<Self, RuntimeHttpError> {
        Self::with_private_network_access()
    }

    #[cfg(test)]
    fn with_private_network_timeouts_for_tests(
        request_timeout: Duration,
        connect_timeout: Duration,
    ) -> Result<Self, RuntimeHttpError> {
        Self::uncached(request_timeout, connect_timeout, true)
    }
}

/// Runtime-owned transport selection for native HTTP capabilities. Exact
/// harness exchanges can enter only through the private harness registry;
/// ordinary executions always construct the live guarded transport.
pub(crate) enum NativeHttpTransport<'a> {
    Live(ReqwestHttpTransport),
    Harness(&'a [RuntimeHarnessHttpExchange]),
}

impl<'a> NativeHttpTransport<'a> {
    #[cfg(test)]
    pub(crate) fn new(
        harness: Option<&'a [RuntimeHarnessHttpExchange]>,
    ) -> Result<Self, RuntimeHttpError> {
        match harness {
            Some(exchanges) => Ok(Self::Harness(exchanges)),
            None => Ok(Self::Live(ReqwestHttpTransport::new()?)),
        }
    }

    pub(crate) fn for_hosted_api(
        harness: Option<&'a [RuntimeHarnessHttpExchange]>,
        allow_private_network: bool,
    ) -> Result<Self, RuntimeHttpError> {
        match harness {
            Some(exchanges) => Ok(Self::Harness(exchanges)),
            None if allow_private_network => Ok(Self::Live(
                ReqwestHttpTransport::with_private_network_access()?,
            )),
            None => Ok(Self::Live(ReqwestHttpTransport::new()?)),
        }
    }
}

impl RuntimeHttpTransport for NativeHttpTransport<'_> {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        match self {
            Self::Live(transport) => transport.send(request),
            Self::Harness(exchanges) => exact_harness_response(exchanges, &request),
        }
    }

    fn send_limited(
        &self,
        request: RuntimeHttpRequest,
        response_limit: usize,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        match self {
            Self::Live(transport) => transport.send_limited(request, response_limit),
            Self::Harness(exchanges) => enforce_harness_response_limit(
                exact_harness_response(exchanges, &request)?,
                response_limit,
            ),
        }
    }

    fn send_idempotent(
        &self,
        request: RuntimeHttpRequest,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        match self {
            Self::Live(transport) => transport.send_idempotent(request),
            Self::Harness(exchanges) => exact_harness_response(exchanges, &request),
        }
    }

    fn send_idempotent_limited(
        &self,
        request: RuntimeHttpRequest,
        response_limit: usize,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        match self {
            Self::Live(transport) => transport.send_idempotent_limited(request, response_limit),
            Self::Harness(exchanges) => enforce_harness_response_limit(
                exact_harness_response(exchanges, &request)?,
                response_limit,
            ),
        }
    }
}

/// The harness's answer to `request`: the one exchange whose method, URL and
/// body match it exactly. Anything else is refused, never sent.
pub(crate) fn exact_harness_response(
    exchanges: &[RuntimeHarnessHttpExchange],
    request: &RuntimeHttpRequest,
) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
    exchanges
        .iter()
        .find(|exchange| {
            exchange.method == request.method
                && exchange.url == request.url
                && harness_request_body_matches(&exchange.body, request.body.as_deref())
        })
        .map(|exchange| exchange.response.clone())
        .ok_or_else(|| RuntimeHttpError::Transport {
            message: format!(
                "the harness declares no exchange for {} {}",
                request.method.as_str(),
                request.url
            ),
        })
}

fn harness_request_body_matches(
    expected: &RuntimeHarnessHttpRequestBody,
    actual: Option<&str>,
) -> bool {
    match expected {
        RuntimeHarnessHttpRequestBody::None => actual.is_none(),
        RuntimeHarnessHttpRequestBody::Json(expected) => actual
            .and_then(|body| serde_json::from_str::<runx_contracts::JsonValue>(body).ok())
            .is_some_and(|actual| actual == *expected),
    }
}

fn enforce_harness_response_limit(
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

fn build_http_client(config: TransportConfig) -> Result<reqwest::Client, String> {
    // reqwest is built with `rustls-no-provider`, so the process needs a
    // default crypto provider before a TLS client can be constructed.
    // Install ring once; an Err means another transport already set it.
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Decode like a browser (the decoders also advertise the matching
    // Accept-Encoding) and let ALPN negotiate HTTP/2; a no-compression,
    // http1-only client is a bot tell. The response cap measures DECODED
    // bytes (read_limited_response_body), so a decompression bomb stays bounded.
    let builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(config.request_timeout)
        .connect_timeout(config.connect_timeout)
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .zstd(true)
        // Verify against the compiled-in Mozilla root program instead of the
        // host trust store. The governed front then has one deterministic
        // trust surface everywhere it runs; a sandbox with no system CA
        // bundle (hosted publish harness) behaves exactly like a developer
        // laptop.
        .tls_certs_only(
            webpki_root_certs::TLS_SERVER_ROOT_CERTS
                .iter()
                .filter_map(|der| reqwest::Certificate::from_der(der).ok()),
        );
    let builder = if config.no_proxy {
        builder.no_proxy()
    } else {
        builder
    };
    let builder = if config.allow_private_networks {
        builder
    } else {
        builder.dns_resolver(GuardedDnsResolver::new(TokioDnsResolver))
    };
    builder
        .build()
        .map_err(|error| transport_error_message(&error))
}

impl RuntimeHttpTransport for ReqwestHttpTransport {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.send_with_limit(request, None, false)
    }

    fn send_limited(
        &self,
        request: RuntimeHttpRequest,
        response_limit: usize,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.send_with_limit(request, Some(response_limit), false)
    }

    fn send_idempotent(
        &self,
        request: RuntimeHttpRequest,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.send_with_limit(request, None, true)
    }

    fn send_idempotent_limited(
        &self,
        request: RuntimeHttpRequest,
        response_limit: usize,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        self.send_with_limit(request, Some(response_limit), true)
    }
}

impl RuntimeHttpTransport for ExactLoopbackHttpTransport {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        if request.method != HttpMethod::Post
            || request.url != self.endpoint
            || validate_exact_loopback_agent_endpoint(&request.url).is_err()
        {
            return Err(RuntimeHttpError::Transport {
                message: "local model request exceeded its exact endpoint binding".to_owned(),
            });
        }
        self.inner.send(request)
    }
}

impl ReqwestHttpTransport {
    /// Send and read the whole body, refusing one longer than `response_limit`
    /// when a limit is given. A body is never clipped.
    fn send_with_limit(
        &self,
        request: RuntimeHttpRequest,
        response_limit: Option<usize>,
        retry_as_idempotent: bool,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        validate_http_url(&request.url, self.allow_private_networks)?;
        let client = self.client.clone();
        let request_timeout = self.request_timeout;
        let headers = reqwest_headers(&request.headers)?;
        block_on_http(async move {
            tokio::time::timeout(
                request_timeout,
                send_reqwest_with_safe_read_retries(
                    client,
                    request,
                    headers,
                    response_limit,
                    retry_as_idempotent,
                ),
            )
            .await
            .map_err(|_| RuntimeHttpError::Transport {
                message: format!(
                    "request deadline exceeded after {}ms",
                    request_timeout.as_millis()
                ),
            })?
        })
    }
}

fn reqwest_headers(
    headers: &[RuntimeHttpHeader],
) -> Result<reqwest::header::HeaderMap, RuntimeHttpError> {
    let mut output = reqwest::header::HeaderMap::new();
    for header in headers {
        validate_header(header)?;
        let name = reqwest::header::HeaderName::from_bytes(header.name.trim().as_bytes()).map_err(
            |error| RuntimeHttpError::InvalidHeaderName {
                name: header.name.clone(),
                message: error.to_string(),
            },
        )?;
        let value = reqwest::header::HeaderValue::from_str(&header.value).map_err(|error| {
            RuntimeHttpError::InvalidHeaderValue {
                name: header.name.clone(),
                message: error.to_string(),
            }
        })?;
        output.insert(name, value);
    }
    Ok(output)
}

async fn send_reqwest_with_safe_read_retries(
    client: reqwest::Client,
    request: RuntimeHttpRequest,
    headers: reqwest::header::HeaderMap,
    response_limit: Option<usize>,
    retry_as_idempotent: bool,
) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
    let mut attempt = 1_usize;
    loop {
        let mut builder = client
            .request(reqwest_method(request.method), &request.url)
            .headers(headers.clone());
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        let response = builder
            .send()
            .await
            .map_err(|error| RuntimeHttpError::Transport {
                message: transport_error_message(&error),
            })?;
        let status = response.status().as_u16();
        if (request.method == HttpMethod::Get || retry_as_idempotent)
            && retryable_read_status(status)
            && attempt < MAX_SAFE_READ_ATTEMPTS
        {
            let delay = safe_read_retry_delay(&response, attempt);
            drop(response);
            tokio::time::sleep(delay).await;
            attempt += 1;
            continue;
        }
        let response_headers = safe_response_headers(&response);
        let body = read_response_body(response, response_limit).await?;
        let body_bytes = body.len();
        let body_digest = runx_contracts::sha256_prefixed(&body);
        return Ok(RuntimeHttpResponse {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
            headers: response_headers,
            body_digest,
            body_bytes,
        });
    }
}

fn safe_response_headers(response: &reqwest::Response) -> Vec<RuntimeHttpHeader> {
    response
        .headers()
        .iter()
        .filter(|(name, _)| !types::sensitive_header_name(name.as_str()))
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| RuntimeHttpHeader::new(name.as_str(), value))
        })
        .collect()
}

fn retryable_read_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

fn safe_read_retry_delay(response: &reqwest::Response, completed_attempts: usize) -> Duration {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after)
        .unwrap_or_else(|| {
            DEFAULT_SAFE_READ_RETRY_DELAY
                .saturating_mul(u32::try_from(completed_attempts).unwrap_or(u32::MAX))
        })
        .min(MAX_SAFE_READ_RETRY_DELAY)
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let deadline = httpdate::parse_http_date(value).ok()?;
    Some(
        deadline
            .duration_since(std::time::SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

fn transport_error_message(error: &(dyn StdError + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        parts.push(error.to_string());
        source = error.source();
    }
    parts.dedup();
    parts.join(": ")
}

#[derive(Clone, Debug)]
struct GuardedDnsResolver<R> {
    inner: R,
}

impl<R> GuardedDnsResolver<R> {
    fn new(inner: R) -> Self {
        Self { inner }
    }
}

impl<R> reqwest::dns::Resolve for GuardedDnsResolver<R>
where
    R: reqwest::dns::Resolve + Clone + Send + Sync + 'static,
{
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let inner = self.inner.clone();
        Box::pin(async move {
            let addrs = inner.resolve(name).await?;
            let mut public_addrs = Vec::new();
            for addr in addrs {
                if is_private_network_ip(addr.ip()) {
                    return Err(PrivateDnsResolutionError { host, addr }.into());
                }
                public_addrs.push(addr);
            }
            if public_addrs.is_empty() {
                return Err(EmptyDnsResolutionError { host }.into());
            }
            Ok(Box::new(public_addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct TokioDnsResolver;

impl reqwest::dns::Resolve for TokioDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn StdError + Send + Sync>)?;
            let addrs = addrs.collect::<Vec<_>>();
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[derive(Debug)]
struct PrivateDnsResolutionError {
    host: String,
    addr: SocketAddr,
}

impl fmt::Display for PrivateDnsResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime HTTP DNS resolved '{}' to non-public address {}",
            self.host, self.addr
        )
    }
}

impl StdError for PrivateDnsResolutionError {}

#[derive(Debug)]
struct EmptyDnsResolutionError {
    host: String,
}

impl fmt::Display for EmptyDnsResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime HTTP DNS returned no addresses for '{}'",
            self.host
        )
    }
}

impl StdError for EmptyDnsResolutionError {}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeHttpError {
    #[error("invalid runtime HTTP url: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("runtime HTTP transport failed: {message}")]
    Transport { message: String },
    #[error("runtime HTTP transport cannot block inside an active async runtime")]
    BlockingHttpInsideAsyncRuntime,
    #[error("runtime HTTP async runtime is unavailable: {message}")]
    AsyncRuntimeUnavailable { message: String },
    #[error("runtime HTTP transport returned invalid output: {message}")]
    TransportDecode { message: String },
    #[error("runtime HTTP response body exceeds {limit} byte limit")]
    ResponseBodyTooLarge { limit: usize },
    #[error("unsupported runtime HTTP url scheme '{scheme}': only http and https are allowed")]
    UnsupportedUrlScheme { scheme: String },
    #[error("runtime HTTP url host '{host}' is not publicly routable")]
    PrivateNetworkUrl { host: String },
    #[error("invalid runtime HTTP header name '{name}': {message}")]
    InvalidHeaderName { name: String, message: String },
    #[error("invalid runtime HTTP header value for '{name}': {message}")]
    InvalidHeaderValue { name: String, message: String },
}

pub(crate) fn strip_one_trailing_slash(value: &str) -> String {
    value.strip_suffix('/').unwrap_or(value).to_owned()
}

fn validate_header(header: &RuntimeHttpHeader) -> Result<(), RuntimeHttpError> {
    let name = header.name.trim();
    if name.is_empty() || !name.bytes().all(is_header_token_byte) {
        return Err(RuntimeHttpError::InvalidHeaderName {
            name: header.name.clone(),
            message: "header names must be HTTP token characters".to_owned(),
        });
    }
    if header.value.contains('\r') || header.value.contains('\n') {
        return Err(RuntimeHttpError::InvalidHeaderValue {
            name: header.name.clone(),
            message: "header values must not contain line breaks".to_owned(),
        });
    }
    Ok(())
}

fn validate_http_url(value: &str, allow_private_networks: bool) -> Result<(), RuntimeHttpError> {
    let url = Url::parse(value)?;
    match url.scheme() {
        "http" | "https" => validate_public_host(&url, allow_private_networks),
        scheme => Err(RuntimeHttpError::UnsupportedUrlScheme {
            scheme: scheme.to_owned(),
        }),
    }
}

/// Validate the complete, canonical local chat-completions endpoint. A hostname,
/// alternate loopback address, path, credential, query or fragment is excluded.
pub(crate) fn validate_exact_loopback_agent_endpoint(value: &str) -> Result<(), RuntimeHttpError> {
    let url = Url::parse(value)?;
    let loopback = matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip == Ipv4Addr::LOCALHOST)
        || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip == Ipv6Addr::LOCALHOST);
    if url.scheme() != "http"
        || !loopback
        || url.port().is_none()
        || url.path() != "/v1/chat/completions"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.as_str() != value
    {
        return Err(RuntimeHttpError::Transport {
            message: "invalid exact loopback model endpoint".to_owned(),
        });
    }
    Ok(())
}

fn validate_public_host(url: &Url, allow_private_networks: bool) -> Result<(), RuntimeHttpError> {
    if allow_private_networks {
        return Ok(());
    }
    let Some(host) = url.host_str() else {
        return Err(RuntimeHttpError::PrivateNetworkUrl {
            host: "<missing>".to_owned(),
        });
    };
    let normalized = host.trim_end_matches('.').to_ascii_lowercase();
    if normalized == "localhost"
        || normalized.ends_with(".localhost")
        || normalized == "metadata.google.internal"
    {
        return Err(RuntimeHttpError::PrivateNetworkUrl {
            host: host.to_owned(),
        });
    }
    let ip_host = normalized
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(&normalized);
    if let Ok(ip) = ip_host.parse::<IpAddr>()
        && is_private_network_ip(ip)
    {
        return Err(RuntimeHttpError::PrivateNetworkUrl {
            host: host.to_owned(),
        });
    }
    Ok(())
}

fn is_private_network_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_private_network_ipv4(ip),
        IpAddr::V6(ip) => is_private_network_ipv6(ip),
    }
}

fn is_private_network_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || octets[0] == 0
        || (octets[0] == 100 && (octets[1] & 0xc0) == 0x40)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
        || octets[0] >= 240
        || octets == [169, 254, 169, 254]
}

fn is_private_network_ipv6(ip: Ipv6Addr) -> bool {
    ip.to_ipv4_mapped().is_some_and(is_private_network_ipv4)
        || ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || is_unique_local_ipv6(ip)
        || is_unicast_link_local_ipv6(ip)
        || is_documentation_ipv6(ip)
        || nat64_embedded_ipv4(ip).is_some_and(is_private_network_ipv4)
        || six_to_four_embedded_ipv4(ip).is_some_and(is_private_network_ipv4)
}

fn is_unique_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_unicast_link_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_documentation_ipv6(ip: Ipv6Addr) -> bool {
    ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8
}

fn nat64_embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = ip.segments();
    if segments[..6] != [0x0064, 0xff9b, 0, 0, 0, 0] {
        return None;
    }
    Some(Ipv4Addr::new(
        (segments[6] >> 8) as u8,
        segments[6] as u8,
        (segments[7] >> 8) as u8,
        segments[7] as u8,
    ))
}

fn six_to_four_embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = ip.segments();
    if segments[0] != 0x2002 {
        return None;
    }
    Some(Ipv4Addr::new(
        (segments[1] >> 8) as u8,
        segments[1] as u8,
        (segments[2] >> 8) as u8,
        segments[2] as u8,
    ))
}

async fn read_response_body(
    mut response: reqwest::Response,
    limit: Option<usize>,
) -> Result<Vec<u8>, RuntimeHttpError> {
    if let Some(limit) = limit
        && declared_response_length(&response)?.is_some_and(|length| length > limit as u64)
    {
        return Err(RuntimeHttpError::ResponseBodyTooLarge { limit });
    }
    let mut body = Vec::new();
    while let Some(chunk) =
        response
            .chunk()
            .await
            .map_err(|error| RuntimeHttpError::TransportDecode {
                message: error.to_string(),
            })?
    {
        if let Some(limit) = limit
            && body.len().saturating_add(chunk.len()) > limit
        {
            return Err(RuntimeHttpError::ResponseBodyTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn declared_response_length(response: &reqwest::Response) -> Result<Option<u64>, RuntimeHttpError> {
    let Some(value) = response.headers().get(reqwest::header::CONTENT_LENGTH) else {
        return Ok(response.content_length());
    };
    let value = value
        .to_str()
        .map_err(|error| RuntimeHttpError::TransportDecode {
            message: format!("invalid Content-Length header: {error}"),
        })?;
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|error| RuntimeHttpError::TransportDecode {
            message: format!("invalid Content-Length header: {error}"),
        })
}

fn reqwest_method(method: HttpMethod) -> reqwest::Method {
    match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Patch => reqwest::Method::PATCH,
        HttpMethod::Delete => reqwest::Method::DELETE,
    }
}

fn block_on_http<F, T>(future: F) -> Result<T, RuntimeHttpError>
where
    F: std::future::Future<Output = Result<T, RuntimeHttpError>>,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(RuntimeHttpError::BlockingHttpInsideAsyncRuntime);
    }
    http_runtime()?.block_on(future)
}

pub(crate) fn http_runtime() -> Result<&'static tokio::runtime::Runtime, RuntimeHttpError> {
    HTTP_CLIENT_RUNTIME
        .get_or_try_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("runx-http")
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
        })
        .map_err(|message| RuntimeHttpError::AsyncRuntimeUnavailable { message })
}

fn is_header_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::time::Duration;

    use super::RuntimeHttpResponse;
    use super::RuntimeHttpTransport;
    use super::{
        GuardedDnsResolver, NativeHttpTransport, PROVIDER_OPERATION_REQUEST_TIMEOUT,
        ReqwestHttpTransport, TransportProfile, block_on_http, http_runtime,
    };
    use super::{HttpMethod, RuntimeHttpError, RuntimeHttpHeader, RuntimeHttpRequest};
    use super::{RuntimeHarnessHttpExchange, RuntimeHarnessHttpRequestBody};
    use reqwest::dns::Resolve as _;

    #[derive(Clone, Debug)]
    struct StaticDnsResolver {
        addrs: Vec<SocketAddr>,
    }

    impl reqwest::dns::Resolve for StaticDnsResolver {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            let addrs = self.addrs.clone();
            Box::pin(async move { Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs) })
        }
    }

    #[derive(Debug, thiserror::Error)]
    enum RuntimeHttpTestError {
        #[error(transparent)]
        RuntimeHttp(#[from] RuntimeHttpError),
        #[error(transparent)]
        Io(#[from] io::Error),
        #[error("server thread panicked")]
        ServerThread,
    }

    #[test]
    fn harness_http_transport_is_exact_limited_and_never_falls_through()
    -> Result<(), RuntimeHttpTestError> {
        let url = "https://fixture.runx.invalid/source";
        let exchanges = [RuntimeHarnessHttpExchange {
            method: HttpMethod::Get,
            url: url.to_owned(),
            body: RuntimeHarnessHttpRequestBody::None,
            response: RuntimeHttpResponse::new(200, "hello world"),
        }];
        let transport = NativeHttpTransport::new(Some(&exchanges))?;
        let read = || RuntimeHttpRequest {
            method: HttpMethod::Get,
            url: url.to_owned(),
            headers: Vec::new(),
            body: None,
        };

        assert_eq!(transport.send(read())?.body, "hello world");
        assert!(matches!(
            transport.send_limited(read(), 5),
            Err(RuntimeHttpError::ResponseBodyTooLarge { limit: 5 })
        ));

        let missing = transport.send(RuntimeHttpRequest {
            method: HttpMethod::Get,
            url: "https://fixture.runx.invalid/missing".to_owned(),
            headers: Vec::new(),
            body: None,
        });
        assert!(matches!(missing, Err(RuntimeHttpError::Transport { .. })));

        let mutation = transport.send(RuntimeHttpRequest {
            method: HttpMethod::Post,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Some("{}".to_owned()),
        });
        assert!(matches!(mutation, Err(RuntimeHttpError::Transport { .. })));

        let query = transport.send_idempotent(RuntimeHttpRequest {
            method: HttpMethod::Post,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Some("{}".to_owned()),
        });
        assert!(matches!(query, Err(RuntimeHttpError::Transport { .. })));

        Ok(())
    }

    #[test]
    fn harness_http_exchanges_match_complete_requests() -> Result<(), RuntimeHttpTestError> {
        let url = "https://fixture.runx.invalid/mcp";
        let exchanges = [
            RuntimeHarnessHttpExchange {
                method: HttpMethod::Post,
                url: url.to_owned(),
                body: RuntimeHarnessHttpRequestBody::Json(runx_contracts::JsonValue::Object(
                    runx_contracts::JsonObject::from([(
                        "operation".to_owned(),
                        runx_contracts::JsonValue::String("checkout".to_owned()),
                    )]),
                )),
                response: RuntimeHttpResponse::new(200, "exact checkout"),
            },
            RuntimeHarnessHttpExchange {
                method: HttpMethod::Delete,
                url: url.to_owned(),
                body: RuntimeHarnessHttpRequestBody::None,
                response: RuntimeHttpResponse::new(204, ""),
            },
            RuntimeHarnessHttpExchange {
                method: HttpMethod::Delete,
                url: url.to_owned(),
                body: RuntimeHarnessHttpRequestBody::Json(runx_contracts::JsonValue::Null),
                response: RuntimeHttpResponse::new(200, "json null"),
            },
        ];
        let exact_transport = NativeHttpTransport::new(Some(&exchanges))?;
        let exact_mutation = exact_transport.send(RuntimeHttpRequest {
            method: HttpMethod::Post,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Some(r#"{"operation":"checkout"}"#.to_owned()),
        })?;
        assert_eq!(exact_mutation.body, "exact checkout");
        let wrong_mutation = exact_transport.send(RuntimeHttpRequest {
            method: HttpMethod::Post,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Some(r#"{"operation":"status"}"#.to_owned()),
        });
        assert!(matches!(
            wrong_mutation,
            Err(RuntimeHttpError::Transport { .. })
        ));
        let bodyless = exact_transport.send(RuntimeHttpRequest {
            method: HttpMethod::Delete,
            url: url.to_owned(),
            headers: Vec::new(),
            body: None,
        })?;
        assert_eq!(bodyless.status, 204);
        let json_null = exact_transport.send(RuntimeHttpRequest {
            method: HttpMethod::Delete,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Some("null".to_owned()),
        })?;
        assert_eq!(json_null.body, "json null");
        Ok(())
    }

    #[test]
    fn debug_output_redacts_sensitive_header_values() {
        let request = RuntimeHttpRequest {
            method: HttpMethod::Get,
            url: "https://api.example/v1/grants".to_owned(),
            headers: vec![
                RuntimeHttpHeader::new("authorization", "Bearer SECRET_RUNTIME_TOKEN"),
                RuntimeHttpHeader::new("x-runx-token", "SECRET_HEADER_TOKEN"),
                RuntimeHttpHeader::new("accept", "application/json"),
            ],
            body: Some("SECRET_BODY".to_owned()),
        };

        let debug = format!("{request:?}");
        assert!(!debug.contains("SECRET_RUNTIME_TOKEN"));
        assert!(!debug.contains("SECRET_HEADER_TOKEN"));
        assert!(!debug.contains("SECRET_BODY"));
        assert!(debug.contains("[redacted]"));
        assert!(debug.contains("application/json"));
        assert!(super::types::sensitive_header_name("set-cookie"));
        assert!(super::types::sensitive_header_name("cookie"));
    }

    #[test]
    fn invalid_base_urls_fail_closed() {
        assert!(super::validate_http_url("not a url", false).is_err());
        assert!(matches!(
            super::validate_http_url("file:///tmp/runx.sock", false),
            Err(RuntimeHttpError::UnsupportedUrlScheme { .. })
        ));
    }

    #[test]
    fn local_model_endpoint_requires_a_literal_exact_loopback_url() {
        for allowed in [
            "http://127.0.0.1:18081/v1/chat/completions",
            "http://[::1]:18081/v1/chat/completions",
        ] {
            assert!(super::validate_exact_loopback_agent_endpoint(allowed).is_ok());
        }
        for denied in [
            "http://localhost:18081/v1/chat/completions",
            "http://127.0.0.2:18081/v1/chat/completions",
            "http://127.0.0.1/v1/chat/completions",
            "http://127.0.0.1:18081/v1/chat/completions?x=1",
            "http://127.0.0.1:18081/v1/models",
            "http://user@127.0.0.1:18081/v1/chat/completions",
        ] {
            assert!(
                super::validate_exact_loopback_agent_endpoint(denied).is_err(),
                "{denied}"
            );
        }
    }

    #[test]
    fn local_model_transport_rechecks_every_request() -> Result<(), RuntimeHttpTestError> {
        let endpoint = "http://127.0.0.1:18081/v1/chat/completions";
        let transport = ReqwestHttpTransport::for_exact_loopback_agent(endpoint)?;
        for (method, url) in [
            (HttpMethod::Get, endpoint),
            (HttpMethod::Post, "http://127.0.0.1:18081/v1/models"),
            (
                HttpMethod::Post,
                "http://127.0.0.2:18081/v1/chat/completions",
            ),
        ] {
            assert!(matches!(
                transport.send(RuntimeHttpRequest {
                    method,
                    url: url.to_owned(),
                    headers: Vec::new(),
                    body: None,
                }),
                Err(RuntimeHttpError::Transport { .. })
            ));
        }
        Ok(())
    }

    #[test]
    fn private_network_base_urls_fail_closed() {
        for value in [
            "http://localhost",
            "http://service.localhost",
            "http://127.0.0.1",
            "http://10.0.0.1",
            "http://172.16.0.1",
            "http://192.168.0.1",
            "http://169.254.169.254",
            "http://100.64.0.1",
            "http://100.127.255.255",
            "http://192.0.0.1",
            "http://198.18.0.1",
            "http://240.0.0.1",
            "http://0.1.2.3",
            "http://[::1]",
            "http://[::ffff:127.0.0.1]",
            "http://[64:ff9b::7f00:1]",
            "http://[2002:7f00:1::]",
            "http://[fc00::1]",
            "http://[fe80::1]",
            "http://metadata.google.internal",
        ] {
            assert!(
                matches!(
                    super::validate_http_url(value, false),
                    Err(RuntimeHttpError::PrivateNetworkUrl { .. })
                ),
                "{value} should be rejected as private"
            );
        }
    }

    #[test]
    fn public_base_urls_are_allowed() -> Result<(), RuntimeHttpTestError> {
        super::validate_http_url("https://api.example", false)?;
        super::validate_http_url("http://8.8.8.8", false)?;
        super::validate_http_url("http://[64:ff9b::808:808]", false)?;
        Ok(())
    }

    #[test]
    fn synchronous_http_reuses_one_runtime() -> Result<(), RuntimeHttpTestError> {
        let first = http_runtime()?;
        let second = http_runtime()?;
        assert!(std::ptr::eq(first, second));
        Ok(())
    }

    #[test]
    fn retryable_cell_does_not_memoize_transient_initialization_failure() {
        let attempts = std::cell::Cell::new(0);
        let cell = super::RetryableCell::new();
        let first: Result<&u8, &str> = cell.get_or_try_init(|| {
            attempts.set(attempts.get() + 1);
            Err("transient")
        });
        let second: Result<&u8, &str> = cell.get_or_try_init(|| {
            attempts.set(attempts.get() + 1);
            Ok(7)
        });
        let third: Result<&u8, &str> = cell.get_or_try_init(|| {
            attempts.set(attempts.get() + 1);
            Ok(8)
        });

        assert_eq!(first, Err("transient"));
        assert_eq!(second, Ok(&7));
        assert_eq!(third, Ok(&7));
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn canonical_http_profiles_populate_distinct_policy_cache_slots()
    -> Result<(), RuntimeHttpTestError> {
        let _public_first = ReqwestHttpTransport::new()?;
        let _public_second = ReqwestHttpTransport::new()?;
        let _private_first = ReqwestHttpTransport::with_private_network_access()?;
        let _private_second = ReqwestHttpTransport::with_private_network_access()?;
        let _managed_first = ReqwestHttpTransport::for_managed_agent()?;
        let _managed_second = ReqwestHttpTransport::for_managed_agent()?;
        let provider_public = ReqwestHttpTransport::for_provider_operation(false)?;
        let provider_private = ReqwestHttpTransport::for_provider_operation(true)?;

        let (
            Some(public),
            Some(private),
            Some(patient),
            Some(provider_public_client),
            Some(provider_private_client),
        ) = (
            TransportProfile::PublicStandard.cache().value.get(),
            TransportProfile::PrivateStandard.cache().value.get(),
            TransportProfile::PublicPatient.cache().value.get(),
            TransportProfile::PublicProviderOperation
                .cache()
                .value
                .get(),
            TransportProfile::PrivateProviderOperation
                .cache()
                .value
                .get(),
        )
        else {
            return Err(RuntimeHttpError::Transport {
                message: "canonical HTTP profile client was not cached".to_owned(),
            }
            .into());
        };
        assert!(!std::ptr::eq(public, private));
        assert!(!std::ptr::eq(public, patient));
        assert!(!std::ptr::eq(public, provider_public_client));
        assert!(!std::ptr::eq(
            provider_public_client,
            provider_private_client
        ));
        assert_eq!(
            provider_public.request_timeout,
            PROVIDER_OPERATION_REQUEST_TIMEOUT
        );
        assert_eq!(
            provider_private.request_timeout,
            PROVIDER_OPERATION_REQUEST_TIMEOUT
        );
        assert!(!provider_public.allow_private_networks);
        assert!(provider_private.allow_private_networks);
        Ok(())
    }

    #[test]
    fn shared_http_runtime_supports_concurrent_sync_callers() -> Result<(), RuntimeHttpTestError> {
        let callers = (0..8)
            .map(|value| {
                std::thread::spawn(move || {
                    block_on_http(async move {
                        tokio::task::yield_now().await;
                        Ok(value)
                    })
                })
            })
            .collect::<Vec<_>>();

        let mut values = Vec::new();
        for caller in callers {
            values.push(
                caller
                    .join()
                    .map_err(|_| RuntimeHttpTestError::ServerThread)??,
            );
        }
        values.sort_unstable();
        assert_eq!(values, (0..8).collect::<Vec<_>>());
        Ok(())
    }

    #[test]
    fn guarded_dns_resolver_rejects_private_resolved_addresses() -> Result<(), RuntimeHttpTestError>
    {
        let resolver = GuardedDnsResolver::new(StaticDnsResolver {
            addrs: vec![SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(127, 0, 0, 1),
                0,
            ))],
        });
        let name = "public.example"
            .parse()
            .map_err(|error| RuntimeHttpError::Transport {
                message: format!("test DNS name should parse: {error}"),
            })?;
        let error =
            block_on_http(async {
                resolver.resolve(name).await.map(|_| ()).map_err(|error| {
                    RuntimeHttpError::Transport {
                        message: error.to_string(),
                    }
                })
            })
            .err();

        assert!(
            matches!(error, Some(RuntimeHttpError::Transport { ref message }) if message.contains("non-public address")),
            "expected private DNS resolution to fail closed, got: {error:?}"
        );
        Ok(())
    }

    #[test]
    fn reqwest_transport_does_not_follow_redirects() -> Result<(), RuntimeHttpTestError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || -> Result<String, std::io::Error> {
            let (mut stream, _) = listener.accept()?;
            let mut buffer = [0_u8; 1024];
            let bytes_read = stream.read(&mut buffer)?;
            stream.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /redirected\r\nContent-Length: 0\r\n\r\n",
            )?;
            Ok(String::from_utf8_lossy(&buffer[..bytes_read]).into_owned())
        });

        let transport = ReqwestHttpTransport::with_private_network_access_for_tests()?;
        let response = transport.send(RuntimeHttpRequest {
            method: HttpMethod::Get,
            url: format!("http://{address}/start"),
            headers: Vec::new(),
            body: None,
        })?;
        let request = server
            .join()
            .map_err(|_| RuntimeHttpTestError::ServerThread)??;

        assert_eq!(response.status, 302);
        assert!(request.starts_with("GET /start "));
        Ok(())
    }

    #[test]
    fn reqwest_transport_retries_bounded_safe_reads() -> Result<(), RuntimeHttpTestError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || -> Result<Vec<String>, std::io::Error> {
            let mut requests = Vec::new();
            for attempt in 1..=3 {
                let (mut stream, _) = listener.accept()?;
                let mut buffer = [0_u8; 1024];
                let bytes_read = stream.read(&mut buffer)?;
                requests.push(String::from_utf8_lossy(&buffer[..bytes_read]).into_owned());
                if attempt < 3 {
                    stream.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )?;
                } else {
                    stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )?;
                }
            }
            Ok(requests)
        });

        let transport = ReqwestHttpTransport::with_private_network_access_for_tests()?;
        let response = transport.send(RuntimeHttpRequest {
            method: HttpMethod::Get,
            url: format!("http://{address}/retry"),
            headers: Vec::new(),
            body: None,
        })?;
        let requests = server
            .join()
            .map_err(|_| RuntimeHttpTestError::ServerThread)??;

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
        Ok(())
    }

    #[test]
    fn reqwest_transport_never_retries_mutations() -> Result<(), RuntimeHttpTestError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || -> Result<String, std::io::Error> {
            let (mut stream, _) = listener.accept()?;
            let mut buffer = [0_u8; 1024];
            let bytes_read = stream.read(&mut buffer)?;
            stream.write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )?;
            Ok(String::from_utf8_lossy(&buffer[..bytes_read]).into_owned())
        });

        let transport = ReqwestHttpTransport::with_private_network_access_for_tests()?;
        let response = transport.send(RuntimeHttpRequest {
            method: HttpMethod::Post,
            url: format!("http://{address}/mutate"),
            headers: Vec::new(),
            body: Some("{}".to_owned()),
        })?;
        let request = server
            .join()
            .map_err(|_| RuntimeHttpTestError::ServerThread)??;

        assert_eq!(response.status, 503);
        assert!(request.starts_with("POST "));
        Ok(())
    }

    #[test]
    fn reqwest_transport_retries_declared_idempotent_posts() -> Result<(), RuntimeHttpTestError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || -> Result<Vec<String>, std::io::Error> {
            let mut requests = Vec::new();
            for attempt in 1..=3 {
                let (mut stream, _) = listener.accept()?;
                let mut buffer = [0_u8; 1024];
                let bytes_read = stream.read(&mut buffer)?;
                requests.push(String::from_utf8_lossy(&buffer[..bytes_read]).into_owned());
                if attempt < 3 {
                    stream.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )?;
                } else {
                    stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )?;
                }
            }
            Ok(requests)
        });

        let transport = ReqwestHttpTransport::with_private_network_access_for_tests()?;
        let response = transport.send_idempotent(RuntimeHttpRequest {
            method: HttpMethod::Post,
            url: format!("http://{address}/query"),
            headers: vec![RuntimeHttpHeader::new("content-type", "application/json")],
            body: Some("{}".to_owned()),
        })?;
        let requests = server
            .join()
            .map_err(|_| RuntimeHttpTestError::ServerThread)??;

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|request| request.starts_with("POST ")));
        Ok(())
    }

    #[test]
    fn reqwest_transport_rejects_header_injection() -> Result<(), RuntimeHttpTestError> {
        let transport = ReqwestHttpTransport::new()?;
        let error = transport
            .send(RuntimeHttpRequest {
                method: HttpMethod::Get,
                url: "https://api.example/v1".to_owned(),
                headers: vec![RuntimeHttpHeader::new("x-runx", "good\nbad")],
                body: None,
            })
            .err();
        assert!(matches!(
            error,
            Some(RuntimeHttpError::InvalidHeaderValue { .. })
        ));
        Ok(())
    }

    #[test]
    fn reqwest_transport_rejects_non_http_urls_before_sending() -> Result<(), RuntimeHttpTestError>
    {
        let transport = ReqwestHttpTransport::new()?;
        let error = transport
            .send(RuntimeHttpRequest {
                method: HttpMethod::Get,
                url: "file:///etc/passwd".to_owned(),
                headers: Vec::new(),
                body: None,
            })
            .err();

        assert!(matches!(
            error,
            Some(RuntimeHttpError::UnsupportedUrlScheme { .. })
        ));
        Ok(())
    }

    #[test]
    fn reqwest_transport_refuses_a_declared_length_over_the_limit()
    -> Result<(), RuntimeHttpTestError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || -> Result<(), std::io::Error> {
            let (mut stream, _) = listener.accept()?;
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer)?;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n")?;
            Ok(())
        });

        let transport = ReqwestHttpTransport::with_private_network_access_for_tests()?;
        let error = transport
            .send_limited(
                RuntimeHttpRequest {
                    method: HttpMethod::Get,
                    url: format!("http://{address}/too-large"),
                    headers: Vec::new(),
                    body: None,
                },
                16,
            )
            .err();
        server
            .join()
            .map_err(|_| RuntimeHttpTestError::ServerThread)??;

        assert!(matches!(
            error,
            Some(RuntimeHttpError::ResponseBodyTooLarge { limit: 16 })
        ));
        Ok(())
    }

    #[test]
    fn reqwest_transport_reads_whole_bodies_and_refuses_a_stream_over_the_limit()
    -> Result<(), RuntimeHttpTestError> {
        const BODY_BYTES: usize = 9 * 1024 * 1024;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || -> Result<(), std::io::Error> {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept()?;
                let mut buffer = [0_u8; 1024];
                let _ = stream.read(&mut buffer)?;
                stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")?;
                let _ = stream.write_all(&vec![b'a'; BODY_BYTES]);
            }
            Ok(())
        });
        let request = || RuntimeHttpRequest {
            method: HttpMethod::Get,
            url: format!("http://{address}/stream"),
            headers: Vec::new(),
            body: None,
        };

        let transport = ReqwestHttpTransport::with_private_network_access_for_tests()?;
        let whole = transport.send(request())?;
        let limited = transport.send_limited(request(), BODY_BYTES - 1).err();
        server
            .join()
            .map_err(|_| RuntimeHttpTestError::ServerThread)??;

        assert_eq!(whole.body_bytes, BODY_BYTES);
        assert!(matches!(
            limited,
            Some(RuntimeHttpError::ResponseBodyTooLarge { limit }) if limit == BODY_BYTES - 1
        ));
        Ok(())
    }

    #[test]
    fn transport_filters_response_secrets() -> Result<(), RuntimeHttpTestError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || -> Result<(), std::io::Error> {
            let (mut stream, _) = listener.accept()?;
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer)?;
            stream.write_all(
                b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/plain\r\nSet-Cookie: secret=value\r\n\r\nabcdef",
            )?;
            Ok(())
        });

        let transport = ReqwestHttpTransport::with_private_network_access_for_tests()?;
        let response = transport.send(RuntimeHttpRequest {
            method: HttpMethod::Get,
            url: format!("http://{address}/secrets"),
            headers: Vec::new(),
            body: None,
        })?;
        server
            .join()
            .map_err(|_| RuntimeHttpTestError::ServerThread)??;

        assert_eq!(response.body, "abcdef");
        assert_eq!(response.body_bytes, 6);
        assert_eq!(
            response.body_digest,
            runx_contracts::sha256_prefixed(b"abcdef")
        );
        assert!(response.headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("content-type") && header.value == "text/plain"
        }));
        assert!(
            response
                .headers
                .iter()
                .all(|header| !header.name.eq_ignore_ascii_case("set-cookie"))
        );
        Ok(())
    }

    #[test]
    fn reqwest_transport_times_out_stalled_response() -> Result<(), RuntimeHttpTestError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || -> Result<(), std::io::Error> {
            // Stall without ever responding until the client's timeout fires
            // and it drops the connection (read observes EOF or reset), rather
            // than sleeping a fixed window longer than the timeout under test.
            let (mut stream, _) = listener.accept()?;
            let mut buffer = [0_u8; 1024];
            while let Ok(read) = stream.read(&mut buffer) {
                if read == 0 {
                    break;
                }
            }
            Ok(())
        });

        let transport = ReqwestHttpTransport::with_private_network_timeouts_for_tests(
            Duration::from_millis(100),
            Duration::from_millis(100),
        )?;
        let error = transport
            .send(RuntimeHttpRequest {
                method: HttpMethod::Get,
                url: format!("http://{address}/stall"),
                headers: Vec::new(),
                body: None,
            })
            .err();
        server
            .join()
            .map_err(|_| RuntimeHttpTestError::ServerThread)??;

        assert!(matches!(error, Some(RuntimeHttpError::Transport { .. })));
        Ok(())
    }
}
