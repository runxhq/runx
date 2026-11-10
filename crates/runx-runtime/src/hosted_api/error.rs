use std::fmt;

use serde::Deserialize;

use super::HOSTED_API_TOKEN_ENV;
use crate::config::ConfigError;
use crate::http::RuntimeHttpError;
use crate::secrets::SealedError;

#[derive(Debug)]
pub enum HostedApiError {
    Config(ConfigError),
    /// The token `runx login` sealed in the runx home could not be read or
    /// written.
    Sealed(SealedError),
    RuntimeHttp(RuntimeHttpError),
    InvalidBaseUrl(String),
    MissingToken,
    /// The token in the environment and the one `runx login` stored for the
    /// same API differ: either could be another principal.
    ConflictingTokens {
        base_url: String,
    },
    StoredCredentialEnvironmentMismatch {
        base_url: String,
    },
    AuthenticationStatus {
        status: u16,
        detail: String,
    },
    InvalidPrincipal(String),
    PrincipalMismatch {
        expected: String,
        actual: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum HostedApiOperationError {
    #[error(transparent)]
    RuntimeHttp(#[from] RuntimeHttpError),
    #[error("runx API {operation} returned error [{code}] with HTTP {status}: {detail}")]
    Api {
        operation: &'static str,
        status: u16,
        code: String,
        detail: String,
        hint: Option<String>,
        retry_after_seconds: Option<u32>,
        /// The conflict the API states, when it refuses an idempotency key
        /// as another request's and says which requests.
        conflict: Option<Box<KeyConflict>>,
    },
    #[error("runx API {operation} returned HTTP {status}: {detail}")]
    HttpStatus {
        operation: &'static str,
        status: u16,
        detail: String,
    },
    #[error("runx API {operation} returned invalid JSON: {message}")]
    InvalidJson {
        operation: &'static str,
        message: String,
    },
    #[error("invalid runx API {operation}: {message}")]
    InvalidRequest {
        operation: &'static str,
        message: String,
    },
}

impl fmt::Display for HostedApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Sealed(error) => write!(formatter, "{error}"),
            Self::RuntimeHttp(error) => write!(formatter, "{error}"),
            Self::InvalidBaseUrl(message) => {
                write!(formatter, "invalid Runx API base URL: {message}")
            }
            Self::MissingToken => write!(
                formatter,
                "missing public API token; run `runx login` or set {HOSTED_API_TOKEN_ENV}"
            ),
            Self::ConflictingTokens { base_url } => write!(
                formatter,
                "{HOSTED_API_TOKEN_ENV} and the token `runx login` stored for {base_url} differ; unset {HOSTED_API_TOKEN_ENV} or sign in again"
            ),
            Self::StoredCredentialEnvironmentMismatch { base_url } => write!(
                formatter,
                "stored login belongs to a different runx environment; login to {base_url} or provide {HOSTED_API_TOKEN_ENV} explicitly"
            ),
            Self::AuthenticationStatus { status, detail } => write!(
                formatter,
                "runx API authentication returned HTTP {status}: {detail}"
            ),
            Self::InvalidPrincipal(message) => {
                write!(
                    formatter,
                    "runx API returned an invalid principal: {message}"
                )
            }
            Self::PrincipalMismatch { expected, actual } => write!(
                formatter,
                "runx API principal mismatch: stored {expected}, authenticated {actual}"
            ),
        }
    }
}

impl std::error::Error for HostedApiError {}

impl From<ConfigError> for HostedApiError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<SealedError> for HostedApiError {
    fn from(error: SealedError) -> Self {
        Self::Sealed(error)
    }
}

impl From<RuntimeHttpError> for HostedApiError {
    fn from(error: RuntimeHttpError) -> Self {
        Self::RuntimeHttp(error)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HostedApiErrorPayload {
    pub code: String,
    pub detail: String,
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub retry_after_seconds: Option<u32>,
    /// This request's digest as the API claims it, for an idempotency
    /// conflict.
    #[serde(default)]
    pub request_digest: Option<String>,
    /// The digest of the request the key is bound to, when a single one is.
    #[serde(default)]
    pub existing_request_digest: Option<String>,
}

/// An idempotency key the API has bound to another request: this request's
/// digest as the API claims it, and the existing request's, when the key is
/// bound to a single one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyConflict {
    pub request_digest: runx_contracts::Digest,
    pub existing_request_digest: Option<runx_contracts::Digest>,
}

impl HostedApiOperationError {
    /// The API's refusal of `operation` with `status`, as `payload` states
    /// it.
    pub(crate) fn api(
        operation: &'static str,
        status: u16,
        payload: HostedApiErrorPayload,
    ) -> Self {
        let conflict = payload
            .request_digest
            .and_then(runx_contracts::Digest::new)
            .map(|request_digest| {
                Box::new(KeyConflict {
                    request_digest,
                    existing_request_digest: payload
                        .existing_request_digest
                        .and_then(runx_contracts::Digest::new),
                })
            });
        Self::Api {
            operation,
            status,
            code: payload.code,
            detail: payload.detail,
            hint: payload.hint,
            retry_after_seconds: payload.retry_after_seconds,
            conflict,
        }
    }
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: HostedApiErrorPayload,
}

#[derive(Deserialize)]
struct PlainErrorEnvelope {
    error: PlainError,
}

#[derive(Deserialize)]
struct FlatErrorEnvelope {
    error: String,
    code: String,
    #[serde(default)]
    hint: Option<String>,
    #[serde(default)]
    retry_after_seconds: Option<u32>,
    #[serde(default)]
    request_digest: Option<String>,
    #[serde(default)]
    existing_request_digest: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PlainError {
    Message(String),
    Payload(HostedApiErrorPayload),
}

pub fn parse_hosted_api_error(body: &str) -> Option<HostedApiErrorPayload> {
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .map(|envelope| envelope.error)
        .or_else(|| {
            serde_json::from_str::<FlatErrorEnvelope>(body)
                .ok()
                .map(|envelope| HostedApiErrorPayload {
                    code: envelope.code,
                    detail: envelope.error,
                    hint: envelope.hint,
                    retry_after_seconds: envelope.retry_after_seconds,
                    request_digest: envelope.request_digest,
                    existing_request_digest: envelope.existing_request_digest,
                })
        })
        .or_else(|| {
            serde_json::from_str::<PlainErrorEnvelope>(body)
                .ok()
                .map(|envelope| match envelope.error {
                    PlainError::Message(detail) => HostedApiErrorPayload {
                        code: "api_error".to_owned(),
                        detail,
                        hint: None,
                        retry_after_seconds: None,
                        request_digest: None,
                        existing_request_digest: None,
                    },
                    PlainError::Payload(payload) => payload,
                })
        })
}
