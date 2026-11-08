mod bundled;
mod connector_install;
mod http;
mod install;
mod keys;
pub(crate) mod package_bundle;
mod package_files;
pub(crate) use package_files::registry_bundle_digest;
pub use package_files::skill_content_digest;
mod package_metadata;
mod payload;
mod publish;
mod publish_closure;
mod publish_package;
mod refs;
pub mod resolve;
pub(crate) mod signature;
mod source_authority;
mod trust_record;
mod types;

pub(crate) use bundled::{bundled_identity, bundled_skills};
pub use bundled::{is_bundled_reference, materialize_bundled_skill};
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
pub use keys::{
    REGISTRY_SIGNED_MANIFEST_SCHEMA, RUNX_REGISTRY_KEYS_ENV, RegistryKey, RegistryKeyScope,
    RegistryKeysError, RegistryManifestVerificationFailure, official_registry_keys,
    registry_key_signs, registry_key_states, registry_keys, verify_registry_signed_manifest,
    verify_registry_tier_statement,
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
pub use signature::{SIGNATURE_FILE, SignedPackage, carries_signature, verify_package_signature};
pub use source_authority::{
    RegistryManifestSourceAuthority, canonical_registry_url, is_official_runx_registry_url,
    registry_manifest_source_authority_from_registry_url, registry_manifest_source_key,
};
pub(crate) use trust_record::{TrustRecord, record_trust, trusted};
pub use types::{
    AcquiredRegistrySkill, ProfileMode, RegistryAttestation, RegistryConnectorDetail,
    RegistryConnectorOperation, RegistryConnectorSearchResult, RegistryManifestSignature,
    RegistryManifestSigner, RegistryPackageFile, RegistryPublishTestReport, RegistryPublisher,
    RegistrySearchResult, RegistrySignedManifest, RegistrySkillDetail, RegistrySkillVersion,
    RegistrySourceMetadata, RegistrySubject, RegistrySubjectKind, TrustSignal, TrustTier,
    UnreadVersion,
};
