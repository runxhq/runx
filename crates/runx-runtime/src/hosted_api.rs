//! Shared hosted Runx API environment resolution.
//!
//! This module keeps environment selection, credential binding, authentication,
//! and public error parsing behind one stable facade. Provider-specific operator
//! logic remains outside the hosted control-plane boundary.

mod connect;
mod environment;
mod error;
mod login;
mod receipts;
pub(crate) mod request;
mod skill_endpoint;

pub use connect::{
    ConnectSession, HostedConnectAction, HostedConnectStart, execute_hosted_connect,
};
pub use environment::{
    AuthenticatedHostedApiEnvironment, HostedApiCredentialPurpose, HostedApiEnvironment,
    hosted_api_transport, hosted_private_network_allowed, hosted_provider_api_transport,
    store_authenticated_hosted_environment,
};
pub use error::{
    HostedApiError, HostedApiErrorPayload, HostedApiOperationError, parse_hosted_api_error,
};
pub use login::{
    HostedLoginCompleteResponse, HostedLoginStartResponse, HostedProviderTokenLoginResponse,
    complete_hosted_login, exchange_hosted_provider_token, start_hosted_login,
};
pub use receipts::{ReceiptNotarization, notarize_hosted_receipt};
pub use skill_endpoint::{
    HostedSkillChallenge, HostedSkillEndpointError, request_hosted_skill_challenge,
};

pub const DEFAULT_HOSTED_API_BASE_URL: &str = "https://api.runx.ai";
pub const HOSTED_API_BASE_URL_ENV: &str = "RUNX_PUBLIC_API_BASE_URL";
pub const HOSTED_API_TOKEN_ENV: &str = "RUNX_PUBLIC_API_TOKEN";
pub const HOSTED_API_ALLOW_PRIVATE_NETWORK_ENV: &str = "RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK";
