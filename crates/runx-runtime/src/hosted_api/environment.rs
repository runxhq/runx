use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use runx_contracts::RunxPrincipalId;
use serde::Deserialize;
use url::{Host, Url};

use super::{
    DEFAULT_HOSTED_API_BASE_URL, HOSTED_API_ALLOW_PRIVATE_NETWORK_ENV, HOSTED_API_BASE_URL_ENV,
    HOSTED_API_TOKEN_ENV, HostedApiError, parse_hosted_api_error,
};
use crate::config::{
    load_local_public_api_token, load_runx_config_file, store_local_public_api_token,
    write_runx_config_file,
};
use crate::http::{
    HttpMethod, ReqwestHttpTransport as DefaultRuntimeHttpTransport, RuntimeHttpError,
    RuntimeHttpHeader, RuntimeHttpRequest as HttpRequest, RuntimeHttpTransport as Transport,
};

/// One resolved hosted environment for CLI commands and native provider
/// tools. Base URL, credential, and expected principal are selected together
/// so a stored token can never silently cross environments.
#[derive(Clone, PartialEq, Eq)]
pub struct HostedApiEnvironment {
    base_url: String,
    token: Option<String>,
    expected_principal_id: Option<String>,
    stored_credential_environment_mismatch: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AuthenticatedHostedApiEnvironment {
    base_url: String,
    token: String,
    principal_id: RunxPrincipalId,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostedApiCredentialPurpose {
    #[default]
    Default,
    Publish,
}

impl HostedApiCredentialPurpose {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Publish => "publish",
        }
    }
}

impl fmt::Debug for HostedApiEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostedApiEnvironment")
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("expected_principal_id", &self.expected_principal_id)
            .field(
                "stored_credential_environment_mismatch",
                &self.stored_credential_environment_mismatch,
            )
            .finish()
    }
}

impl fmt::Debug for AuthenticatedHostedApiEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedHostedApiEnvironment")
            .field("base_url", &self.base_url)
            .field("token", &"[REDACTED]")
            .field("principal_id", &self.principal_id)
            .finish()
    }
}

