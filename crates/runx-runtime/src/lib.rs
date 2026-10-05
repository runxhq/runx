//! Native Rust runtime skeleton for runx execution.
//!
//! The runtime owns impure boundaries: filesystem reads, subprocess execution,
//! process preparation, host reporting, and receipt emission. Pure
//! parser/core/receipt crates stay upstream of this crate.
//!
//! The root exports are a facade for CLI, SDK, and test consumers. Helper
//! surfaces stay under their owning modules: harness replay under `harness`,
//! receipt stores under `receipts`, adapter protocol under `adapter`, and
//! runtime orchestration under `runner` or `orchestrator`.

pub mod adapter;
mod adapter_pipeline;
mod agent_contract;
mod agent_invocation;
pub mod approval;
mod bytes;
mod capability;
pub mod config;
pub mod credential_resolver;
pub mod credentials;
pub mod dev;
pub mod doctor;
pub mod effects;
pub mod error;
pub mod execution;
mod execution_environment;
pub mod export;
mod filesystem;
pub mod host;
#[cfg(feature = "async-http")]
mod hosted_api;
mod http;
mod init;
mod input_contract;
pub mod interrupt;
pub mod journal;
#[cfg(feature = "mcp")]
mod json_render;
mod lifecycle;
pub mod list;
#[cfg(feature = "thread-outbox-provider")]
pub mod outbox_provider;
mod output_contract;
mod packet_schemas;
mod packet_validation;
mod path_util;
mod process;
pub mod process_invocation;
#[cfg(feature = "async-http")]
mod provider_operations;
pub mod receipts;
pub mod registry;
mod services;
mod skill_package;
mod time;
pub use time::now_iso8601;
pub mod tool_catalogs;

pub use execution::harness;
pub use execution::orchestrator;
pub use execution::runner;
pub use execution::skill_front;
pub use execution_environment::environment_requirement_statuses;
pub use tool_catalogs::native::{
    EventStoreMigrationProof, EventStoreMigrationRequest, EventStoreMigrationStatus,
    migrate_event_store,
};
#[cfg(feature = "catalog")]
pub use tool_catalogs::native::{NativeCapabilitySnapshot, native_capability_snapshot};

pub mod adapters;

