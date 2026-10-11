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
pub mod dev;
pub mod doctor;
pub mod execution;
pub mod execution_boundary;
pub mod execution_requirements;
pub mod external_job;
pub mod fingerprint;
pub mod fixture;
pub mod handoff;
pub mod host_protocol;
pub mod javascript_worker;
pub mod json;
pub mod ledger;
pub mod limits;
pub mod links;
pub mod list;
pub mod maturity;
pub mod native_packets;
pub mod operational_policy;
pub mod operational_proposal;
pub mod orchestrator_handoff;
pub mod output;
pub mod packet_index;
pub mod paid_invocation;
pub mod paid_invocation_fingerprint;
pub mod paid_skill_listing;
pub mod provider_operation;
pub mod receipt;
pub mod redaction;
pub mod reference;
pub mod registry_binding;
pub mod registry_taxonomy;
pub mod registry_version;
pub mod review;
pub mod run_summary;
pub mod schema;
pub mod schema_artifacts;
mod schema_reconcile;
pub mod signal;
pub mod skill_authoring;
pub mod skill_output;
pub mod source_packet;
pub mod suppression;
pub mod tools;
pub mod work_outcome;
pub mod x402;

pub use act::assignment::{
    ActAssignment, ActAssignmentActor, ActAssignmentHost, ActAssignmentHostKind,
    ActAssignmentIdempotency, ActAssignmentSchema, BuildActAssignment, IntentKeyInput,
    derive_content_hash, derive_intent_key, derive_trigger_key,
};
pub use act::call::{CALL_ACT_TARGET_MAX, CALL_ACT_TEXT_MAX, CallAct};
pub use act::result::{
    ActResultEnvelope, ActResultNeedsAgentEnvelope, ActResultNeedsAgentStatus, ActResultNull,
    ActResultSignal, ActResultTerminalEnvelope, ActResultTerminalStatus,
};
pub use act::{ActForm, Criterion, CriterionBinding, CriterionStatus, Intent};
pub use admission::{
    Admission, AdmissionCheck, AdmissionDisposition, AdmissionReason, CheckOutcome,
    OperationAmount, OperationIntent, Readiness,
};
pub use agent_context::{
    AgentContextEnvelope, AgentContextProfiles, ContextArtifactMeta, ContextArtifactProducer,
    ContextEntry, ContextEntryVersion, ContextPurpose, ContextScope, ContextSharing, ContextTrust,
    ExecutionLocation, ProfileFile, ProvenanceEntry,
};
pub use artifact::{ARTIFACT_SCHEMA, Artifact, ArtifactProducedBy, ArtifactSchema};
pub use authority::{
    AuthorityApproval, AuthorityBounds, AuthorityCapability, AuthorityCondition,
    AuthorityConditionPredicate, AuthorityDerivation, AuthorityEffectCredentialForm,
    AuthorityEffectGuard, AuthorityEffectGuardKind, AuthorityEffectLimit, AuthorityEvidence,
    AuthorityEvidenceKind, AuthorityResourceFamily, AuthoritySubsetComparison,
    AuthoritySubsetProof, AuthoritySubsetRelation, AuthoritySubsetResult, AuthorityTerm,
    AuthorityUseLimit, AuthorityVerb, authority_term_legitimacy, legitimacy_authority_term,
};
pub use canonical_json::{
    CanonicalJsonError, STABLE_JSON_CANONICALIZATION, canonical_digest, canonical_stable_json,
    to_canonical_bytes, write_canonical_json_fragment,
};
pub use connector::{
    ConnectorId, ConnectorManifest, ConnectorOperation, Digest, InstanceId, OperationAccess,
    OperationName, OperationVerb, Placement,
};
pub use connector_instance::{ConnectorInstance, InstanceAuthority};
pub use conversation_source::{ConversationObservation, ConversationPage};
pub use credential_delivery::{
    CredentialDeliveryMode, CredentialDeliveryObservation, CredentialDeliveryObservationSchema,
    CredentialDeliveryObservationStatus, CredentialDeliveryPurpose, CredentialMaterialRole,
};
pub use decision::{ClosureDisposition, Decision, DecisionChoice};
pub use decision_adapter::{DecisionObservation, DecisionOrigin};
pub use dev::{
    DevFixtureAssertion, DevFixtureAssertionKind, DevFixtureResult, DevFixtureStatus, DevReport,
    DevReportSchema, DevReportStatus,
};
pub use doctor::{
    DoctorDiagnostic, DoctorDiagnosticSeverity, DoctorLocation, DoctorRepair,
    DoctorRepairConfidence, DoctorRepairKind, DoctorRepairRisk, DoctorReport, DoctorReportSchema,
    DoctorStatus, DoctorSummary,
};
pub use execution::{
    ArtifactContract, IdempotencyPolicy, InputDefinition, input_contract_schema,
    input_contract_schema_with_examples,
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
    ExternalJobStage, ExternalJobStageRequest, ExternalJobStageResult, ExternalJobStatus,
};
pub use fingerprint::{Fingerprint, FingerprintAlgorithm, hex_lower, sha256_hex, sha256_prefixed};
pub use fixture::{Fixture, FixtureLane};
pub use handoff::{
    HandoffDisposition, HandoffSignal, HandoffSignalActor, HandoffSignalSchema,
    HandoffSignalSource, HandoffSignalSourceRef, HandoffState, HandoffStateSchema, HandoffStatus,
    SuppressionReason,
};
pub use host_protocol::{
    AgentActInvocation, AllowedOperation, ApprovalDecision, ApprovalGate, ExecutionEvent,
    HostNeedsAgentState, HostRunApproval, HostRunApprovalDecision, HostRunKind, HostRunLineage,
    HostRunLineageKind, HostRunResult, HostRunState, HostRunVerification,
    HostRunVerificationStatus, HostTerminalState, Question, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor, TurnAllowance, TurnCallOutcome, TurnCallResult, TurnStatus,
    TurnTarget, TurnTool,
};
pub use json::{
    JsonNumber, JsonObject, JsonValue, MAX_PORTABLE_INTEGER, json_bool_field, json_object,
    json_object_field, json_string_field,
};
pub use ledger::{
    LedgerCanonicalization, LedgerChain, LedgerChainVersion, LedgerEntry, LedgerEntryMeta,
    LedgerEntrySchemaVersion, LedgerHashAlgorithm, LedgerPayload, LedgerPayloadVersion,
    LedgerProducer, LedgerSha256Hex,
};
pub use limits::{
    EXECUTION_LIMITS_METADATA, ExecutionLimit, ExecutionLimitHit, ExecutionLimitUnit,
    ExecutionLimits,
};
pub use links::{DuplicateCandidate, Links};
pub use list::{
    RunxListEmit, RunxListItem, RunxListItemKind, RunxListReport, RunxListRequestedKind,
    RunxListSchema, RunxListSource, RunxListStatus,
};
pub use native_packets::{
    ApprovalDecisionActor, ApprovalDecisionPacket, ApprovalDecisionStatus, DataOperationResult,
    DataOperationStatus, DataStopCondition, ExternalReceiptVerification, GitBlobDigest,
    LocalArtifact, LocalArtifactPage,
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
pub use packet_index::{PacketIndex, PacketIndexEntry, PacketIndexSchema};
pub use paid_invocation::{
    CANCEL_PAID_INVOCATION, CANCEL_PAID_INVOCATION_REQUEST_SCHEMA,
    CANCEL_PAID_INVOCATION_RESULT_SCHEMA, CancelPaidInvocationRequest, CancelPaidInvocationResult,
    CurrencyCode, EXECUTE_PAID_INVOCATION, EXECUTE_PAID_INVOCATION_REQUEST_SCHEMA,
    EXECUTE_PAID_INVOCATION_RESULT_SCHEMA, EmbeddedOfferRevisionRef, ExecutePaidInvocationRequest,
    ExecutePaidInvocationResult, GET_PAID_INVOCATION, GET_PAID_INVOCATION_REQUEST_SCHEMA,
    GET_PAID_INVOCATION_RESULT_SCHEMA, GetPaidInvocationRequest, GetPaidInvocationResult,
    MediatedReceiptClass, MediationEndpointUrl, MediationListingRef, OFFER_REVISION_REF_SCHEMA,
    OfferRevisionRef, PAID_INVOCATION_SCHEMA, PARENT_INVOCATION_BINDING_SCHEMA, PaidInvocation,
    PaidInvocationAdmission, PaidInvocationAttribution, PaidInvocationAttributionCampaign,
    PaidInvocationAttributionConfidence, PaidInvocationAttributionSource,
    PaidInvocationCanonicalizerVersion, PaidInvocationExecutionState, PaidInvocationMediation,
    PaidInvocationOutcomeGate, PaidInvocationPaymentChallenge, PaidInvocationPaymentState,
    PaidInvocationRefusalCode, PaidInvocationRefusalReason, ParentInvocationBinding,
    PaymentIdempotencyBinding, PaymentReference, PortableAmountMinor,
    PreparedInvocationPriceBinding, PrincipalReference, QUOTE_PAID_INVOCATION,
    QUOTE_PAID_INVOCATION_REQUEST_SCHEMA, QUOTE_PAID_INVOCATION_RESULT_SCHEMA,
    QuotePaidInvocationAdmission, QuotePaidInvocationAttribution, QuotePaidInvocationRequest,
    QuotePaidInvocationResult, SettlementFamilies, SettlementFamily, Sha256Digest,
};
pub use paid_invocation_fingerprint::{
    PAID_INVOCATION_REQUEST_FINGERPRINT_SCHEMA, fingerprint_cancel_paid_invocation_request,
    fingerprint_execute_paid_invocation_request, fingerprint_quote_paid_invocation_request,
    quote_paid_invocation_binding,
};
pub use paid_skill_listing::{
    PAID_SKILL_LISTING_SCHEMA, PaidSkillExecutorBinding, PaidSkillFixedOfferTerms,
    PaidSkillFixedRunnerOffer, PaidSkillListing, PaidSkillMediationTerms, PaidSkillOfferTerms,
    PaidSkillOffers, PaidSkillPreparedMediation, PaidSkillPreparedMediationTerms,
    PaidSkillPreparedOfferTerms, PaidSkillPreparedRunnerOffer, PaidSkillRunnerOffer,
    PaidSkillVendorAmountRange,
};
pub use provider_operation::ProviderOperationPacket;
pub use receipt::{
    Lineage, RECEIPT_CANONICALIZATION, RECEIPT_SCHEMA, Receipt, ReceiptAct, ReceiptAuthority,
    ReceiptCanonicalization, ReceiptEnforcement, ReceiptIssuer, ReceiptIssuerType, ReceiptSchema,
    ReceiptSignature, Seal, SignatureAlgorithm, Subject, receipt_subject_kind,
};
pub use redaction::{HashAlgorithm, HashCommitment, REDACTION_SCHEMA, Redaction, RedactionSchema};
pub use reference::{
    ActRef, ExternalReferenceKind, ProofKind, Reference, ReferenceLink, ReferenceType,
    RunxPrincipalId,
};
pub use registry_binding::{
    RegistryBinding, RegistryBindingHarness, RegistryBindingRegistry, RegistryBindingSchema,
    RegistryBindingSkill, RegistryBindingState, RegistryBindingUpstream, RegistryHarnessStatus,
    RegistryTrustTier,
};
pub use registry_taxonomy::{
    Category as RegistryCategory, TaxonomySubject, UNCATEGORIZED, registry_categories,
    registry_category, validate_registry_category,
};
pub use copyable_command::{CommandShell, CopyableCommand, render_posix_command};
pub use registry_version::{
    REGISTRY_VERSION_MAX_LENGTH, REGISTRY_VERSION_PATTERN, is_registry_version,
    validate_registry_version,
};
pub use review::{ReviewReceiptImprovementProposal, ReviewReceiptOutput, ReviewReceiptVerdict};
pub use run_summary::{RunSummary, RunSummarySchema, RunSummaryStatus};
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
    SkillArchitectureDecision, SkillArchitectureDecisionSchema, SkillArchitectureDisposition,
    SkillArchitecturePlan, SkillArchitecturePlanSchema, SkillBehaviorDecision, SkillChainPlan,
    SkillChainUseContract, SkillChangeBundle, SkillChangeBundleSchema, SkillChangeDecision,
    SkillChangeDraft, SkillChangeDraftSchema, SkillDirectUseContract, SkillEffectClass,
    SkillEffectRequirement, SkillExecutionLane, SkillExpectedOutput, SkillFileWrite,
    SkillIdentityAction, SkillIdentityDecision, SkillKnowledgeContract, SkillNativeReuseEvidence,
    SkillPackageDelta, SkillPackageMetrics, SkillPackageVisibility, SkillProofKind,
    SkillProofRequirement, SkillResourceBudget, SkillValidationCheck, SkillValidationCheckStatus,
    SkillValidationResult, SkillValidationResultSchema,
};
pub use source_packet::{SOURCE_PACKET_SCHEMA, SourcePacket, SourcePacketSchema};
pub use suppression::{SuppressionRecord, SuppressionRecordSchema, SuppressionScope};
pub use tools::{
    RuntimeCommand, ToolCommandInputMode, ToolIdempotencyPolicy, ToolInput, ToolManifest,
    ToolManifestSchema, ToolSource,
};
pub use work_outcome::{WorkDisposition, WorkOutcome, WorkProposal};
pub use x402::{
    RUNX_INVOCATION_EXTENSION_KEY, RUNX_X402_INVOCATION_EXTENSION_SCHEMA,
    RunxX402InvocationExtension, RunxX402InvocationExtensionInfo, X402_PAYMENT_PAYLOAD_SCHEMA_ID,
    X402_PAYMENT_REQUIRED_HEADER, X402_PAYMENT_REQUIRED_SCHEMA_ID,
    X402_PAYMENT_REQUIREMENTS_SCHEMA_ID, X402_PAYMENT_RESPONSE_HEADER,
    X402_PAYMENT_SIGNATURE_HEADER, X402_PROTOCOL_VERSION, X402_RESOURCE_INFO_SCHEMA_ID,
    X402_SETTLE_RESPONSE_SCHEMA_ID, X402_UPSTREAM_COMMIT, X402_UPSTREAM_PACKAGE,
    X402_UPSTREAM_PACKAGE_VERSION, X402AcceptedRequirements, X402IconUrl, X402Network,
    X402PaymentPayload, X402PaymentRequired, X402PaymentRequirements, X402PositiveNumber,
    X402ResourceInfo, X402ServiceName, X402SettleResponse, X402Tag, X402Tags, X402Version2,
    parse_runx_invocation_extension, runx_invocation_extension_value,
};

pub use skill_output::{ProjectArtifactRef, SkillOutput};
