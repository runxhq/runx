//! Shared Rust contract types for runx JSON and protocol boundaries.

// Lets the `#[derive(RunxSchema)]` output reference `::runx_contracts::schema`
// from inside this crate, the same way serde_derive references `::serde`.
extern crate self as runx_contracts;

pub mod act;
pub mod admission;
pub mod agent_context;
pub mod artifact;
pub mod authority;
pub mod canonical_json;
pub mod connector;
pub mod connector_instance;
pub mod conversation_source;
pub mod copyable_command;
pub mod credential_delivery;
pub mod decision;
pub mod decision_adapter;
pub mod digest;
pub mod doctor;
pub mod execution;
pub mod execution_boundary;
pub mod execution_requirements;
pub mod external_job;
pub mod fixture;
pub mod host_protocol;
pub mod javascript_worker;
pub mod json;
pub mod ledger;
pub mod links;
pub mod list;
pub mod maturity;
pub mod native_packets;
pub mod operational_policy;
pub mod operational_proposal;
pub mod orchestrator_handoff;
pub mod output;
pub mod packet_id;
pub mod packet_lock;
pub mod paid_skill_listing;
pub mod provider_operation;
pub mod purchase;
pub mod purchase_claim;
pub mod purchase_request_digest;
pub mod receipt;
pub mod reference;
pub mod registry_binding;
pub mod registry_signed_manifest;
pub mod registry_taxonomy;
pub mod registry_version;
pub mod resume;
pub mod run_read;
pub mod schema;
pub mod schema_artifacts;
mod schema_reconcile;
pub mod signal;
pub mod skill_authoring;
pub mod skill_inspection;
pub mod skill_output;
pub mod source_packet;
pub mod tools;
pub mod work_outcome;
pub mod x402;

