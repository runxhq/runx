use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::request::{non_empty, path_identifier, send_json};
use super::{AuthenticatedHostedApiEnvironment, HostedApiOperationError};
use crate::http::{HttpMethod, RuntimeHttpTransport};

pub enum HostedConnectAction<'a> {
    List,
    Status {
        session_id: &'a str,
    },
    Revoke {
        grant_id: &'a str,
    },
    Start(HostedConnectStart<'a>),
    SubmitCredentials {
        session_id: &'a str,
        credentials: &'a Value,
    },
}

/// A new Connect grant: the provider, the scopes it serves and, optionally,
/// the one target it is narrowed to. Connect derives the rest of the grant's
/// reference from the provider's operations.
#[derive(Clone, Copy, Debug)]
pub struct HostedConnectStart<'a> {
    pub provider: &'a str,
    pub scopes: &'a [String],
    pub target: Option<&'a str>,
}

#[derive(Serialize)]
struct ConnectStartRequest<'a> {
    provider: &'a str,
    scopes: &'a [String],
    #[serde(rename = "target_locator", skip_serializing_if = "Option::is_none")]
    target: Option<&'a str>,
}

/// A Connect session as the hosted API reports it, from opening it, sending
/// it a credential or reading its status.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConnectSession {
    /// The session made a grant.
    Created { grant: ConnectSessionGrant },
    /// An existing grant already serves the request.
    Unchanged { grant: ConnectSessionGrant },
    /// Waiting on the account holder, at `authorization_url` when they have
    /// one to visit.
    Pending {
        session_id: String,
        #[serde(default)]
        authorization_url: Option<String>,
        #[serde(default)]
        poll_after_ms: Option<u64>,
    },
    /// Waiting on the provider to verify the credential the session holds.
    PendingVerification {
        session_id: String,
        #[serde(default)]
        poll_after_ms: Option<u64>,
    },
    /// The placement takes the user's own credential, which the session
    /// does not hold yet.
    CredentialRequired { session_id: String },
    Failed {
        #[serde(default)]
        error: Option<String>,
    },
    VerificationFailed {
        #[serde(default)]
        error: Option<String>,
    },
    ConnectUnavailable {
        #[serde(default)]
        error: Option<String>,
    },
    Unsupported {
        #[serde(default)]
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConnectSessionGrant {
    pub grant_id: String,
}

impl ConnectSession {
    /// Read a Connect response as a session.
    ///
    /// # Errors
    ///
    /// Returns the status the response named when it is no session state.
    pub fn from_response(response: Value) -> Result<Self, String> {
        let status = response
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("missing")
            .to_owned();
        serde_json::from_value(response).map_err(|error| {
            format!("Connect answered a session status {status} runx does not read: {error}")
        })
    }
}

#[derive(Serialize)]
struct ConnectCredentialsRequest<'a> {
    credentials: &'a Value,
}

pub fn execute_hosted_connect(
    transport: &(impl RuntimeHttpTransport + ?Sized),
    environment: &AuthenticatedHostedApiEnvironment,
    action: HostedConnectAction<'_>,
) -> Result<Value, HostedApiOperationError> {
    let (method, path, body) = match action {
        HostedConnectAction::List => (HttpMethod::Get, "/v1/grants".to_owned(), None),
        HostedConnectAction::Status { session_id } => (
            HttpMethod::Get,
            format!(
                "/v1/connect/sessions/{}",
                path_identifier("connect request", "session id", session_id)?
            ),
            None,
        ),
        HostedConnectAction::Revoke { grant_id } => (
            HttpMethod::Delete,
            format!(
                "/v1/grants/{}",
                path_identifier("connect request", "grant id", grant_id)?
            ),
            None,
        ),
        HostedConnectAction::Start(start) => {
            let body = serde_json::to_string(&ConnectStartRequest {
                provider: start.provider,
                scopes: start.scopes,
                target: non_empty(start.target),
            })
            .map_err(|error| HostedApiOperationError::InvalidRequest {
                operation: "connect start request",
                message: error.to_string(),
            })?;
            (
                HttpMethod::Post,
                "/v1/connect/sessions".to_owned(),
                Some(body),
            )
        }
        HostedConnectAction::SubmitCredentials {
            session_id,
            credentials,
        } => {
            let body = serde_json::to_string(&ConnectCredentialsRequest { credentials }).map_err(
                |error| HostedApiOperationError::InvalidRequest {
                    operation: "connect credential request",
                    message: error.to_string(),
                },
            )?;
            (
                HttpMethod::Post,
                format!(
                    "/v1/connect/sessions/{}/credentials",
                    path_identifier("connect request", "session id", session_id)?
                ),
                Some(body),
            )
        }
    };
    send_json(
        transport,
        environment.base_url(),
        "connect",
        method,
        &path,
        Some(environment.token()),
        body,
    )
}
