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
pub mod credential_profiles;
pub mod credentials;
pub mod doctor;
mod drivers;
pub mod effects;
pub mod error;
pub mod execution;
mod execution_environment;
pub mod export;
mod filesystem;
pub mod host;
mod hosted_api;
mod http;
mod input_contract;
mod install;
pub mod interrupt;
pub mod kernel;
mod lifecycle;
pub mod list;
mod observations;
mod output_contract;
mod packet_schemas;
mod packet_validation;
mod path_util;
pub mod paths;
mod private_file;
pub use private_file::{DirectoryProblem, PrivateFileFault};
mod process;
pub mod process_invocation;
mod provider_operations;
pub mod receipts;
pub mod registry;
mod secret_refs;
mod services;
mod skill_package;
mod time;
pub use process::detached::{
    ProcessError, SUPERVISOR_FLAG, list_processes, read_process, stop_process, supervise_from_args,
};
pub use time::now_iso8601;
pub mod tool_catalogs;

pub use execution::harness;
pub use execution::orchestrator;
pub use execution::run_record;
pub use execution::runner;
pub use execution::runner::turn::Turn;
pub use execution::skill_front;
pub use execution::skill_front::TurnSession;
pub use execution_environment::environment_requirement_statuses;
pub use tool_catalogs::native::{NativeCapabilitySnapshot, native_capability_snapshot};

pub mod adapters;

pub use adapter::{
    EphemeralValue, Executable, InvocationDiagnostics, InvocationOutput, InvocationStatus,
    SkillAdapter, SkillInvocation,
};
pub use approval::{ApprovalError, LocalApprovalGateResolver, request_approval};
pub use capability::{
    CapabilityApproval, CapabilityArtifacts, CapabilityContract, CapabilityDefinition,
    CapabilityEffect, CapabilityField, CapabilityInput, CapabilityOutput, TypedCapability,
};
pub use config::{
    ConfigError, ConfigKey, MODEL_PROVIDERS, ManagedAgentAuthMode, ManagedAgentConfig,
    ModelDialect, ModelProvider, RunxAgentConfig, RunxConfigFile, RunxCredentialProfile,
    RunxCredentialsConfig, RunxPublicConfig, clear_runx_agent_dialect,
    clear_runx_agent_endpoint_url, load_local_agent_api_key, load_local_credential_secret,
    load_local_public_api_token, load_managed_agent_config, load_runx_config_file,
    lookup_runx_config_value, mask_runx_config_file, model_provider, parse_config_key,
    remove_local_agent_api_key, remove_local_credential_secret, set_runx_agent_api_key_reference,
    store_local_credential_secret, update_runx_config_value, write_runx_config_file,
};
pub use credential_profiles::{
    CredentialProfileError, CredentialProfileSummary, list_local_credential_profiles,
    remove_local_credential_profile, set_local_credential_profile,
};
pub use credentials::{CredentialDelivery, CredentialDeliveryError, SecretEnv, SecretString};
pub use doctor::{DoctorOptions, default_doctor_options, run_doctor};
pub use effects::{
    EffectAdmission, EffectOutputRequest, EffectPreparationOutcome, EffectReceiptRequest,
    EffectStepRequest, EffectToolRequest, RuntimeEffect, RuntimeEffectError, RuntimeEffectRegistry,
};
pub use error::RuntimeError;
pub use harness::{
    HarnessExpectedStatus, HarnessFixtureError, HarnessFixtureKind, HarnessOptions,
    HarnessReplayError, HarnessReplayOutput, PackageHarnessReport, load_harness_fixture,
    replay_harness_fixture, run_harness_fixture, run_harness_fixture_with_adapter,
};
pub use host::{Host, NoopHost};
pub use hosted_api::{
    AuthenticatedHostedApiEnvironment, DEFAULT_HOSTED_API_BASE_URL, HOSTED_API_BASE_URL_ENV,
    HOSTED_API_TOKEN_ENV, HostedApiCredentialPurpose, HostedApiEnvironment, HostedApiError,
    HostedApiErrorPayload, HostedApiOperationError, HostedConnectAction, HostedConnectStart,
    HostedLoginCompleteResponse, HostedLoginStartResponse, HostedProviderTokenLoginResponse,
    ReceiptNotarization, complete_hosted_login, exchange_hosted_provider_token,
    execute_hosted_connect, hosted_api_transport, hosted_private_network_allowed,
    hosted_provider_api_transport, notarize_hosted_receipt, parse_hosted_api_error,
    read_hosted_purchase, start_hosted_login, store_authenticated_hosted_environment,
};
pub use http::RuntimeHttpError;
pub use http::{
    ExactLoopbackHttpTransport, HttpMethod, ReqwestHttpTransport, RuntimeHttpHeader,
    RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};
