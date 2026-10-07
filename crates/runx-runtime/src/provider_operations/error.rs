use std::fmt;

use crate::hosted_api::HostedApiOperationError;

#[derive(Debug)]
pub enum ProviderOperationError {
    InvalidGrantId,
    InvalidOperation,
    InvalidScopes,
    HostedApi(HostedApiOperationError),
    InvalidResponse(String),
}

impl fmt::Display for ProviderOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGrantId => formatter
                .write_str("provider grant id must be a safe, non-empty URL path identifier"),
            Self::InvalidOperation => formatter.write_str(
                "provider operation must be dotted lowercase segments such as thread.reply or reports.run_realtime",
            ),
            Self::InvalidScopes => formatter
                .write_str("provider operation scopes must contain non-empty capability strings"),
            Self::HostedApi(error) => write!(formatter, "{error}"),
            Self::InvalidResponse(message) => write!(
                formatter,
                "Connect API returned invalid provider evidence: {message}"
            ),
        }
    }
}

impl std::error::Error for ProviderOperationError {}

impl From<HostedApiOperationError> for ProviderOperationError {
    fn from(error: HostedApiOperationError) -> Self {
        Self::HostedApi(error)
    }
}
