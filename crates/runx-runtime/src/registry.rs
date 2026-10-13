mod bundled;
mod connector_install;
mod http;
mod install;
mod local;
pub(crate) mod package_bundle;
mod package_files;
pub use package_files::skill_content_digest;
mod package_metadata;
mod payload;
mod publish;
mod publish_closure;
mod publish_package;
mod refs;
mod source_authority;
mod trust_anchor;
mod types;

pub(crate) use bundled::bundled_skills;
pub use bundled::{bundled_runx_manual, materialize_bundled_skill};
pub(crate) use connector_install::write_connector_version;
pub use connector_install::{
    ConnectorInstallError, InstallRegistryConnectorOptions, InstalledRegistryConnector,
    RegistryConnectorVersion, install_registry_connector,
};
pub use http::{AcquireOptions, RegistryClient, RegistryClientError};
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
    HostedAdminSkillPublishRequest, HostedConnectorPublishRequest, HostedConnectorPublishResult,
    HostedSkillPublishRequest, HostedSkillPublishResult, RegistryPublishError,
    publish_hosted_admin_skill, publish_hosted_connector, publish_hosted_skill,
};
pub use publish_closure::{PublishClosure, PublishClosureError, materialize_publish_closure};
pub use publish_package::{
    RegistryConnectorPackage, RegistryPublishPackage, RegistryPublishPackageError,
    RegistryPublishPackageParts, RegistryPublishPackageRequest, prepare_registry_connector_package,
    prepare_registry_publish_package, unheld_publish_registry_refs,
};
pub use refs::{
    ParsedRegistryRef, RegistryResolveError, parse_registry_ref, safe_skill_package_parts,
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
    RegistryAttestation, RegistryConnectorDetail, RegistryConnectorOperation,
    RegistryLinkResolution, RegistryManifestSignature, RegistryManifestSigner, RegistryPackageFile,
    RegistryPublishHarnessReport, RegistryPublisher, RegistrySearchResult, RegistrySignedManifest,
    RegistrySkill, RegistrySkillDetail, RegistrySkillResolution, RegistrySkillVersion,
    RegistrySourceMetadata, RegistrySubject, RegistrySubjectKind, ResolvedRegistryRef, TrustSignal,
    TrustTier,
};