impl HostedApiEnvironment {
    pub fn resolve(
        explicit_base_url: Option<&str>,
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, HostedApiError> {
        Self::resolve_inner(
            explicit_base_url,
            env,
            cwd,
            Some(HostedApiCredentialPurpose::Default),
        )
    }

    pub fn resolve_publish(
        explicit_base_url: Option<&str>,
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, HostedApiError> {
        Self::resolve_inner(
            explicit_base_url,
            env,
            cwd,
            Some(HostedApiCredentialPurpose::Publish),
        )
    }

    pub fn resolve_unauthenticated(
        explicit_base_url: Option<&str>,
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, HostedApiError> {
        Self::resolve_inner(explicit_base_url, env, cwd, None)
    }

    /// The token is the environment's, else the stored login's; it is never
    /// taken from a command argument.
    fn resolve_inner(
        explicit_base_url: Option<&str>,
        env: &BTreeMap<String, String>,
        cwd: &Path,
        credential_purpose: Option<HostedApiCredentialPurpose>,
    ) -> Result<Self, HostedApiError> {
        let paths = crate::paths::RunxPaths::resolve(env, cwd);
        let config = load_runx_config_file(&paths.config_file())?;
        let public = config.public.unwrap_or_default();
        let stored_base_url = normalize_hosted_base_url(match credential_purpose {
            Some(HostedApiCredentialPurpose::Publish) => public.publish_api_base_url.as_deref(),
            Some(HostedApiCredentialPurpose::Default) | None => public.api_base_url.as_deref(),
        })?
        .unwrap_or_else(|| DEFAULT_HOSTED_API_BASE_URL.to_owned());
        let explicit_base_url = normalize_hosted_base_url(explicit_base_url)?;
        let environment_base_url =
            normalize_hosted_base_url(env.get(HOSTED_API_BASE_URL_ENV).map(String::as_str))?;
        let base_url = explicit_base_url
            .or(environment_base_url)
            .unwrap_or_else(|| stored_base_url.clone());
        let direct_token = credential_purpose
            .is_some()
            .then(|| non_empty(env.get(HOSTED_API_TOKEN_ENV).map(String::as_str)))
            .flatten();
        let stored_token_allowed = base_url == stored_base_url;
        let (stored_token_ref, stored_principal_id) = match credential_purpose {
            Some(HostedApiCredentialPurpose::Default) => {
                (public.api_token_ref.as_deref(), public.principal_id.clone())
            }
            Some(HostedApiCredentialPurpose::Publish) => (
                public.publish_api_token_ref.as_deref(),
                public.publish_principal_id.clone(),
            ),
            None => (None, None),
        };
        let stored_token = if direct_token.is_none() && stored_token_allowed {
            stored_token_ref
                .map(|token_ref| load_local_public_api_token(&paths, token_ref))
                .transpose()?
                .and_then(|token| non_empty(Some(&token)))
        } else {
            None
        };
        let using_stored_token = direct_token.is_none() && stored_token.is_some();
        let stored_credential_environment_mismatch = credential_purpose.is_some()
            && direct_token.is_none()
            && stored_token_ref.is_some()
            && !stored_token_allowed;
        Ok(Self {
            base_url,
            token: direct_token.or(stored_token),
            expected_principal_id: using_stored_token.then_some(stored_principal_id).flatten(),
            stored_credential_environment_mismatch,
        })
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn require_token(&self) -> Result<&str, HostedApiError> {
        if let Some(token) = self.token.as_deref() {
            return Ok(token);
        }
        if self.stored_credential_environment_mismatch {
            return Err(HostedApiError::StoredCredentialEnvironmentMismatch {
                base_url: self.base_url.clone(),
            });
        }
        Err(HostedApiError::MissingToken)
    }

    pub fn authenticate<T: Transport + ?Sized>(
        &self,
        transport: &T,
    ) -> Result<AuthenticatedHostedApiEnvironment, HostedApiError> {
        let token = self.require_token()?;
        // Reading the principal is free and changes nothing, so it may repeat.
        let response = transport.send_idempotent(HttpRequest {
            method: HttpMethod::Get,
            url: format!("{}/v1/me", self.base_url),
            headers: vec![RuntimeHttpHeader::new(
                "authorization",
                format!("Bearer {token}"),
            )],
            body: None,
        })?;
        if !(200..=299).contains(&response.status) {
            return Err(HostedApiError::AuthenticationStatus {
                status: response.status,
                detail: parse_hosted_api_error(&response.body)
                    .map(|error| error.detail)
                    .unwrap_or(response.body),
            });
        }
        let profile = serde_json::from_str::<PrincipalProfile>(&response.body)
            .map_err(|error| HostedApiError::InvalidPrincipal(error.to_string()))?;
        if profile.status != "success" {
            return Err(HostedApiError::InvalidPrincipal(
                "response did not identify a successful principal".to_owned(),
            ));
        }
        let principal_id = parse_runx_principal_id(profile.principal.principal_id)?;
        if let Some(expected) = self.expected_principal_id.as_deref()
            && expected != principal_id.as_str()
        {
            return Err(HostedApiError::PrincipalMismatch {
                expected: expected.to_owned(),
                actual: principal_id.as_str().to_owned(),
            });
        }
        Ok(AuthenticatedHostedApiEnvironment {
            base_url: self.base_url.clone(),
            token: token.to_owned(),
            principal_id,
        })
    }
}

impl AuthenticatedHostedApiEnvironment {
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    #[must_use]
    pub fn principal_id(&self) -> &str {
        self.principal_id.as_str()
    }

    #[must_use]
    pub(crate) fn runx_principal_id(&self) -> &RunxPrincipalId {
        &self.principal_id
    }
}

pub fn store_authenticated_hosted_environment(
    env: &BTreeMap<String, String>,
    cwd: &Path,
    purpose: HostedApiCredentialPurpose,
    base_url: &str,
    principal_id: &str,
    token: &str,
) -> Result<(), HostedApiError> {
    let principal_id = parse_runx_principal_id(principal_id)?;
    let paths = crate::paths::RunxPaths::resolve(env, cwd);
    let config_path = paths.config_file();
    let mut next = load_runx_config_file(&config_path)?;
    let token_ref = store_local_public_api_token(&paths, token)?;
    let public = next.public.get_or_insert_default();
    let base_url = normalize_hosted_base_url(Some(base_url))?
        .unwrap_or_else(|| DEFAULT_HOSTED_API_BASE_URL.to_owned());
    match purpose {
        HostedApiCredentialPurpose::Default => {
            public.api_base_url = Some(base_url);
            public.api_token_ref = Some(token_ref);
            public.principal_id = Some(principal_id.into_string());
        }
        HostedApiCredentialPurpose::Publish => {
            public.publish_api_base_url = Some(base_url);
            public.publish_api_token_ref = Some(token_ref);
            public.publish_principal_id = Some(principal_id.into_string());
        }
    }
    write_runx_config_file(&config_path, &next)?;
    Ok(())
}

#[must_use]
pub fn hosted_private_network_allowed(explicit: bool, env: &BTreeMap<String, String>) -> bool {
    explicit
        || env
            .get(HOSTED_API_ALLOW_PRIVATE_NETWORK_ENV)
            .is_some_and(|value| truthy_env(value))
}

pub fn hosted_api_transport(
    allow_private_network: bool,
) -> Result<DefaultRuntimeHttpTransport, RuntimeHttpError> {
    if allow_private_network {
        return DefaultRuntimeHttpTransport::with_private_network_access();
    }
    DefaultRuntimeHttpTransport::new()
}

/// Provider calls may legitimately consume their adapter-owned synchronous
/// deadline. Give only that boundary the larger control-plane envelope.
pub fn hosted_provider_api_transport(
    allow_private_network: bool,
) -> Result<DefaultRuntimeHttpTransport, RuntimeHttpError> {
    DefaultRuntimeHttpTransport::for_provider_operation(allow_private_network)
}

fn normalize_hosted_base_url(value: Option<&str>) -> Result<Option<String>, HostedApiError> {
    let Some(normalized) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let parsed = Url::parse(normalized)
        .map_err(|_| HostedApiError::InvalidBaseUrl("expected an absolute HTTPS URL".to_owned()))?;
    if parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(HostedApiError::InvalidBaseUrl(
            "credentials, query strings, fragments, and hostless URLs are not allowed".to_owned(),
        ));
    }
    match parsed.scheme() {
        "https" => {}
        "http" if is_loopback_host(&parsed) => {}
        "http" => {
            return Err(HostedApiError::InvalidBaseUrl(
                "HTTP is allowed only for loopback development endpoints".to_owned(),
            ));
        }
        _ => {
            return Err(HostedApiError::InvalidBaseUrl(
                "only HTTPS is allowed outside loopback development".to_owned(),
            ));
        }
    }
    Ok(Some(parsed.as_str().trim_end_matches('/').to_owned()))
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        None => false,
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_runx_principal_id(value: impl Into<String>) -> Result<RunxPrincipalId, HostedApiError> {
    RunxPrincipalId::new(value).ok_or_else(|| {
        HostedApiError::InvalidPrincipal(
            "principal_id must match ^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$".to_owned(),
        )
    })
}

fn truthy_env(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES")
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct PrincipalProfile {
    status: String,
    principal: PrincipalIdentity,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct PrincipalIdentity {
    principal_id: String,
}

#[cfg(test)]
mod tests;
