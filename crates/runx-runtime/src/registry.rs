mod bundled;
mod connector_install;
mod http;
mod install;
mod install_record;
pub(crate) mod package_bundle;
mod package_files;
pub use package_files::skill_content_digest;
mod package_metadata;
mod payload;
mod publish;
mod publish_closure;
mod publish_package;
mod refs;
pub mod resolve;
mod source_authority;
mod trust_anchor;
mod types;

pub(crate) use bundled::bundled_skills;
pub use bundled::materialize_bundled_skill;
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
pub(crate) use install::{copy_installed_skill, copy_installed_skills};
pub use install_record::{InstallRecord, read_install_record};
pub(crate) use install_record::{
    InstalledOrigin, TrustRecord, installed_origin, record_trust, trusted,
};
pub use package_metadata::{
    RegistryConnectorMetadata, RegistryPackageMetadata, RegistryTestCaseMetadata,
};
pub use payload::VersionPage;
pub use publish::{
    HostedAdminSkillPublishRequest, HostedConnectorPublishRequest, HostedConnectorPublishResult,
    HostedSkillPublishRequest, HostedSkillPublishResult, RegistryPublishError,
    publish_hosted_admin_skill, publish_hosted_connector, publish_hosted_skill,
};
pub use publish_closure::{
    PublishClosure, PublishClosureError, RecordInstaller, install_registry_closure,
    materialize_publish_closure,
};
pub use publish_package::{
    RegistryConnectorPackage, RegistryPublishPackage, RegistryPublishPackageError,
    RegistryPublishPackageParts, RegistryPublishPackageRequest, prepare_registry_connector_package,
    prepare_registry_publish_package, unheld_publish_registry_refs,
};
pub(crate) use refs::split_skill_id;
pub use refs::{ParsedRegistryRef, parse_registry_ref, safe_skill_package_parts};
pub use source_authority::{
    RegistryManifestSourceAuthority, canonical_registry_url, is_official_runx_registry_url,
    registry_manifest_source_authority_from_registry_url, registry_manifest_source_key,
};
pub use trust_anchor::{
    REGISTRY_SIGNED_MANIFEST_SCHEMA, RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV,
    RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV, RUNX_REGISTRY_MANIFEST_TRUST_PUBLISHER_ENV,
    RegistryManifestKeyError, RegistryManifestTrustEnvError, RegistryManifestTrustScope,
    RegistryManifestVerificationFailure, TrustedRegistryManifestKey,
    default_trusted_registry_manifest_keys, registry_manifest_key_allows,
    trusted_registry_manifest_keys_from_env, verify_registry_signed_manifest,
};
pub use types::{
    AcquiredRegistrySkill, ProfileMode, RegistryAttestation, RegistryConnectorDetail,
    RegistryConnectorOperation, RegistryConnectorSearchResult, RegistryManifestSignature,
    RegistryManifestSigner, RegistryPackageFile, RegistryPublishTestReport, RegistryPublisher,
    RegistrySearchResult, RegistrySignedManifest, RegistrySkillDetail, RegistrySkillVersion,
    RegistrySourceMetadata, RegistrySubject, RegistrySubjectKind, TrustSignal, TrustTier,
    UnreadVersion,
};
