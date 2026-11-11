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
pub mod decision;
pub mod digest;
pub mod doctor;
pub mod eval_evidence;
pub mod execution;
pub mod execution_boundary;
pub mod execution_requirements;
pub mod host_protocol;
pub mod input_digest;
pub mod javascript_worker;
pub mod json;
pub mod list;
pub mod maturity;
pub mod native_packets;
pub mod operational_policy;
pub mod orchestrator_handoff;
pub mod output;
pub mod package_evals;
pub mod packet_id;
pub mod paid_skill_listing;
pub mod provider_operation;
pub mod purchase;
pub mod receipt;
pub mod reference;
pub mod registry_signed_manifest;
pub mod registry_taxonomy;
pub mod registry_tier_statement;
pub mod registry_version;
pub mod resume;
pub mod run_read;
pub mod schema;
pub mod schema_artifacts;
pub mod schema_definitions;
mod schema_reconcile;
pub mod skill_authoring;
pub mod skill_inspection;
pub mod skill_output;
pub mod source_packet;
pub mod tools;
pub mod verdict;
pub mod work_outcome;
pub mod x402;

pub use act::call::{CALL_ACT_TARGET_MAX, CALL_ACT_TEXT_MAX, CallAct};
pub use act::{ActForm, Criterion, CriterionBinding, CriterionStatus, Intent};
pub use admission::{
    AdmissionCheck, AdmissionReason, CheckOutcome, OperationAmount, OperationIntent, Readiness,
};
pub use agent_context::{
    AgentContextEnvelope, AgentContextProfiles, ContextEntry, ContextEntrySchema, ContextLabels,
    ContextOrigin, ContextProducer, ContextPurpose, ContextSharing, ContextTrust,
    ExecutionLocation, ProfileFile, ProvenanceEntry,
};
pub use artifact::{
    ARTIFACT_SCHEMA, Artifact, ArtifactLabels, ArtifactPage, ArtifactProducedBy, ArtifactSchema,
};
pub use authority::{
    AuthorityApproval, AuthorityBounds, AuthorityDerivation, AuthorityEvidence,
    AuthorityEvidenceKind, AuthoritySubsetComparison, AuthoritySubsetProof,
    AuthoritySubsetRelation, AuthoritySubsetResult, AuthorityTerm, AuthorityUseLimit,
    AuthorityVerb, Effects, PackagePattern, Reach, TermLegitimacy, every_package,
};
pub use canonical_json::{
    CanonicalJsonError, STABLE_JSON_CANONICALIZATION, canonical_digest, canonical_stable_json,
    to_canonical_bytes,
};
pub use connector::{
    ConnectorManifest, ConnectorName, ConnectorOperation, Digest, InstanceId, OperationAccess,
    OperationName, OperationVerb, Placement,
};
pub use connector_instance::{ConnectorInstance, InstanceAuthority};
pub use conversation_source::{ConversationObservation, ConversationPage};
pub use copyable_command::{CommandShell, CopyableCommand, render_posix_command};
pub use decision::{Decision, DecisionChoice};
pub use digest::{ContentDigest, DigestAlgorithm, hex_lower, sha256_hex, sha256_prefixed};
pub use doctor::{
    DoctorDiagnostic, DoctorDiagnosticSeverity, DoctorLocation, DoctorRepair,
    DoctorRepairConfidence, DoctorRepairKind, DoctorRepairRisk, DoctorReport, DoctorReportSchema,
    DoctorStatus, DoctorSummary,
};
pub use eval_evidence::{
    EVAL_EVIDENCE_SCHEMA, EvalAttempt, EvalAttempts, EvalEvidence, EvalEvidenceKind,
    EvalEvidenceSchema, EvalResults, HostEvidence, QuerySelection, Tally,
};
pub use execution::{
    InputDefinition, call_input_schema, input_contract_schema, input_contract_schema_with_examples,
};
pub use execution_boundary::{ExecutionBoundaryKind, ExecutionBoundaryObservation};
pub use execution_requirements::{
    AgentExecutionRequirements, EnvironmentRequirementStatus, EnvironmentRequirements,
};
pub use host_protocol::{
    AllowedOperation, ApprovalGate, BoundClosure, CallTarget, CatalogRule, CatalogTrust,
    ExecutionEvent, LoadedMethod, LoadedMethodInput, LoadedPackage, PURCHASE_BALANCE_CHOICE,
    ProjectBinding, PurchaseMethod, PurchasePaymentOption, PurchaseQuote, PurchaseRequest,
    Question, QuestionOption, RESERVED_ANSWER_FIELDS, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor, RetryResend, SkillAllowance, SkillManual, TURN_READ_PAGE_CHARS,
    TurnAllowance, TurnCallOutcome, TurnCallResult, TurnContextLine, TurnInvocation, TurnLoad,
    TurnLocator, TurnReadPage, TurnRequirement, TurnStatus, TurnTool, TurnUsage, TurnView,
    TurnViewEnvelope, check_answer,
};
pub use json::{JsonNumber, JsonObject, JsonValue, MAX_PORTABLE_INTEGER, json_string_field};
pub use list::{
    RunxListItem, RunxListItemKind, RunxListOutputPacket, RunxListReport, RunxListRequestedKind,
    RunxListSchema, RunxListSource, RunxListStatus, RunxListVisibility,
};
pub use native_packets::{
    ATTESTATION_MAX_ACTION_BYTES, ATTESTATION_MAX_CLAIM_BYTES, ATTESTATION_MAX_EVIDENCE,
    ATTESTATION_MAX_LOCATOR_BYTES, ATTESTATION_MAX_PRINCIPAL_BYTES, ATTESTATION_MAX_PROVES_BYTES,
    ApprovalDecisionActor, ApprovalDecisionPacket, ApprovalDecisionStatus, Attestation,
    AttestationDecision, AttestationEvidence, AttestationGap, AttestationProofBoundary,
    AttestationSchema, CLI_HELP_MAX_ARGS, COMMAND_MAX_ENV, COMMAND_MAX_ERRORS,
    COMMAND_MAX_TIMEOUT_MS, COMMAND_MIN_TIMEOUT_MS, COMMAND_OUTPUT_MAX_BYTES, CatalogHit,
    CatalogMethod, CatalogPackageKind, CatalogSource, CliHelp, CliHelpFlag, CommandDecision,
    CommandError, CommandExecution, CommandExecutionSchema, CommandOutputMode, CommandPlan,
    CommandPlanSchema, ConnectorInspection, DataComparison, DataDigest, DataValidation,
    DataValidationIssue, DataValueValidation, ExternalLedgerNotRequested, FS_BUNDLE_MAX_BYTES,
    FS_BUNDLE_MAX_FILES, FS_LIST_MAX_ENTRIES, FS_SEARCH_MAX_CONTEXT_LINES, FS_SEARCH_MAX_MATCHES,
    FileAppliedDelete, FileAppliedWrite, FileApplyBundle, FileBoundDelete, FileBundleWrite,
    FileDeleteStatus, FileEdit, FileEntryKind, FileInverseBundle, FileList, FileListEntry,
    FileRead, FileReadBundle, FileSearch, FileSearchMatch, FileWrite, GIT_LOG_MAX_COMMITS,
    GitBlobDigest, GitBranch, GitCommit, GitDiff, GitLog, GitPatch, GitShow, GitStatus,
    InspectedOperation, MatchedReceipt, MissingFile, PROCESS_ENV_NAME_PATTERN, PolicyLint,
    PolicyLintEngine, PolicyLintStatus, ProcessEnvName, ProcessExit, ProcessExitReason,
    ProcessKind, ProcessList, ProcessOutput, ProcessOutputChunk, ProcessState, ProcessStream,
    ProviderNotCalled, RECEIPT_TOOL_MAX_RECEIPTS, RUN_LIST_MAX_RUNS, ReceiptProof,
    ReceiptProofDecision, ReceiptProofSchema, ReceiptProofStore, ReceiptProofVerification,
    ReceiptQuery, ReceiptQuerySchema, ReceiptQuerySource, ReceiptQueryVerification, ReceiptRead,
    ReceiptReadSchema, ReceiptReadStatus, RegistryInspection, RegistrySearchPacket, RunContext,
    RunList, RunListEntry, RunListSchema, RuntimeSealPending, RunxProcess, TerminalSize,
    UnreadableRun,
};
pub use native_packets::{
    StoreAppendOperation, StoreAppended, StoreAppliedStatus, StoreBackend, StoreConflictStatus,
    StoreConflicted, StoreEvent, StoreEventRef, StoreInspectOperation, StoreInspected,
    StoreListOperation, StoreListed, StoreListedKey, StoreMatch, StoreRead, StoreReadOperation,
    StoreResult, StoreResultSchema, StoreSearchOperation, StoreSearched, StoreWriter,
};
pub use operational_policy::{
    OperationalPolicy, OperationalPolicyAction, OperationalPolicyAutomationPermissions,
    OperationalPolicyAutomationReadback, OperationalPolicyAutomationRule,
    OperationalPolicyAutomationState, OperationalPolicyDedupePolicy,
    OperationalPolicyDedupeStrategy, OperationalPolicyDuplicateBehavior, OperationalPolicyError,
    OperationalPolicyMissingBehavior, OperationalPolicyOutcomeCloseMode,
    OperationalPolicyOutcomePolicy, OperationalPolicyOwnerRoute, OperationalPolicyPublishMode,
    OperationalPolicyReadback, OperationalPolicySchema, OperationalPolicySourceReadback,
    OperationalPolicySourceRule, OperationalPolicySourceThreadPolicy,
    OperationalPolicyTargetReadback, OperationalPolicyTargetRule,
    OperationalPolicyValidationFinding, lint_operational_policy_contract,
    project_operational_policy_readback, validate_operational_policy_contract,
};
pub use orchestrator_handoff::{
    OrchestratorExecutionContext, OrchestratorHandoffBinding, OrchestratorHandoffContext,
    OrchestratorHandoffDelivery, OrchestratorHandoffIdempotency, OrchestratorReceiptExpectations,
    OrchestratorReceiverValidation,
};
pub use output::{
    Output, OutputContractParseError, OutputField, OutputValidationError, output_contract_digest,
    output_value_schema, parse_output_contract, validate_output_value,
};
pub use packet_id::{
    PACKET_ID_EXPECTED, PACKET_ID_MAX_LENGTH, PACKET_ID_PATTERN, PacketId, is_packet_id,
};
pub use paid_skill_listing::{
    PAID_SKILL_LISTING_SCHEMA, PaidSkillListing, PaidSkillMethodOffer, PaidSkillOfferTerms,
    PaidSkillOffers, PaidSkillPrice, PaidSkillPriceCase, PriceChoice, PriceError, RequiredLease,
};
pub use provider_operation::{AlreadyDone, ProviderOperationPacket};
pub use purchase::{
    BuyerKey, CurrencyCode, EmbeddedOfferVersionBinding, OFFER_VERSION_BINDING_SCHEMA,
    OfferVersionBinding, PURCHASE_READ_SCHEMA, PURCHASE_SCHEMA, PaymentIdempotencyBinding,
    PaymentReference, PortableAmountMinor, PrincipalReference, Purchase,
    PurchaseCanonicalizerVersion, PurchaseExecutionState, PurchaseLease, PurchaseOutcomeGate,
    PurchasePaymentChallenge, PurchasePaymentMethod, PurchasePaymentOptionBinding,
    PurchasePaymentState, PurchaseRead, PurchaseRefusalCode, PurchaseRefusalReason,
    QUOTE_PURCHASE_RESULT_SCHEMA, QuotePurchaseAdmission, QuotePurchaseResult,
    RUNX_BALANCE_SETTLEMENT_FAMILY, SettlementFamilies, SettlementFamily,
};
pub use receipt::{
    Lineage, MAX_RECEIPT_TREE_DEPTH, MAX_RECEIPT_TREE_RECEIPTS, RECEIPT_CANONICALIZATION,
    RECEIPT_SCHEMA, RECEIPTS_PER_SKILL_CALL, Receipt, ReceiptAct, ReceiptAuthority,
    ReceiptCanonicalization, ReceiptEnforcement, ReceiptIssuer, ReceiptIssuerType, ReceiptSchema,
    ReceiptSignature, RunOutcome, Seal, SealFailure, SealReason, SignatureAlgorithm, Subject,
    SubjectKind,
};
pub use reference::{
    ActAddress, ExternalReferenceKind, ProofKind, Reference, ReferenceType, RunxPrincipalId,
};
pub use registry_signed_manifest::{
    REGISTRY_MANIFEST_SIGNATURE_ALG, REGISTRY_MANIFEST_SIGNATURE_PREFIX,
    REGISTRY_SIGNED_MANIFEST_SCHEMA, RegistryContentDigestError, RegistryContentFile,
    RegistryManifestSignature, RegistryManifestSigner, RegistrySignedManifest, RegistrySubject,
    RegistrySubjectKind, is_registry_subject_name, registry_content_digest,
};
pub use registry_taxonomy::{
    Category as RegistryCategory, TaxonomySubject, UNCATEGORIZED, registry_category,
    validate_registry_category,
};
pub use registry_tier_statement::{
    REGISTRY_TIER_STATEMENT_SCHEMA, RegistryTierStatement, TrustTier,
};
pub use registry_version::{
    REGISTRY_VERSION_MAX_LENGTH, REGISTRY_VERSION_PATTERN, is_registry_version,
    validate_registry_version,
};
pub use resume::{
    BoundAnswer, BoundApproval, CloseOutcome, ResumeDocument, RunClose, StandingApproval,
    StandingCommands, StandingEdits, StandingOperations,
};
pub use run_read::{
    RanOn, RecomputeReason, ReuseDecision, RunCallOutcome, RunCallOutput, RunCallReuse, RunDriver,
    RunEventFact, RunEventRecord, RunHold, RunImplementation, RunRead, RunReadSchema,
    RunReadSummary, RunState, TurnHost, WaitedRequest,
};
pub use schema_artifacts::{
    SchemaArtifact, generated_schema_artifacts, public_packet_artifact, schema_artifact,
};
pub use schema_definitions::{
    DEFINITIONS_FILE, DEFINITIONS_ID, PublishedSchemas, SchemaDefinitionError, document_text,
    publish_schema_artifacts,
};
pub use schema_reconcile::{SchemaDrift, reconcile_schema_artifacts};
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
pub use source_packet::{
    OperationalProposalRedactionStatus, SOURCE_PACKET_SCHEMA, SignalAuthenticity, SignalTrustLevel,
    SourcePacket, SourcePacketSchema,
};
pub use tools::{
    RuntimeCommand, ToolCommandInputMode, ToolInput, ToolManifest, ToolManifestSchema, ToolProcess,
};
pub use verdict::{
    Assessment, AssessmentFailure, CRITERIA_SCHEMA, Criteria, CriteriaSchema, CriterionId, Decided,
    Evaluator, Pinned, VERDICT_SCHEMA, Verdict, VerdictAnswer, VerdictCriterion, VerdictDecision,
    VerdictRefused, VerdictSchema,
};
pub use work_outcome::{WorkOutcome, WorkProposal, WorkState};
pub use x402::{
    RUNX_PURCHASE_EXTENSION_KEY, RUNX_X402_PURCHASE_EXTENSION_SCHEMA,
    RunxX402PurchaseExtensionInfo, X402_PAYMENT_PAYLOAD_SCHEMA_ID, X402_PAYMENT_REQUIRED_HEADER,
    X402_PAYMENT_REQUIRED_SCHEMA_ID, X402_PAYMENT_REQUIREMENTS_SCHEMA_ID,
    X402_PAYMENT_RESPONSE_HEADER, X402_PAYMENT_SIGNATURE_HEADER, X402_PROTOCOL_VERSION,
    X402_RESOURCE_INFO_SCHEMA_ID, X402_SETTLE_RESPONSE_SCHEMA_ID, X402_UPSTREAM_COMMIT,
    X402_UPSTREAM_PACKAGE, X402_UPSTREAM_PACKAGE_VERSION, X402AcceptedRequirements, X402IconUrl,
    X402Network, X402PaymentPayload, X402PaymentRequired, X402PaymentRequirements,
    X402PositiveNumber, X402ResourceInfo, X402ServiceName, X402SettleResponse, X402Tag, X402Tags,
    X402Version2,
};

pub use skill_inspection::{
    EnvironmentReadiness, EnvironmentReadinessStatus, InspectedInput, InspectedOutput,
    InspectionReadiness, InspectionReadinessStatus, InspectionResume, MethodInspection,
    MethodInspectionEntry, MethodSchemas, SkillCatalog, SkillInspection, SkillInspectionSchema,
    SkillInspectionStatus,
};
pub use skill_output::SkillOutput;
