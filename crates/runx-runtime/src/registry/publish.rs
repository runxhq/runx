use runx_contracts::CopyableCommand;
use serde::{Deserialize, Serialize};

use super::{RegistryPackageFile, RegistryPublishHarnessReport};
use crate::hosted_api::{HostedApiOperationError, request::send_json};
use crate::http::{HttpMethod, RuntimeHttpTransport};

#[derive(Serialize)]
pub struct HostedSkillPublishRequest<'a> {
    pub markdown: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_document: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<&'a str>,
    /// The namespace to publish into, for a principal that may publish into
    /// several; the registry otherwise takes the principal's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<&'a str>,
    #[serde(skip_serializing_if = "slice_empty")]
    pub package_files: &'a [RegistryPackageFile],
}

#[derive(Serialize)]
pub struct HostedAdminSkillPublishRequest<'a> {
    pub owner: &'a str,
    pub markdown: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_document: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<&'a str>,
    #[serde(skip_serializing_if = "slice_empty")]
    pub package_files: &'a [RegistryPackageFile],
    pub harness: &'a RegistryPublishHarnessReport,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HostedSkillPublishResult {
    pub status: String,
    pub skill_id: String,
    pub owner: String,
    pub name: String,
    pub version: String,
    pub digest: String,
    #[serde(default)]
    pub profile_digest: Option<String>,
    pub trust_tier: String,
    pub install_command: CopyableCommand,
    pub run_command: CopyableCommand,
    pub public_url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryPublishError {
    #[error(transparent)]
    Operation(#[from] HostedApiOperationError),
    #[error("remote registry publish returned an invalid contract: {0}")]
    Contract(String),
}

pub fn publish_hosted_skill(
    transport: &impl RuntimeHttpTransport,
    registry_url: &str,
    token: &str,
    request: &HostedSkillPublishRequest<'_>,
) -> Result<HostedSkillPublishResult, RegistryPublishError> {
    let body = serde_json::to_string(request).map_err(invalid_request_json)?;
    let envelope: HostedSkillPublishEnvelope = send_json(
        transport,
        registry_url,
        "registry publish",
        HttpMethod::Post,
        "/v1/skills",
        Some(token),
        Some(body),
    )?;
    if envelope.status != "success" || envelope.publish.status != "published" {
        return Err(RegistryPublishError::Contract(format!(
            "unsuccessful status: envelope={}, publish={}",
            envelope.status, envelope.publish.status
        )));
    }
    Ok(envelope.publish)
}

/// A connector package for the hosted registry, which re-reads the files
/// natively and publishes them under the authenticated publisher's namespace.
#[derive(Serialize)]
pub struct HostedConnectorPublishRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<&'a str>,
    pub files: &'a [RegistryPackageFile],
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HostedConnectorPublishResult {
    /// `published`, or `unchanged` for these exact files under this version.
    pub status: String,
    #[serde(rename = "ref")]
    pub name: runx_contracts::connector::ConnectorName,
    pub version: String,
    pub content_digest: runx_contracts::connector::Digest,
    pub manifest_digest: runx_contracts::connector::Digest,
    pub connector_digest: runx_contracts::connector::Digest,
    pub trust_tier: super::TrustTier,
    pub install_command: CopyableCommand,
    pub public_url: String,
}

pub fn publish_hosted_connector(
    transport: &impl RuntimeHttpTransport,
    registry_url: &str,
    token: &str,
    request: &HostedConnectorPublishRequest<'_>,
) -> Result<HostedConnectorPublishResult, RegistryPublishError> {
    let body = serde_json::to_string(request).map_err(invalid_request_json)?;
    let envelope: HostedConnectorPublishEnvelope = send_json(
        transport,
        registry_url,
        "registry connector publish",
        HttpMethod::Post,
        "/v1/connectors",
        Some(token),
        Some(body),
    )?;
    if envelope.status != "success"
        || !matches!(envelope.publish.status.as_str(), "published" | "unchanged")
    {
        return Err(RegistryPublishError::Contract(format!(
            "unsuccessful status: envelope={}, publish={}",
            envelope.status, envelope.publish.status
        )));
    }
    Ok(envelope.publish)
}

pub fn publish_hosted_admin_skill(
    transport: &impl RuntimeHttpTransport,
    registry_url: &str,
    token: &str,
    request: &HostedAdminSkillPublishRequest<'_>,
) -> Result<HostedSkillPublishResult, RegistryPublishError> {
    let body = serde_json::to_string(request).map_err(invalid_request_json)?;
    let envelope: HostedAdminSkillPublishEnvelope = send_json(
        transport,
        registry_url,
        "registry admin publish",
        HttpMethod::Post,
        "/v1/admin/registry/publish",
        Some(token),
        Some(body),
    )?;
    if envelope.status != "success"
        || !matches!(envelope.publish.status.as_str(), "published" | "unchanged")
    {
        return Err(RegistryPublishError::Contract(format!(
            "unsuccessful status: envelope={}, publish={}",
            envelope.status, envelope.publish.status
        )));
    }
    Ok(envelope.publish.into_hosted_result())
}

fn invalid_request_json(error: serde_json::Error) -> RegistryPublishError {
    HostedApiOperationError::InvalidRequest {
        operation: "registry publish request",
        message: error.to_string(),
    }
    .into()
}

fn slice_empty<T>(values: &&[T]) -> bool {
    values.is_empty()
}

#[derive(Deserialize)]
struct HostedSkillPublishEnvelope {
    status: String,
    publish: HostedSkillPublishResult,
}

#[derive(Deserialize)]
struct HostedConnectorPublishEnvelope {
    status: String,
    publish: HostedConnectorPublishResult,
}

#[derive(Deserialize)]
struct HostedAdminSkillPublishEnvelope {
    status: String,
    publish: HostedAdminSkillPublishResult,
}

#[derive(Deserialize)]
struct HostedAdminSkillPublishResult {
    status: String,
    skill_id: String,
    name: String,
    version: String,
    digest: String,
    #[serde(default)]
    profile_digest: Option<String>,
    #[serde(default)]
    record: Option<HostedAdminSkillRecord>,
    link: HostedSkillPublishLink,
}

impl HostedAdminSkillPublishResult {
    fn into_hosted_result(self) -> HostedSkillPublishResult {
        let owner = self
            .record
            .as_ref()
            .map(|record| record.owner.clone())
            .or_else(|| {
                self.skill_id
                    .split_once('/')
                    .map(|(owner, _)| owner.to_owned())
            })
            .unwrap_or_default();
        let trust_tier = self
            .record
            .as_ref()
            .and_then(|record| record.trust_tier.clone())
            .unwrap_or_else(|| "first_party".to_owned());
        HostedSkillPublishResult {
            status: self.status,
            public_url: self.link.public_url(&self.skill_id, &self.version),
            skill_id: self.skill_id,
            owner,
            name: self.name,
            version: self.version,
            digest: self.digest,
            profile_digest: self.profile_digest,
            trust_tier,
            install_command: self.link.install_command,
            run_command: self.link.run_command,
        }
    }
}

#[derive(Deserialize)]
struct HostedAdminSkillRecord {
    owner: String,
    #[serde(default)]
    trust_tier: Option<String>,
}

#[derive(Deserialize)]
struct HostedSkillPublishLink {
    install_command: CopyableCommand,
    run_command: CopyableCommand,
    #[serde(default)]
    public_url: Option<String>,
    #[serde(default)]
    link: Option<String>,
}

impl HostedSkillPublishLink {
    fn public_url(&self, skill_id: &str, version: &str) -> String {
        self.public_url
            .as_deref()
            .or(self
                .link
                .as_deref()
                .filter(|link| link.starts_with("http://") || link.starts_with("https://")))
            .map(str::to_owned)
            .unwrap_or_else(|| runx_skill_public_url(skill_id, version))
    }
}

fn runx_skill_public_url(skill_id: &str, version: &str) -> String {
    let (owner, name) = skill_id.split_once('/').unwrap_or(("", skill_id));
    format!(
        "https://runx.ai/x/{}/{}@{}",
        encode_path_component(owner),
        encode_path_component(name),
        encode_path_component(version)
    )
}

fn encode_path_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests;