pub use adapter::{
    EphemeralValue, InvocationDiagnostics, InvocationOutput, InvocationStatus, SkillAdapter,
    SkillInvocation,
};
pub use approval::{ApprovalError, LocalApprovalGateResolver, request_approval};
pub use capability::{
    CapabilityApproval, CapabilityArtifacts, CapabilityContract, CapabilityDefinition,
    CapabilityEffect, CapabilityField, CapabilityInput, CapabilityOutput, TypedCapability,
};
pub use config::{
    ConfigError, ConfigKey, ManagedAgentAuthMode, ManagedAgentConfig, RunxAgentConfig,
    RunxConfigFile, RunxCredentialProfile, RunxCredentialsConfig, RunxPublicConfig,
    clear_runx_agent_endpoint_url, load_local_agent_api_key, load_local_credential_secret,
    load_local_public_api_token, load_managed_agent_config, load_runx_config_file,
    lookup_runx_config_value, managed_agent_provider, mask_runx_config_file, parse_config_key,
    remove_local_credential_secret, resolve_path_from_user_input, resolve_runx_global_home_dir,
    resolve_runx_home_dir, resolve_runx_workspace_base, store_local_credential_secret,
    update_runx_config_value, write_runx_config_file,
};
pub use credential_resolver::{
    CredentialBindingsFile, CredentialProfileSummary, ResolvedSkillCredential,
    SkillCredentialContext, SkillCredentialError, SkillCredentialRequest,
    SkillCredentialResolution, SkillCredentialSource, bind_project_credential,
    bind_project_provider_transport, list_local_credential_profiles, load_project_bindings,
    remove_local_credential_profile, resolve_skill_credential, resolve_skill_credential_for_path,
    set_local_credential_profile,
};
pub use credentials::{
    CredentialDelivery, CredentialDeliveryError, CredentialDeliveryProfile, CredentialMaterialRole,
    CredentialResolution, CredentialResolutionRequest, CredentialSupervisor,
    InMemoryMaterialResolver, MaterialCredentialSupervisor, MaterialResolver,
    ResolvedCredentialMaterial, SecretEnv, SecretString,
};
pub use dev::{
    DevFixtureLane, DevFixtureResult, DevFixtureStatus, DevLoopOptions, DevReport, DevReportStatus,
    DevWatchOptions, DevWatchTrigger, PollingDevWatcher, dev_receipt_metadata,
    discover_fixture_paths, render_dev_result, run_dev_once, should_ignore_dev_watch_path,
};
pub use doctor::{DoctorOptions, default_doctor_options, run_doctor};
pub use effects::{
    ASSISTANT_CONFIDENTIAL_TERMS_ENV, ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV, EffectAdmission,
    EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest, EffectReplay,
    EffectReplayOutputRequest, EffectReplayReceiptRequest, EffectStepRequest, EffectToolRequest,
    NOTIFICATION_AUTHORITY_ID_ENV, NOTIFICATION_SOURCE_SET_DIGEST_ENV, NotificationAuthorityError,
    NotificationAuthorityGrant, NotificationAuthorityGrantSpec, NotificationAuthorityStatus,
    PROVIDER_MUTATE_TOOL, PROVIDER_PERMISSION_EFFECT_FAMILY, PROVIDER_PERMISSION_GRANT_ID_ENV,
    PROVIDER_PERMISSION_GRANTED_SCOPES_ENV, PROVIDER_PERMISSION_PAID_EXTERNAL_JOB_AUTHORITY_ENV,
    PROVIDER_PERMISSION_PRINCIPAL_REF_ENV, PROVIDER_READ_TOOL, ProviderAcknowledgementEvidence,
    ProviderApprovalEvidence, ProviderEffectAcknowledged, ProviderEffectAmount,
    ProviderEffectAttempt, ProviderEffectAuthority, ProviderEffectClass, ProviderEffectError,
    ProviderEffectFinality, ProviderEffectIntent, ProviderEffectIntentInput,
    ProviderEffectReadback, ProviderEffectReadbackEvidence, ProviderEffectResolved,
    ProviderEffectUnknown, ProviderPermissionAdmission, ProviderPermissionEffect,
    ProviderScopeTransportError, RuntimeEffect, RuntimeEffectError, RuntimeEffectRegistry,
    assistant_text_is_safe, decode_provider_scopes_env, encode_provider_scopes_env,
    insert_effect_verification_ref, install_notification_authority, notification_authority_status,
    notification_intent_has_reservation, revoke_notification_authority,
};
#[cfg(feature = "catalog")]
pub use effects::{
    EXTERNAL_RECEIPT_EFFECT_FAMILY, EXTERNAL_RECEIPT_VERIFY_TOOL, ExternalReceiptEffect,
};
#[cfg(feature = "catalog")]
pub use effects::{
    LocalProviderTransportReadiness, PROVIDER_PERMISSION_TRANSPORT_ENV,
    ProviderTransportPreference, preflight_local_provider_transport,
    resolve_provider_transport_preference,
};
pub use error::RuntimeError;
pub use harness::{
    HarnessExpectedStatus, HarnessFixtureError, HarnessFixtureKind, HarnessReplayError,
    HarnessReplayOutput, load_harness_fixture, run_harness_fixture,
    run_harness_fixture_with_adapter,
};
pub use host::{Host, NoopHost};
#[cfg(feature = "async-http")]
pub use hosted_api::{
    AuthenticatedHostedApiEnvironment, DEFAULT_HOSTED_API_BASE_URL, HOSTED_API_BASE_URL_ENV,
    HOSTED_API_TOKEN_ENV, HostedApiCredentialPurpose, HostedApiEnvironment, HostedApiError,
    HostedApiErrorPayload, HostedApiOperationError, HostedConnectAction, HostedConnectStart,
    HostedLoginCompleteResponse, HostedLoginStartResponse, HostedProviderTokenLoginResponse,
    HostedSkillChallenge, HostedSkillEndpointError, ReceiptPublishResponse, complete_hosted_login,
    exchange_hosted_provider_token, execute_hosted_connect, hosted_api_transport,
    hosted_private_network_allowed, hosted_provider_api_transport, parse_hosted_api_error,
    publish_hosted_receipt, request_hosted_skill_challenge, start_hosted_login,
    store_authenticated_hosted_environment,
};
pub use http::{
    ExactLoopbackHttpTransport, HttpMethod, ReqwestHttpTransport, RuntimeHttpError,
    RuntimeHttpHeader, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};
