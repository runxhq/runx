// Module rationale: registry wire/domain types are kept together so
// local, hosted, install, publish, trust, and signed-manifest envelopes evolve
// under one serialization contract.
use runx_contracts::maturity::MaturityTier;
use runx_contracts::{CopyableCommand, JsonValue, PaidSkillListing};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryLinkResolution {
    pub link: String,
    pub skill_id: String,
    pub version: String,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
    pub install_command: CopyableCommand,
    pub run_command: CopyableCommand,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    FirstParty,
    Verified,
    Community,
}

impl TrustTier {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::FirstParty => "first_party",
            Self::Verified => "verified",
            Self::Community => "community",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileMode {
    Portable,
    Profiled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublisher {
    pub kind: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// The registry's one signed envelope for skills and connectors.
pub use runx_contracts::{
    RegistryManifestSignature, RegistryManifestSigner, RegistrySignedManifest, RegistrySubject,
    RegistrySubjectKind,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPackageFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistryAttestation {
    pub kind: String,
    pub id: String,
    pub status: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrySourceMetadata {
    pub provider: String,
    pub repo: String,
    pub repo_url: String,
    pub skill_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_path: Option<String>,
    pub r#ref: String,
    pub sha: String,
    pub default_branch: String,
    pub event: String,
    pub immutable: bool,
    pub live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstoned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher_handle: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustSignal {
    pub id: String,
    pub label: String,
    pub status: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrySearchResult {
    pub skill_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename(deserialize = "description"))]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    pub profile_mode: ProfileMode,
    pub runner_names: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_trust_tier: Option<TrustTier>,
    pub required_scopes: Vec<String>,
    pub tags: Vec<String>,
    pub trust_tier: TrustTier,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust_signals: Vec<TrustSignal>,
    pub install_command: CopyableCommand,
    pub run_command: CopyableCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrySkillVersion {
    pub skill_id: String,
    pub owner: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub version: String,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_manifest: Option<RegistrySignedManifest>,
    pub markdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_document: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_files: Vec<RegistryPackageFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_listing: Option<PaidSkillListing>,
    pub runner_names: Vec<String>,
    pub trust_tier: TrustTier,
    #[serde(default)]
    pub maturity: MaturityTier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_audience: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_metadata: Option<RegistrySourceMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestations: Vec<RegistryAttestation>,
    pub required_scopes: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harness_cases: Vec<super::RegistryHarnessCaseMetadata>,
    pub publisher: RegistryPublisher,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrySkill {
    pub skill_id: String,
    pub owner: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub latest_version: String,
    pub latest_digest: String,
    pub versions: Vec<RegistrySkillVersion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrySkillResolution {
    pub markdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_document: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_files: Vec<RegistryPackageFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_listing: Option<PaidSkillListing>,
    pub runner_names: Vec<String>,
    pub skill_id: String,
    pub name: String,
    pub version: String,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_manifest: Option<RegistrySignedManifest>,
    pub source: String,
    pub source_label: String,
    pub trust_tier: TrustTier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
    pub install_command: CopyableCommand,
    pub run_command: CopyableCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PublishSkillMarkdownResult {
    pub status: PublishStatus,
    pub skill_id: String,
    pub name: String,
    pub version: String,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_manifest: Option<RegistrySignedManifest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_digest: Option<String>,
    pub runner_names: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
    #[serde(default)]
    pub harness: RegistryPublishHarnessReport,
    pub link: RegistryLinkResolution,
    pub record: RegistrySkillVersion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublishHarnessReport {
    pub status: String,
    pub case_count: usize,
    pub assertion_error_count: usize,
    pub assertion_errors: Vec<String>,
    pub case_names: Vec<String>,
    pub receipt_ids: Vec<String>,
    pub graph_case_count: usize,
}

impl RegistryPublishHarnessReport {
    #[must_use]
    pub fn not_declared() -> Self {
        Self {
            status: "not_declared".to_owned(),
            case_count: 0,
            assertion_error_count: 0,
            assertion_errors: Vec::new(),
            case_names: Vec::new(),
            receipt_ids: Vec::new(),
            graph_case_count: 0,
        }
    }

    #[must_use]
    pub fn failed(&self) -> bool {
        self.status == "failed"
    }
}

impl Default for RegistryPublishHarnessReport {
    fn default() -> Self {
        Self::not_declared()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishStatus {
    Published,
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrySkillDetail {
    pub skill_id: String,
    pub owner: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub version: String,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_manifest: Option<RegistrySignedManifest>,
    pub markdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_files: Vec<RegistryPackageFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_listing: Option<PaidSkillListing>,
    pub runner_names: Vec<String>,
    pub trust_tier: TrustTier,
    pub required_scopes: Vec<String>,
    pub tags: Vec<String>,
    pub publisher: RegistryPublisher,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_metadata: Option<RegistrySourceMetadata>,
    pub attestations: Vec<RegistryAttestation>,
    pub install_command: CopyableCommand,
    pub run_command: CopyableCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcquiredRegistrySkill {
    pub skill_id: String,
    pub owner: String,
    pub name: String,
    pub version: String,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_manifest: Option<RegistrySignedManifest>,
    pub markdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_document: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_files: Vec<RegistryPackageFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_listing: Option<PaidSkillListing>,
    pub runner_names: Vec<String>,
    pub trust_tier: TrustTier,
    pub publisher: RegistryPublisher,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_metadata: Option<RegistrySourceMetadata>,
    pub attestations: Vec<RegistryAttestation>,
    #[serde(default)]
    pub install_count: u64,
}

/// A published connector as the registry reads it: its current version, or
/// the one asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryConnectorDetail {
    #[serde(rename = "ref")]
    pub connector: runx_contracts::connector::ConnectorName,
    pub version: String,
    pub name: String,
    pub description: String,
    pub content_digest: runx_contracts::connector::Digest,
    pub manifest_digest: runx_contracts::connector::Digest,
    pub connector_digest: runx_contracts::connector::Digest,
    pub runtime_capability: bool,
    pub trust_tier: TrustTier,
    pub operations: Vec<RegistryConnectorOperation>,
    /// Carried as arguments; show and copy `copy_text`.
    pub install_command: CopyableCommand,
}

/// One published connector a registry search found: its current version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistryConnectorSearchResult {
    #[serde(rename = "ref")]
    pub connector: runx_contracts::connector::ConnectorName,
    pub version: String,
    pub name: String,
    pub description: String,
    pub runtime_capability: bool,
    pub trust_tier: TrustTier,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryConnectorOperation {
    pub name: runx_contracts::connector::OperationName,
    pub description: String,
}