pub use install::{InstallStateError, installation_id};
pub use kernel::authority::RunAuthority;
pub use kernel::connectors::{
    ConnectorOrigin, ConnectorRegistryError, ConnectorScope, LoadedConnector, OperationDigests,
};
pub use kernel::instances::{InstanceError, InstanceRecord, Instances};
pub use kernel::operation::{OPERATION_EFFECT_FAMILY, OperationEffect};
pub use kernel::store::{KernelStore, KernelStoreError};
pub use list::{
    RunxListItem, RunxListItemKind, RunxListOptions, RunxListRequestedKind, RunxListStatus,
    list_authoring_primitives, list_authoring_primitives_with_effects,
};
pub use observations::Observations;
pub use orchestrator::{
    DEFAULT_MANAGED_AGENT_MAX_ROUNDS, HarnessRunRequest, LocalOrchestrator,
    MANAGED_AGENT_MAX_ROUNDS_LIMIT, ManagedAgentPolicy, OrchestratorError, PackageHarnessRequest,
    ResumeRequest, RunRequest, RunResult, RunStatus, SkillRunRequest, TurnRequest,
};
pub use paths::{
    RUNX_CWD_ENV, RUNX_HOME_ENV, RUNX_PROJECT_DIR_ENV, RUNX_RECEIPT_DIR_ENV, RunxPaths,
};
pub use provider_operations::{
    HostedProviderGrant, ProviderOperationError, ProviderOperationRequest, list_provider_grants,
    validate_provider_grant_id, validate_provider_operation,
};
pub use receipts::paths::{
    RUNTIME_RECEIPTS_DIR_CONFIG_KEY, ReceiptPathInputs, ReceiptPathSource, ReceiptStoreLabel,
    ResolvedReceiptPath, RuntimeReceiptConfig, resolve_receipt_path, safe_receipt_store_label,
};
pub use receipts::store::{
    LocalReceiptStore, ReceiptStoreError, ReceiptStoreIndex, ReceiptStoreIndexEntry,
    StoreTreeVerification, StoredReceiptTree, UnreadableReceiptFile,
};
pub use receipts::tree::{
    RuntimeReceiptResolver, validate_runtime_receipt_tree, verify_runtime_receipt_tree,
};
pub use receipts::{
    Ed25519CounterSealVerifier, Ed25519ReceiptSigner, Ed25519ReceiptVerifier, FIXTURE_RECEIPT_SEED,
    LOCAL_RECEIPT_KEY_SCHEMA, LocalReceiptKeyFault, RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV,
    RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV, RUNX_RECEIPT_SIGN_KID_ENV,
    RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_VERIFY_KID_ENV, ReceiptTrust, ReceiptTrustOrigin, ReceiptTrustSource,
    RuntimeReceiptSignatureConfig, RuntimeReceiptSignaturePolicy, RuntimeReceiptSigner,
    RuntimeReceiptSigningError, TrustedReceiptKey, local_receipt_kid, receipt_signer,
    receipt_trust, receipt_verifier,
};
pub use run_record::{RunLog, RunRecordError, RunState, RunSummary};
pub use runner::{
    GraphRun, RUNX_RUN_ID_ENV, RunClock, Runtime, RuntimeOptions, StepOutcome, StepRun,
};
pub use runx_core::kernel_eval;
pub use runx_core::policy::{ScopeGrantPolicy, missing_granted_scopes, scope_grant_allows};
pub use runx_parser::{
    PROFILE_FILE, SkillArtifactContract, SkillPackageSource, SkillRunnerDefinition,
    SkillRunnerManifest, ValidatedSkillPackage,
};
pub use runx_receipts::ReceiptTreeConfig;
pub use services::{
    VerifiedReceiptStore, WorkspaceEnv, WorkspaceEnvError, WorkspaceFileError, read_workspace_text,
};
pub use skill_package::{
    LoadedSkillPackage, SkillInspectionError, inspect_skill_package, inspect_skill_readiness,
    load_validated_skill_package, unheld_registry_refs_at,
};
pub use skill_package::{inspect_connector_readiness, resolve_local_skill_reference};
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
pub const EXECUTION_RUNTIME_RELEASE: &str = "0.10.1";

pub use services::{persist_json_artifact, read_json_artifact};

pub use skill_package::validate_skill_runner_inputs;

pub use execution::skill_context::load_context_skills;