pub use init::{
    InitAction, InitError, InitGeneratedValues, RunxInitOptions, RunxInitResult, RunxInstallState,
    RunxProjectState, ensure_runx_install_state, ensure_runx_project_state, runx_init,
};
pub use journal::ExecutionJournal;
pub use list::{
    RunxListItem, RunxListItemKind, RunxListOptions, RunxListRequestedKind, RunxListStatus,
    list_authoring_primitives, list_authoring_primitives_with_effects,
};
pub use orchestrator::{
    DEFAULT_MANAGED_AGENT_MAX_ROUNDS, GraphRunRequest, HarnessRunRequest, LocalOrchestrator,
    MANAGED_AGENT_MAX_ROUNDS_LIMIT, ManagedAgentPolicy, OrchestratorError, PackageHarnessRequest,
    RunContinuation, RunRequest, RunResult, RunStatus, SkillRunRequest,
};
#[cfg(feature = "thread-outbox-provider")]
pub use outbox_provider::{
    ThreadOutboxProviderProcessOutcome, ThreadOutboxProviderProcessSupervisor,
    ThreadOutboxProviderSupervisorError, ThreadOutboxProviderSupervisorOptions,
};
#[cfg(feature = "async-http")]
pub use provider_operations::{
    HostedProviderGrant, ProviderOperationAccess, ProviderOperationError, ProviderOperationRequest,
    invoke_provider_operation, list_provider_grants, validate_provider_grant_id,
    validate_provider_operation,
};
pub use receipts::paths::{
    INIT_CWD_ENV, RUNTIME_RECEIPTS_DIR_CONFIG_KEY, RUNX_CWD_ENV, RUNX_PROJECT_DIR_ENV,
    RUNX_RECEIPT_DIR_ENV, ReceiptPathInputs, ReceiptPathSource, ReceiptStoreLabel,
    ResolvedReceiptPath, RuntimeReceiptConfig, resolve_project_runx_dir, resolve_receipt_path,
    resolve_workspace_base, safe_receipt_store_label,
};
pub use receipts::store::{
    LocalReceiptStore, ReceiptStoreError, ReceiptStoreIndex, ReceiptStoreIndexEntry,
};
pub use receipts::tree::{
    RuntimeReceiptResolver, validate_runtime_receipt_tree, verify_runtime_receipt_tree,
    verify_runtime_receipt_tree_with_policy,
};
pub use receipts::{
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, ProductionReceiptKey,
    RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_SIGN_KID_ENV, RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV,
    RUNX_RECEIPT_VERIFY_KID_ENV, ResolvedReceiptVerifier, RuntimeReceiptSignatureConfig,
    RuntimeReceiptSignaturePolicy, RuntimeReceiptSigner, RuntimeReceiptSigningError,
    RuntimeReceiptVerifierSource, receipt_verifier_from_env,
};
pub use registry::{RegistryInstallMetadataInput, registry_install_receipt_metadata};
pub use runner::{
    GraphCheckpoint, GraphRun, RUNX_MAX_FANOUT_CONCURRENCY_ENV, RUNX_RUN_ID_ENV, Runtime,
    RuntimeOptions, StepOutcome, StepRun,
};
pub use runx_core::kernel_eval;
pub use runx_core::policy::{ScopeGrantPolicy, missing_granted_scopes, scope_grant_allows};
pub use runx_parser::{
    CredentialRequirement, SkillArtifactContract, SkillExternalAdapterManifest, SkillPackageSource,
    SkillRunnerDefinition, SkillRunnerManifest, SkillSource, SkillThreadOutboxProviderSource,
    ValidatedSkillPackage,
};
pub use runx_receipts::ReceiptTreeConfig;
pub use services::{
    VerifiedReceiptStore, WorkspaceEnv, WorkspaceEnvError, WorkspaceFileError, read_workspace_text,
};
pub use skill_front::PackageHarnessReport;
pub use skill_package::{
    LoadedSkillPackage, SkillInspectionError, SkillRetentionError, inspect_local_skill_binding,
    inspect_skill_package, load_validated_skill_package, resolve_retained_skill_binding,
    retain_skill_binding,
};
pub use tool_catalogs::{
    ToolBuildOptions, ToolCatalogError, ToolInspectOptions, ToolSearchOptions, build_tool_catalogs,
    inspect_tool, inspect_tool_with_effects, search_tools, search_tools_with_effects,
};

pub const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");
/// Immutable released CLI identity included in every native execution closure.
///
/// Published Runx binaries are immutable for one CLI version. Binding this
/// value prevents a queued or resumed run from silently crossing a runtime
/// upgrade while retaining the same closure digest.
pub const EXECUTION_RUNTIME_RELEASE: &str = "0.9.1";
