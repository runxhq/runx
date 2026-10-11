mod bundled;
mod http;
mod index;
mod install;
mod local;
pub(crate) mod package_bundle;
mod package_files;
mod package_metadata;
mod payload;
mod publish;
mod publish_package;
mod refs;
mod source_authority;
mod trust_anchor;
mod types;

use runx_contracts::{JsonNumber, JsonObject, JsonValue};

pub(crate) use bundled::bundled_skills;
pub use bundled::{bundled_runx_manual, materialize_bundled_skill};
pub use http::{AcquireOptions, RegistryClient, RegistryClientError};
pub use index::{
    IndexError, IndexRepositoryOptions, IndexResponse, IndexWarning, IndexedListing, IndexedRepo,
    index_repository,
};
pub use install::{
    InstallCandidate, InstallError, InstallLocalSkillOptions, InstallLocalSkillResult,
    InstallStatus, install_local_skill,
};
pub use local::{
    FileRegistryStore, IngestSkillOptions, LocalRegistryError, PublishSkillMarkdownOptions,
    PutVersionOptions, RegistryResolveOptions, RegistrySearchOptions, build_skill_id,
    ingest_skill_markdown, publish_skill_markdown, read_registry_skill, resolve_registry_skill,
    resolve_runx_link, runx_link_for_version, search_registry_with_options, slugify,
    split_skill_id,
};
pub use package_metadata::{
    RegistryConnectorMetadata, RegistryHarnessCaseMetadata, RegistryPackageMetadata,
};
pub use publish::{
    HostedAdminSkillPublishRequest, HostedSkillPublishRequest, HostedSkillPublishResult,
    RegistryPublishError, publish_hosted_admin_skill, publish_hosted_skill,
};
pub use publish_package::{
    RegistryConnectorPackage, RegistryPublishPackage, RegistryPublishPackageError,
    RegistryPublishPackageParts, RegistryPublishPackageRequest, prepare_registry_connector_package,
    prepare_registry_publish_package,
};
pub use refs::{
    ParsedRegistryRef, RegistryResolveError, materialization_cache_path,
    materialization_digest_marker, parse_registry_ref, safe_skill_package_parts,
};
pub use source_authority::{
    RUNX_REGISTRY_SOURCE_AUTHORITY_ENV, RegistryManifestSourceAuthority, canonical_registry_url,
    is_official_runx_registry_url, registry_manifest_source_authority_from_env,
    registry_manifest_source_authority_from_registry_dir,
    registry_manifest_source_authority_from_registry_url, registry_manifest_source_key,
};
pub use trust_anchor::{
    REGISTRY_SIGNED_MANIFEST_SCHEMA, RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV,
    RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV, RUNX_REGISTRY_MANIFEST_TRUST_OWNER_ENV,
    RegistryManifestKeyError, RegistryManifestTrustEnvError, RegistryManifestTrustScope,
    RegistryManifestVerificationFailure, TrustedRegistryManifestKey,
    default_trusted_registry_manifest_keys, registry_manifest_key_allows,
    trusted_registry_manifest_keys_from_env, trusted_registry_manifest_keys_from_env_with_source,
    verify_registry_signed_manifest,
};
pub use types::{
    AcquiredRegistrySkill, ProfileMode, PublishSkillMarkdownResult, PublishStatus,
    RegistryAttestation, RegistryLinkResolution, RegistryManifestSignature, RegistryManifestSigner,
    RegistryPackageFile, RegistryPublishHarnessReport, RegistryPublisher, RegistrySearchResult,
    RegistrySignedManifest, RegistrySkill, RegistrySkillDetail, RegistrySkillResolution,
    RegistrySkillVersion, RegistrySourceMetadata, ResolvedRegistryRef, TrustSignal, TrustTier,
};

#[derive(Clone, Copy, Debug)]
pub struct RegistryInstallMetadataInput<'a> {
    pub candidate: &'a InstallCandidate,
    pub install: &'a InstallLocalSkillResult,
    pub acquisition: Option<&'a AcquiredRegistrySkill>,
}

#[must_use]
pub fn registry_install_receipt_metadata(input: RegistryInstallMetadataInput<'_>) -> JsonObject {
    let mut metadata = JsonObject::new();
    insert_string(&mut metadata, "ref", &input.candidate.r#ref);
    insert_optional_string(&mut metadata, "skill_id", input.install.skill_id.as_deref());
    insert_optional_string(&mut metadata, "version", input.install.version.as_deref());
    insert_string(&mut metadata, "digest", &input.install.digest);
    insert_optional_string(
        &mut metadata,
        "profile_digest",
        input.install.profile_digest.as_deref(),
    );
    insert_optional_string(
        &mut metadata,
        "package_digest",
        input.candidate.package_digest.as_deref(),
    );
    insert_optional_string(
        &mut metadata,
        "trust_tier",
        input.install.trust_tier.as_ref().map(trust_tier_value),
    );
    if let Some(acquisition) = input.acquisition {
        metadata.insert(
            "publisher".to_owned(),
            JsonValue::Object(publisher_metadata(&acquisition.publisher)),
        );
        metadata.insert(
            "install_count".to_owned(),
            JsonValue::Number(JsonNumber::U64(acquisition.install_count)),
        );
    }
    insert_string(&mut metadata, "source_label", &input.install.source_label);
    insert_string(
        &mut metadata,
        "destination",
        &input.install.destination.display().to_string(),
    );
    insert_string(
        &mut metadata,
        "status",
        match input.install.status {
            InstallStatus::Installed => "installed",
            InstallStatus::Unchanged => "unchanged",
        },
    );
    metadata
}

fn publisher_metadata(publisher: &RegistryPublisher) -> JsonObject {
    let mut metadata = JsonObject::new();
    insert_string(&mut metadata, "kind", &publisher.kind);
    insert_string(&mut metadata, "id", &publisher.id);
    insert_optional_string(&mut metadata, "handle", publisher.handle.as_deref());
    insert_optional_string(
        &mut metadata,
        "display_name",
        publisher.display_name.as_deref(),
    );
    metadata
}

fn insert_string(metadata: &mut JsonObject, key: &str, value: &str) {
    metadata.insert(key.to_owned(), JsonValue::String(value.to_owned()));
}

fn insert_optional_string(metadata: &mut JsonObject, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        insert_string(metadata, key, value);
    }
}

fn trust_tier_value(value: &TrustTier) -> &'static str {
    match value {
        TrustTier::FirstParty => "first_party",
        TrustTier::Verified => "verified",
        TrustTier::Community => "community",
    }
}