pub use act::assignment::{
    ActAssignment, ActAssignmentActor, ActAssignmentHost, ActAssignmentHostKind,
    ActAssignmentIdempotency, ActAssignmentSchema, BuildActAssignment, IntentKeyInput,
    derive_content_digest, derive_intent_key, derive_trigger_key,
};
pub use act::call::{CALL_ACT_TARGET_MAX, CALL_ACT_TEXT_MAX, CallAct};
pub use act::result::{
    ActResultEnvelope, ActResultNeedsAgentEnvelope, ActResultNeedsAgentStatus, ActResultNull,
    ActResultSignal, ActResultTerminalEnvelope, ActResultTerminalStatus,
};
pub use act::{ActForm, Criterion, CriterionBinding, CriterionStatus, Intent};
pub use admission::{
    AdmissionCheck, AdmissionReason, CheckOutcome, OperationAmount, OperationIntent, Readiness,
};
pub use agent_context::{
    AgentContextEnvelope, AgentContextProfiles, ContextArtifactMeta, ContextArtifactProducer,
    ContextEntry, ContextEntryVersion, ContextPurpose, ContextScope, ContextSharing, ContextTrust,
    ExecutionLocation, ProfileFile, ProvenanceEntry,
};
pub use artifact::{
    ARTIFACT_SCHEMA, Artifact, ArtifactProducedBy, ArtifactSchema, DigestCommitment,
};
pub use authority::{
    AuthorityApproval, AuthorityBounds, AuthorityDerivation, AuthorityEvidence,
    AuthorityEvidenceKind, AuthorityResourceFamily, AuthoritySubsetComparison,
    AuthoritySubsetProof, AuthoritySubsetRelation, AuthoritySubsetResult, AuthorityTerm,
    AuthorityUseLimit, AuthorityVerb, TermLegitimacy,
};
pub use canonical_json::{
    CanonicalJsonError, STABLE_JSON_CANONICALIZATION, canonical_digest, canonical_stable_json,
    to_canonical_bytes, write_canonical_json_fragment,
};
pub use connector::{
    ConnectorManifest, ConnectorName, ConnectorOperation, Digest, InstanceId, OperationAccess,
    OperationName, OperationVerb, Placement,
};
pub use connector_instance::{ConnectorInstance, InstanceAuthority};
pub use conversation_source::{ConversationObservation, ConversationPage};
pub use copyable_command::{CommandShell, CopyableCommand, render_posix_command};
pub use credential_delivery::{
    CredentialDeliveryMode, CredentialDeliveryObservation, CredentialDeliveryObservationSchema,
    CredentialDeliveryObservationStatus, CredentialDeliveryPurpose, CredentialMaterialRole,
};
pub use decision::{Decision, DecisionChoice};
pub use decision_adapter::{DecisionObservation, DecisionOrigin};
pub use digest::{ContentDigest, DigestAlgorithm, hex_lower, sha256_hex, sha256_prefixed};
pub use doctor::{
    DoctorDiagnostic, DoctorDiagnosticSeverity, DoctorLocation, DoctorRepair,
    DoctorRepairConfidence, DoctorRepairKind, DoctorRepairRisk, DoctorReport, DoctorReportSchema,
    DoctorStatus, DoctorSummary,
};
pub use execution::{
    ArtifactDeclaration, IdempotencyPolicy, InputDefinition, call_input_schema,
    input_contract_schema, input_contract_schema_with_examples,
};
pub use execution_boundary::{ExecutionBoundaryKind, ExecutionBoundaryObservation};
pub use execution_requirements::{
    AgentExecutionRequirements, EnvironmentRequirementStatus, EnvironmentRequirements,
    ExecutionRequirements,
};
pub use external_job::{
    EXTERNAL_JOB_CONTINUATION_SCHEMA, EXTERNAL_JOB_SCHEDULE_INTENT_SCHEMA,
    EXTERNAL_JOB_SCHEDULE_SCHEMA, EXTERNAL_JOB_STAGE_REQUEST_SCHEMA,
    EXTERNAL_JOB_STAGE_RESULT_SCHEMA, ExternalJobAttemptLimit, ExternalJobCheckpoint,
    ExternalJobContinuation, ExternalJobDeadlineMillis, ExternalJobDelayMillis, ExternalJobFailure,
    ExternalJobSchedule, ExternalJobScheduleIntent, ExternalJobScheduleIntentSchema,
    ExternalJobStage, ExternalJobStageRequest, ExternalJobStageResult, ExternalJobState,
};
pub use fixture::{Fixture, FixtureLane};
pub use host_protocol::{
    AgentActInvocation, AllowedOperation, ApprovalDecision, ApprovalGate, BoundClosure,
    CatalogRule, CatalogTrust, ExecutionEvent, HostNeedsAgentState, HostRunApproval,
    HostRunApprovalDecision, HostRunKind, HostRunLineage, HostRunLineageKind, HostRunResult,
    HostRunState, HostRunVerification, HostRunVerificationStatus, HostTerminalState, LoadedPackage,
    LoadedRunner, LoadedRunnerInput, PURCHASE_BALANCE_CHOICE, PurchaseMethod,
    PurchasePaymentOption, PurchaseQuote, PurchaseRequest, Question, QuestionOption,
    ResolutionRequest, ResolutionResponse, ResolutionResponseActor, RetryResend, SkillAllowance,
    SkillManual, TURN_READ_PAGE_CHARS, TurnAllowance, TurnCallOutcome, TurnCallResult,
    TurnContextLine, TurnEnvelope, TurnInvocation, TurnLoad, TurnLocator, TurnReadPage, TurnStatus,
    TurnTarget, TurnTool, check_answer,
};
pub use json::{
    JsonNumber, JsonObject, JsonValue, MAX_PORTABLE_INTEGER, json_bool_field, json_object,
    json_object_field, json_string_field,
};
pub use ledger::{
    LedgerCanonicalization, LedgerChain, LedgerChainVersion, LedgerDigestHex, LedgerEntry,
    LedgerEntryMeta, LedgerEntrySchemaVersion, LedgerPayload, LedgerPayloadVersion, LedgerProducer,
};
pub use links::{DuplicateCandidate, Links};
pub use list::{
    RunxListEmit, RunxListItem, RunxListItemKind, RunxListReport, RunxListRequestedKind,
    RunxListSchema, RunxListSource, RunxListStatus, RunxListVisibility,
};
pub use native_packets::{
    ATTESTATION_MAX_ACTION_BYTES, ATTESTATION_MAX_CLAIM_BYTES, ATTESTATION_MAX_EVIDENCE,
    ATTESTATION_MAX_LOCATOR_BYTES, ATTESTATION_MAX_PRINCIPAL_BYTES, ATTESTATION_MAX_PROVES_BYTES,
    ApprovalDecisionActor, ApprovalDecisionPacket, ApprovalDecisionStatus, Attestation,
    AttestationDecision, AttestationEvidence, AttestationGap, AttestationProofBoundary,
    AttestationSchema, CLI_HELP_MAX_ARGS, COMMAND_MAX_ENV, COMMAND_MAX_ERRORS,
    COMMAND_MAX_TIMEOUT_MS, COMMAND_MIN_TIMEOUT_MS, COMMAND_OUTPUT_MAX_BYTES, CatalogHit,
    CatalogPackageKind, CatalogRunner, CatalogSource, CliHelp, CliHelpFlag, CommandDecision,
    CommandError, CommandExecution, CommandExecutionSchema, CommandOutputMode, CommandPlan,
    CommandPlanSchema, ConnectorInspection, DataComparison, DataDigest, DataOperationResult,
    DataOperationStatus, DataStopCondition, DataValidation, DataValidationIssue,
    DataValueValidation, ExternalLedgerNotRequested, FS_BUNDLE_MAX_BYTES, FS_BUNDLE_MAX_FILES,
    FS_LIST_MAX_ENTRIES, FS_SEARCH_MAX_CONTEXT_LINES, FS_SEARCH_MAX_MATCHES, FileAppliedDelete,
    FileAppliedWrite, FileApplyBundle, FileBoundDelete, FileBundleDelete, FileBundleWrite,
    FileDeleteStatus, FileEdit, FileEntryKind, FileInverseBundle, FileList, FileListEntry,
    FileRead, FileReadBundle, FileSearch, FileSearchMatch, FileWrite, GIT_LOG_MAX_COMMITS,
    GitBlobDigest, GitBranch, GitCommit, GitDiff, GitLog, GitPatch, GitShow, GitStatus,
    HarnessContext, InspectedOperation, LocalArtifact, LocalArtifactPage, MatchedReceipt,
    MissingFile, PROCESS_ENV_NAME_PATTERN, PolicyLint, PolicyLintEngine, PolicyLintStatus,
    ProcessEnvName, ProcessExit, ProcessExitReason, ProcessKind, ProcessList, ProcessOutput,
    ProcessOutputChunk, ProcessState, ProcessStream, ProviderNotCalled, RECEIPT_TOOL_MAX_RECEIPTS,
    RUN_LIST_MAX_RUNS, ReceiptProof, ReceiptProofDecision, ReceiptProofSchema, ReceiptProofStore,
    ReceiptProofVerification, ReceiptQuery, ReceiptQuerySchema, ReceiptQuerySource,
    ReceiptQueryVerification, RegistryInspection, RegistrySearchPacket, RunList, RunListEntry,
    RunListSchema, RuntimeSealPending, RunxProcess, TerminalSize,
};
pub use operational_policy::{
    OperationalPolicy, OperationalPolicyAction, OperationalPolicyAdmission,
    OperationalPolicyAdmissionRequest, OperationalPolicyAdmissionStatus,
    OperationalPolicyAutomationPermissions, OperationalPolicyDedupePolicy,
    OperationalPolicyDedupeStrategy, OperationalPolicyDuplicateBehavior, OperationalPolicyError,
    OperationalPolicyMissingBehavior, OperationalPolicyOutcomeCloseMode,
    OperationalPolicyOutcomePolicy, OperationalPolicyOwnerRoute, OperationalPolicyPublishMode,
    OperationalPolicyReadback, OperationalPolicyRunnerReadback, OperationalPolicyRunnerRule,
    OperationalPolicyRunnerState, OperationalPolicySchema, OperationalPolicySourceReadback,
    OperationalPolicySourceRule, OperationalPolicySourceThreadPolicy,
    OperationalPolicyTargetReadback, OperationalPolicyTargetRule,
    OperationalPolicyValidationFinding, admit_operational_policy_request,
    lint_operational_policy_contract, project_operational_policy_readback,
    validate_operational_policy_contract, validate_operational_policy_semantics,
};
pub use operational_proposal::{
    OPERATIONAL_PROPOSAL_SCHEMA, OperationalProposal, OperationalProposalAuthority,
    OperationalProposalHumanGate, OperationalProposalIdempotency, OperationalProposalOutcome,
    OperationalProposalRecommendedAction, OperationalProposalRedactionStatus,
    OperationalProposalSchema,
};
pub use orchestrator_handoff::{
    OrchestratorExecutionContext, OrchestratorHandoffBinding, OrchestratorHandoffContext,
    OrchestratorHandoffDelivery, OrchestratorHandoffIdempotency, OrchestratorReceiptExpectations,
    OrchestratorReceiverValidation,
};
pub use output::{
    Output, OutputContractParseError, OutputField, OutputFieldSpec, OutputType,
    OutputValidationError, output_contract_digest, output_value_schema, parse_output_contract,
    validate_output_value,
};
pub use packet_id::{
    PACKET_ID_EXPECTED, PACKET_ID_MAX_LENGTH, PACKET_ID_PATTERN, PacketId, is_packet_id,
};
pub use packet_lock::{PacketLock, PacketLockEntry, PacketLockSchema};
pub use paid_skill_listing::{
    PAID_SKILL_LISTING_SCHEMA, PaidSkillActsOn, PaidSkillExecutorBinding, PaidSkillListing,
    PaidSkillOfferTerms, PaidSkillOffers, PaidSkillPrice, PaidSkillPriceCase, PaidSkillRunnerOffer,
    PriceChoice, PriceError,
};
pub use provider_operation::ProviderOperationPacket;
pub use purchase::{
    CANCEL_PURCHASE, CANCEL_PURCHASE_REQUEST_SCHEMA, CANCEL_PURCHASE_RESULT_SCHEMA,
    CancelPurchaseRequest, CancelPurchaseResult, CurrencyCode, EXECUTE_PURCHASE,
    EXECUTE_PURCHASE_REQUEST_SCHEMA, EXECUTE_PURCHASE_RESULT_SCHEMA, EmbeddedOfferVersionBinding,
    ExecutePurchaseRequest, ExecutePurchaseResult, GET_PURCHASE, GET_PURCHASE_REQUEST_SCHEMA,
    GET_PURCHASE_RESULT_SCHEMA, GetPurchaseRequest, GetPurchaseResult,
    OFFER_VERSION_BINDING_SCHEMA, OfferVersionBinding, PURCHASE_READ_SCHEMA, PURCHASE_SCHEMA,
    PaymentIdempotencyBinding, PaymentReference, PortableAmountMinor, PrincipalReference, Purchase,
    PurchaseAdmission, PurchaseCanonicalizerVersion, PurchaseExecutionState, PurchaseHold,
    PurchaseOutcomeGate, PurchasePaymentChallenge, PurchasePaymentMethod,
    PurchasePaymentOptionBinding, PurchasePaymentState, PurchaseRead, PurchaseRefusalCode,
    PurchaseRefusalReason, QUOTE_PURCHASE, QUOTE_PURCHASE_REQUEST_SCHEMA,
    QUOTE_PURCHASE_RESULT_SCHEMA, QuotePurchaseAdmission, QuotePurchaseRequest,
    QuotePurchaseResult, RUNX_BALANCE_SETTLEMENT_FAMILY, SettlementFamilies, SettlementFamily,
};
pub use purchase_claim::{
    BuyerKey, BuyerSignature, PURCHASE_CLAIM_SCHEMA, PURCHASE_READ_REQUEST_SCHEMA, PurchaseClaim,
    purchase_claim_message, purchase_read_request_message,
};
pub use purchase_request_digest::{
    PURCHASE_REQUEST_DIGEST_SCHEMA, digest_cancel_purchase_request,
    digest_execute_purchase_request, digest_quote_purchase_request,
};
pub use receipt::{
    Lineage, RECEIPT_CANONICALIZATION, RECEIPT_SCHEMA, Receipt, ReceiptAct, ReceiptAuthority,
    ReceiptCanonicalization, ReceiptEnforcement, ReceiptIssuer, ReceiptIssuerType, ReceiptSchema,
    ReceiptSignature, RunOutcome, Seal, SealReason, SignatureAlgorithm, Subject, SubjectKind,
};
pub use reference::{
    ActAddress, ExternalReferenceKind, ProofKind, Reference, ReferenceLink, ReferenceType,
    RunxPrincipalId,
};
pub use registry_binding::{
    RegistryBinding, RegistryBindingHarness, RegistryBindingRegistry, RegistryBindingSchema,
    RegistryBindingSkill, RegistryBindingState, RegistryBindingUpstream, RegistryHarnessState,
    TrustTier,
};
pub use registry_signed_manifest::{
    REGISTRY_MANIFEST_SIGNATURE_ALG, REGISTRY_MANIFEST_SIGNATURE_PREFIX,
    REGISTRY_SIGNED_MANIFEST_SCHEMA, RegistryContentDigestError, RegistryContentFile,
    RegistryManifestSignature, RegistryManifestSigner, RegistrySignedManifest, RegistrySubject,
    RegistrySubjectKind, is_registry_subject_name, registry_content_digest,
};
pub use registry_taxonomy::{
    Category as RegistryCategory, TaxonomySubject, UNCATEGORIZED, registry_categories,
    registry_category, validate_registry_category,
};
pub use registry_version::{
    REGISTRY_VERSION_MAX_LENGTH, REGISTRY_VERSION_PATTERN, is_registry_version,
    validate_registry_version,
};
pub use resume::{
    BoundAnswer, BoundApproval, CloseOutcome, ResumeDocument, RunClose, StandingApproval,
    StandingCommands, StandingEdits,
};
pub use run_read::{
    RunCallOutcome, RunCallOutput, RunDriver, RunEventFact, RunEventRecord, RunHold, RunRead,
    RunReadSchema, RunReadSummary, RunState, WaitedRequest,
};
pub use schema_artifacts::{
    SchemaArtifact, generated_schema_artifacts, public_packet_artifact, schema_artifact,
};
pub use schema_reconcile::{SchemaDrift, reconcile_schema_artifacts};
pub use signal::{
    SIGNAL_SCHEMA, Signal, SignalAuthenticity, SignalSchema, SignalTrustLevel, signal_type,
};
pub use skill_authoring::{
    SKILL_APPLY_RESULT_SCHEMA, SKILL_ARCHITECTURE_DECISION_SCHEMA, SKILL_ARCHITECTURE_PLAN_SCHEMA,
    SKILL_CHANGE_BUNDLE_SCHEMA, SKILL_CHANGE_DRAFT_SCHEMA, SKILL_VALIDATION_RESULT_SCHEMA,
    SkillApplyResult, SkillApplyResultSchema, SkillApplyVerdict, SkillApprovalRequirement,
    SkillArchitectureChoice, SkillArchitectureDecision, SkillArchitectureDecisionSchema,
    SkillArchitecturePlan, SkillArchitecturePlanSchema, SkillAuthoringContext,
    SkillBehaviorDecision, SkillCatalogEntry, SkillChainPlan, SkillChainUse, SkillChangeBundle,
    SkillChangeBundleSchema, SkillChangeDecision, SkillChangeDraft, SkillChangeDraftSchema,
    SkillDirectUse, SkillEffectClass, SkillEffectRequirement, SkillExecutionLane,
    SkillExpectedOutput, SkillFileWrite, SkillIdentityAction, SkillIdentityDecision,
    SkillKnowledge, SkillNativeReuseEvidence, SkillPackageDelta, SkillPackageFile,
    SkillPackageMetrics, SkillPackageVisibility, SkillProofKind, SkillProofRequirement,
    SkillResourceBudget, SkillValidation, SkillValidationCheck, SkillValidationCheckStatus,
    SkillValidationResult, SkillValidationResultSchema, SkillValidationSchema,
    SkillValidationVerdict,
};
pub use source_packet::{SOURCE_PACKET_SCHEMA, SourcePacket, SourcePacketSchema};
pub use tools::{
    RuntimeCommand, ToolCommandInputMode, ToolIdempotencyPolicy, ToolInput, ToolManifest,
    ToolManifestSchema, ToolSource,
};
pub use work_outcome::{WorkOutcome, WorkProposal, WorkState};
pub use x402::{
    RUNX_PURCHASE_EXTENSION_KEY, RUNX_X402_PURCHASE_EXTENSION_SCHEMA, RunxX402PurchaseExtension,
    RunxX402PurchaseExtensionInfo, X402_PAYMENT_PAYLOAD_SCHEMA_ID, X402_PAYMENT_REQUIRED_HEADER,
    X402_PAYMENT_REQUIRED_SCHEMA_ID, X402_PAYMENT_REQUIREMENTS_SCHEMA_ID,
    X402_PAYMENT_RESPONSE_HEADER, X402_PAYMENT_SIGNATURE_HEADER, X402_PROTOCOL_VERSION,
    X402_RESOURCE_INFO_SCHEMA_ID, X402_SETTLE_RESPONSE_SCHEMA_ID, X402_UPSTREAM_COMMIT,
    X402_UPSTREAM_PACKAGE, X402_UPSTREAM_PACKAGE_VERSION, X402AcceptedRequirements, X402IconUrl,
    X402Network, X402PaymentPayload, X402PaymentRequired, X402PaymentRequirements,
    X402PositiveNumber, X402ResourceInfo, X402ServiceName, X402SettleResponse, X402Tag, X402Tags,
    X402Version2, parse_runx_purchase_extension, runx_purchase_extension_value,
};

pub use skill_inspection::{
    EnvironmentReadiness, EnvironmentReadinessStatus, InspectedInput, InspectedOutput,
    InspectionReadiness, InspectionReadinessStatus, InspectionResume, OperatorJourney,
    RunnerInspection, RunnerInspectionEntry, SkillCapabilities, SkillCatalog, SkillInspection,
    SkillInspectionSchema, SkillInspectionStatus,
};
pub use skill_output::{ProjectArtifact, SkillOutput};
