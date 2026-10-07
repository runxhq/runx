//! Cloud registry index endpoint (`POST /v1/index`).
//!
//! The client for indexing a repository into the hosted runx registry, used by
//! `runx add <repository-url>`. The hosted index decides which repository hosts
//! it supports and names the repository it indexed; this module only POSTs the
//! URL and returns the typed envelope. Presentation lives in the CLI.

use serde::{Deserialize, Serialize};

use super::types::TrustTier;
use crate::hosted_api::{HostedApiOperationError, request::send_json};
use crate::http::{HttpMethod, RuntimeHttpTransport as Transport};

/// Inputs to [`index_repository`].
#[derive(Clone, Debug)]
pub struct IndexRepositoryOptions<'a> {
    /// Base URL of the hosted registry (no trailing slash required).
    pub base_url: &'a str,
    /// The repository URL, as the person gave it.
    pub repo_url: &'a str,
    /// Optional branch/tag forwarded as `ref` in the request body.
    pub repo_ref: Option<&'a str>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IndexedRepo {
    pub owner: String,
    pub repo: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub sha: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IndexedListing {
    pub owner: String,
    pub name: String,
    pub skill_id: String,
    pub version: String,
    pub permalink: String,
    pub trust_tier: TrustTier,
    pub skill_path: String,
    pub digest_unchanged: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IndexWarning {
    #[serde(default)]
    pub skill_path: Option<String>,
    pub code: String,
    pub detail: String,
}

/// Successful `POST /v1/index` envelope returned by the cloud.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IndexResponse {
    pub repo: IndexedRepo,
    pub listings: Vec<IndexedListing>,
    #[serde(default)]
    pub warnings: Vec<IndexWarning>,
}

/// Errors returned by [`index_repository`].
///
/// Distinct from [`crate::registry::RegistryClientError`] because the `/v1/index`
/// endpoint has its own error envelope shape (`{ status: "error", error: { code,
/// detail, hint?, retry_after_seconds? } }`) that callers want to match on. We
/// surface those structured fields directly so the CLI can render hints and
/// retry guidance without re-parsing strings.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error(transparent)]
    HostedApi(#[from] HostedApiOperationError),
    #[error("runx API index returned an invalid response: {0}")]
    InvalidResponse(String),
}

/// POST the repo URL to the hosted registry's `/v1/index` endpoint.
///
/// The transport handles timeouts/retries/TLS per the runtime's standard HTTP
/// discipline. Generic over `T: Transport` so tests can inject a stub without
/// touching the network.
pub fn index_repository<T: Transport>(
    transport: &T,
    options: &IndexRepositoryOptions<'_>,
) -> Result<IndexResponse, IndexError> {
    let base = options.base_url.trim_end_matches('/');
    let body = serde_json::json!({
        "repo_url": options.repo_url,
        "ref": options.repo_ref,
    })
    .to_string();
    let envelope: SuccessEnvelope = send_json(
        transport,
        base,
        "registry index",
        HttpMethod::Post,
        "/v1/index",
        None,
        Some(body),
    )?;
    if envelope.status != "success" {
        return Err(IndexError::InvalidResponse(format!(
            "expected status \"success\", received \"{}\"",
            envelope.status
        )));
    }
    Ok(IndexResponse {
        repo: envelope.repo,
        listings: envelope.listings,
        warnings: envelope.warnings,
    })
}

#[derive(Deserialize)]
struct SuccessEnvelope {
    status: String,
    repo: IndexedRepo,
    listings: Vec<IndexedListing>,
    #[serde(default)]
    warnings: Vec<IndexWarning>,
}
