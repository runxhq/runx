export const contractsPackage = "@runxhq/contracts";

export {
  RUNX_SCOPE_GRANT_POLICY,
  missingGrantedScopes,
  scopeGrantAllows,
  type ScopeGrantPolicy,
} from "./scope-policy.js";

export { definePacket, type PacketDefinition } from "./packet-definition.js";

export {
  TemplateError,
  connectorBindingSchema,
  validateConnectorBinding,
  hostedHttp,
  isCheckingRead,
  operationAccess,
  operationInputError,
  referenceKind,
  renderDeclaredRequest,
  renderTemplate,
  type ConnectorManifest,
  type ConnectorBinding,
  type ConnectorOperation,
  type CredentialDeclaration,
  type CredentialField,
  type DecodeStep,
  type DeclaredRequestCall,
  type HttpImplementation,
  type HttpAuth,
  type HttpMethod,
  type HttpReadback,
  type HttpRequest,
  type JsonObject,
  type JsonValue,
  type RunxMaintainedConnector,
  type OperationVerb,
  type ReferenceDeclaration,
  type ReferenceKind,
  type RenderedRequest,
  type Template,
  type TemplateInputs,
} from "./connector.js";

export { runxMaintainedConnectors } from "./generated/runx-maintained-connectors.js";
export { posixCommand, renderPosixCommand, type CopyableCommand } from "./copyable-command.js";
export { isRegistryVersion, REGISTRY_VERSION_PATTERN } from "./registry-version.js";
export {
  REGISTRY_MANIFEST_SIGNATURE_ALG,
  REGISTRY_MANIFEST_SIGNATURE_PREFIX,
  REGISTRY_SIGNED_MANIFEST_SCHEMA,
  isRegistrySubjectRef,
  registryContentDigest,
  registrySignedManifestPayload,
  registrySignedManifestSigns,
  validateRegistrySignedManifest,
  type RegistryContentFile,
  type RegistrySignedManifest,
  type RegistrySubject,
  type RegistrySubjectKind,
  type UnsignedRegistryManifest,
} from "./registry-signed-manifest.js";
export {
  registryTaxonomy,
  type RegistryTaxonomy,
  type RegistryTaxonomyCategory,
  type RegistryTaxonomyMark,
  type RegistryTaxonomySubject,
} from "./registry-taxonomy.js";


export {
  RUNX_STABLE_JSON_V1,
  canonicalJsonStringify,
  sha256Hex,
  sha256Prefixed,
} from "./canonical-json.js";

export {
  fingerprintQuotePaidInvocationRequest,
  fingerprintExecutePaidInvocationRequest,
  fingerprintCancelPaidInvocationRequest,
} from "./paid-invocation-fingerprint.js";

export {
  X402_PAYMENT_REQUIRED_HEADER,
  X402_PAYMENT_SIGNATURE_HEADER,
  X402_PAYMENT_RESPONSE_HEADER,
  RUNX_X402_INVOCATION_EXTENSION_KEY,
  RUNX_X402_INVOCATION_SCHEMA_REFERENCE,
  X402_BAZAAR_EXTENSION_KEY,
  X402_JSON_MEDIA_TYPE,
  MAX_X402_HEADER_BYTES,
  MAX_X402_DECODED_BYTES,
  X402PresentationError,
  assembleExternalX402PaymentRequired,
  assembleX402PaymentRequired,
  parseRunxX402InvocationDeclaration,
  declareExternalX402JsonPostDiscovery,
  x402ExternalDiscoveryHttpProjection,
  x402DiscoveryHttpProjection,
  bindX402PaymentRequiredChallenge,
  x402PaymentRequiredFromChallenge,
  validateX402PaymentRetry,
  selectedX402Requirement,
  encodeX402PaymentRequiredHeader,
  decodeX402PaymentRequiredHeader,
  encodeX402PaymentSignatureHeader,
  decodeX402PaymentSignatureHeader,
  encodeX402PaymentResponseHeader,
  decodeX402PaymentResponseHeader,
  type X402PresentationErrorCode,
  type ValidatedX402Retry,
  type X402DiscoveryDescriptor,
  type X402ExternalDiscoveryDescriptor,
  type X402DiscoveryHttpProjection,
} from "./x402-presentation.js";

export {
  RUNX_SCHEMA_BASE_URL,
  RUNX_CONTRACT_IDS,
  RUNX_LOGICAL_SCHEMAS,
  contractSchemaMatches,
  validateContractSchemaForDiagnostics,
  Type,
  Type as t,
  type JsonSchema,
  type Static,
} from "./internal.js";

export {
  skillArchitectureDecisionV1Schema,
  skillArchitecturePlanV1Schema,
  skillChangeDraftV1Schema,
  skillChangeBundleV1Schema,
  skillValidationResultV1Schema,
  skillApplyResultV1Schema,
  validateSkillArchitectureDecisionContract,
  validateSkillArchitecturePlanContract,
  validateSkillChangeDraftContract,
  validateSkillChangeBundleContract,
  validateSkillValidationResultContract,
  validateSkillApplyResultContract,
  type SkillArchitectureDecisionContract,
  type SkillArchitecturePlanContract,
  type SkillChangeDraftContract,
  type SkillChangeBundleContract,
  type SkillValidationResultContract,
  type SkillApplyResultContract,
} from "./schemas/skill-authoring.js";


export {
  credentialDeliveryModeSchema,
  credentialDeliveryPurposeSchema,
  credentialMaterialRoleSchema,
  credentialDeliveryObservationStatusSchema,
  credentialDeliveryObservationV1Schema,
  validateCredentialDeliveryObservationContract,
  type CredentialDeliveryObservationContract,
} from "./schemas/credential-delivery.js";

export {
  dataOperationResultStatuses,
  dataOperationResultStatusSchema,
  dataOperationStopConditionSchema,
  dataOperationResultV1Schema,
  validateDataOperationResultContract,
  type DataOperationStopConditionContract,
  type DataOperationResultContract,
} from "./schemas/data-operation.js";

export {
  outputScalarSchema,
  outputObjectEntrySchema,
  outputEntrySchema,
  outputSchema,
  validateOutputContract,
  type OutputScalarContract,
  type OutputObjectEntryContract,
  type OutputEntryContract,
  type OutputContract,
} from "./schemas/output.js";

export {
  artifactProducerSchema,
  artifactMetaSchema,
  artifactEnvelopeSchema,
  type ArtifactProducerContract,
  type ArtifactMetaContract,
  type ArtifactEnvelopeContract,
} from "./schemas/artifact.js";

export {
  agentContextProvenanceSchema,
  contextDocumentSchema,
  contextSchema,
  executionLocationSchema,
  environmentRequirementsSchema,
  executionRequirementsSchema,
  agentExecutionRequirementsSchema,
  agentContextEnvelopeSchema,
  validateAgentContextEnvelopeContract,
  type AgentContextProvenanceContract,
  type ContextDocumentContract,
  type ContextContract,
  type ExecutionLocationContract,
  type AgentExecutionRequirementsContract,
  type AgentContextEnvelopeContract,
} from "./schemas/context.js";

export {
  agentActInvocationSchema,
  questionSchema,
  approvalGateSchema,
  validateAgentActInvocationContract,
  validateQuestionContract,
  validateApprovalGateContract,
  type AgentActInvocationContract,
  type QuestionContract,
  type ApprovalGateContract,
  type TurnAllowanceContract,
} from "./schemas/agent-act.js";

export {
  turnCallSchema,
  turnStatusSchema,
  validateTurnCallContract,
  validateTurnStatusContract,
  type TurnTargetContract,
  type TurnCallOutcomeContract,
  type TurnCallContract,
  type TurnToolContract,
  type TurnStatusContract,
} from "./schemas/turn.js";

export {
  processSchema,
  processOutputSchema,
  processListSchema,
  validateProcessContract,
  validateProcessOutputContract,
  validateProcessListContract,
  type TerminalSizeContract,
  type ProcessStateContract,
  type ProcessExitReasonContract,
  type ProcessExitContract,
  type RunxProcessContract,
  type ProcessStreamContract,
  type ProcessOutputContract,
  type ProcessListContract,
} from "./schemas/process.js";

export {
  runReadSchema,
  runEventSchema,
  validateRunReadContract,
  validateRunEventContract,
  type RunStateContract,
  type RunDriverContract,
  type WaitedRequestContract,
  type RunHoldContract,
  type RunCallOutcomeContract,
  type RunCallOutputContract,
  type RunEventFactContract,
  type RunEventContract,
  type RunReadContract,
} from "./schemas/run-read.js";

export {
  resumeDocumentSchema,
  validateResumeDocumentContract,
  type ResumeDocumentContract,
  type StandingApprovalContract,
} from "./schemas/resume.js";

export {
  resolutionRequestSchema,
  resolutionResponseSchema,
  actResultEnvelopeSchema,
  validateResolutionRequestContract,
  validateResolutionResponseContract,
  validateActResultEnvelopeContract,
  type InputResolutionRequestContract,
  type ApprovalResolutionRequestContract,
  type AgentActResolutionRequestContract,
  type RetryResolutionRequestContract,
  type ResolutionRequestContract,
  type ResolutionResponseContract,
  type ActResultTerminalStatusContract,
  type ActResultSignalContract,
  type ActResultTerminalEnvelopeContract,
  type ActResultNeedsAgentEnvelopeContract,
  type ActResultEnvelopeContract,
} from "./schemas/resolution.js";

export {
  registryBindingSchema,
  reviewReceiptOutputSchema,
  validateRegistryBindingContract,
  validateReviewReceiptOutputContract,
  type RegistryBindingContract,
  type ReviewReceiptOutputContract,
} from "./schemas/registry.js";

export {
  doctorRepairSchema,
  doctorLocationSchema,
  doctorDiagnosticSchema,
  doctorSummarySchema,
  doctorV1Schema,
  validateDoctorReportContract,
  type DoctorRepairContract,
  type DoctorLocationContract,
  type DoctorDiagnosticContract,
  type DoctorSummaryContract,
  type DoctorReportContract,
} from "./schemas/doctor.js";

export {
  runxListRequestedKindSchema,
  runxListItemKindSchema,
  runxListSourceSchema,
  runxListItemSchema,
  listV1Schema,
  validateRunxListReportContract,
  type RunxListRequestedKindContract,
  type RunxListItemKindContract,
  type RunxListSourceContract,
  type RunxListVisibilityContract,
  type RunxListEmitContract,
  type RunxListItemContract,
  type RunxListReportContract,
} from "./schemas/list.js";

export {
  externalJobContinuationV1Schema,
  externalJobScheduleV1Schema,
  externalJobScheduleIntentV1Schema,
  externalJobStageRequestV1Schema,
  externalJobStageResultV1Schema,
  validateExternalJobContinuationContract,
  validateExternalJobScheduleContract,
  validateExternalJobScheduleIntentContract,
  externalJobScheduleIntentFromOutput,
  validateExternalJobStageRequestContract,
  validateExternalJobStageResultContract,
  type ExternalJobCheckpointContract,
  type ExternalJobContinuationContract,
  type ExternalJobFailureContract,
  type ExternalJobScheduleContract,
  type ExternalJobScheduleIntentContract,
  type ExternalJobStageContract,
  type ExternalJobStageRequestContract,
  type ExternalJobStageResultContract,
  type ExternalJobStatusContract,
} from "./schemas/external-job.js";

export {
  receiptV1Schema,
  type ReceiptContract,
  validateReceiptContract,
  RECEIPT_CANONICALIZATION,
  RECEIPT_TREE_LIMITS,
} from "./schemas/receipt.js";

export {
  paidSkillListingV1Schema,
  paidInvocationV1Schema,
  offerRevisionRefV1Schema,
  parentInvocationBindingV1Schema,
  quotePaidInvocationRequestV1Schema,
  quotePaidInvocationResultV1Schema,
  executePaidInvocationRequestV1Schema,
  executePaidInvocationResultV1Schema,
  getPaidInvocationRequestV1Schema,
  getPaidInvocationResultV1Schema,
  cancelPaidInvocationRequestV1Schema,
  cancelPaidInvocationResultV1Schema,
  paymentIdempotencyBindingSchema,
  paidInvocationPaymentChallengeSchema,
  validateRunxPrincipalId,
  principalReferenceFromRunxPrincipalId,
  validatePaidSkillListingContract,
  validatePaidInvocationContract,
  validateOfferRevisionRefContract,
  validateParentInvocationBindingContract,
  validatePaymentIdempotencyBindingContract,
  validatePaidInvocationPaymentChallengeContract,
  validateQuotePaidInvocationRequestContract,
  validateQuotePaidInvocationResultContract,
  validateExecutePaidInvocationRequestContract,
  validateExecutePaidInvocationResultContract,
  validateGetPaidInvocationRequestContract,
  validateGetPaidInvocationResultContract,
  validateCancelPaidInvocationRequestContract,
  validateCancelPaidInvocationResultContract,
  type RunxPrincipalId,
  type Sha256DigestContract,
  type CurrencyCodeContract,
  type SettlementFamilyContract,
  type PaidInvocationCanonicalizerVersionContract,
  type PaidInvocationPaymentStateContract,
  type PaidInvocationExecutionStateContract,
  type PaidInvocationOutcomeGateContract,
  type PaidInvocationRefusalCodeContract,
  type PrincipalReferenceContract,
  type PaymentReferenceContract,
  type PaymentIdempotencyBindingContract,
  type OfferRevisionRefContract,
  type MediatedReceiptClassContract,
  type PaidInvocationMediationContract,
  type PreparedInvocationPriceBindingContract,
  type PaidSkillExecutorBindingContract,
  type PaidSkillFixedOfferTermsContract,
  type PaidInvocationPresentationContract,
  type PaidSkillFixedRunnerOfferContract,
  type PaidSkillMediationTermsContract,
  type PaidSkillOfferTermsContract,
  type PaidSkillPreparedMediationContract,
  type PaidSkillPreparedMediationTermsContract,
  type PaidSkillPreparedOfferTermsContract,
  type PaidSkillPreparedRunnerOfferContract,
  type PaidSkillRunnerOfferContract,
  type PaidSkillVendorAmountRangeContract,
  type PaidSkillListingContract,
  type ParentInvocationBindingContract,
  type PaidInvocationAttributionContract,
  type PaidInvocationContract,
  type PaidInvocationPaymentChallengeContract,
  type QuotePaidInvocationAttributionContract,
  type QuotePaidInvocationRequestContract,
  type PaidInvocationRefusalContract,
  type QuotePaidInvocationResultContract,
  type ExecutePaidInvocationRequestContract,
  type PaidInvocationAdmissionResultContract,
  type ExecutePaidInvocationResultContract,
  type GetPaidInvocationRequestContract,
  type GetPaidInvocationResultContract,
  type CancelPaidInvocationRequestContract,
  type CancelPaidInvocationResultContract,
} from "./schemas/paid-invocation.js";

export {
  X402_PROTOCOL_VERSION,
  X402_UPSTREAM_PACKAGE,
  X402_UPSTREAM_PACKAGE_VERSION,
  X402_UPSTREAM_COMMIT,
  X402_SCHEMA_IDS,
  x402ResourceInfoV2Schema,
  x402PaymentRequirementsV2Schema,
  x402PaymentRequiredV2Schema,
  x402PaymentPayloadV2Schema,
  x402SettleResponseV2Schema,
  runxX402InvocationExtensionInfoV1Schema,
  validateX402ResourceInfoContract,
  validateX402PaymentRequirementsContract,
  validateX402PaymentRequiredContract,
  validateX402PaymentPayloadContract,
  validateX402SettleResponseContract,
  validateRunxX402InvocationExtensionInfoContract,
  type X402ResourceInfoContract,
  type X402PaymentRequirementsContract,
  type X402PaymentRequiredContract,
  type X402PaymentPayloadContract,
  type X402SettleResponseContract,
  type RunxX402DiscoveryExtensionInfoContract,
  type RunxX402InvocationExtensionInfoContract,
  type RunxX402ExtensionInfoContract,
  type RunxX402InvocationExtensionContract,
} from "./schemas/x402.js";

export {
  operationalPolicySchema,
  operationalPolicySchemaVersion,
  operationalPolicyActions,
  operationalPolicyRunnerStates,
  operationalPolicyDedupeStrategies,
  operationalPolicyOutcomeCloseModes,
  validateOperationalPolicyContract,
  admitOperationalPolicyRequest,
  lintOperationalPolicyContract,
  validateOperationalPolicySemantics,
  projectOperationalPolicyReadback,
  type OperationalPolicyAdmission,
  type OperationalPolicyAdmissionRequest,
  type OperationalPolicyValidationFinding,
  type OperationalPolicyReadback,
  type OperationalPolicySourceProviderContract,
  type OperationalPolicyActionContract,
  type OperationalPolicyRunnerKindContract,
  type OperationalPolicyRunnerStateContract,
  type OperationalPolicyContract,
} from "./schemas/operational-policy.js";

export {
  operationalProposalSchema,
  operationalProposalSchemaVersion,
  validateOperationalProposalContract,
  type OperationalProposalAuthorityContract,
  type OperationalProposalContract,
  type OperationalProposalHumanGateContract,
  type OperationalProposalIdempotencyContract,
  type OperationalProposalOutcomeContract,
  type OperationalProposalRecommendedActionContract,
  type OperationalProposalReferenceContract,
  type OperationalProposalReferenceLinkContract,
  type OperationalProposalRedactionStatusContract,
  type OperationalProposalEscalationExtensionContract,
  type OperationalProposalExtensionsContract,
} from "./schemas/operational-proposal.js";

export {
  validateArtifactEnvelopeContract,
} from "./schemas/artifact.js";

export {
  ledgerRecordSchemaVersion,
  ledgerChainSchemaVersion,
  ledgerHashAlgorithm,
  ledgerCanonicalization,
  ledgerChainSchema,
  ledgerRecordSchema,
  validateLedgerRecordContract,
  type LedgerChainContract,
  type LedgerRecordContract,
} from "./schemas/ledger.js";

export {
  hostedReceiptManifestSchema,
  hostedReceiptIndexEntrySchema,
  hostedArtifactIndexEntrySchema,
  validateHostedReceiptManifestContract,
  type HostedReceiptManifestContract,
  type HostedReceiptIndexEntryContract,
  type HostedArtifactIndexEntryContract,
} from "./schemas/hosted-receipt-manifest.js";

export {
  fixtureV1Schema,
  type FixtureContract,
} from "./schemas/fixture.js";

export {
  toolManifestV1Schema,
  type ToolManifestContract,
} from "./schemas/tool-manifest.js";

export {
  packetIndexV1Schema,
  type PacketIndexEntryContract,
  type PacketIndexContract,
} from "./schemas/packet-index.js";

export {
  actAssignmentActorSchema,
  actAssignmentHostSchema,
  actAssignmentIdempotencySchema,
  actAssignmentV1Schema,
  validateActAssignmentContract,
  type ActAssignmentActorContract,
  type ActAssignmentHostContract,
  type ActAssignmentIdempotencyContract,
  type ActAssignmentContract,
} from "./schemas/act-assignment.js";

export {
  executionBoundaryObservationSchema,
  type ExecutionBoundaryObservationContract,
} from "./schemas/execution-boundary.js";

export {
  referenceTypes,
  signalTypes,
  signalTrustLevels,
  closureDispositions,
  decisionChoices,
  actForms,
  criterionStatuses,
  criteria,
  authorityResourceFamilies,
  authorityVerbs,
  proofKinds,
  redactionCommitmentAlgorithms,
  referenceTypeSchema,
  signalTypeSchema,
  signalTrustLevelSchema,
  closureDispositionSchema,
  decisionChoiceSchema,
  actFormSchema,
  criterionStatusSchema,
  criterionSchema,
  authorityResourceFamilySchema,
  authorityVerbSchema,
  proofKindSchema,
  redactionCommitmentAlgorithmSchema,
  referenceSchema,
  referenceLinkSchema,
  nullableReferenceSchema,
  actReferenceSchema,
  hashCommitmentSchema,
  redactionSchema,
  duplicateCandidateSchema,
  linksSchema,
  signalAuthenticitySchema,
  signalSchema,
  authorityBoundsSchema,
  authorityApprovalSchema,
  authorityTermSchema,
  authoritySubsetComparisonSchema,
  authoritySubsetProofSchema,
  intentSchema,
  criterionBindingSchema,
  decisionSchema,
  artifactSchema,
  receiptIssuerSchema,
  receiptSignatureSchema,
  validateReferenceContract,
  validateSignalContract,
  validateAuthoritySubsetProofContract,
  validateDecisionContract,
  validateSpineArtifactContract,
  validateRedactionContract,
  type ReferenceTypeContract,
  type SignalTypeContract,
  type SignalTrustLevelContract,
  type ClosureDispositionContract,
  type DecisionChoiceContract,
  type ActFormContract,
  type CriterionStatusContract,
  type CriterionContract,
  type AuthorityResourceFamilyContract,
  type AuthorityVerbContract,
  type ProofKindContract,
  type ReferenceContract,
  type ReferenceLinkContract,
  type ActReferenceContract,
  type HashCommitmentContract,
  type RedactionContract,
  type LinksContract,
  type SignalAuthenticityContract,
  type SignalContract,
  type AuthorityBoundsContract,
  type AuthorityApprovalContract,
  type AuthorityTermContract,
  type AuthoritySubsetProofContract,
  type IntentContract,
  type CriterionBindingContract,
  type DecisionContract,
  type ArtifactContract,
  type ReceiptIssuerContract,
  type ReceiptSignatureContract,
} from "./schemas/spine.js";

export {
  handoffSignalV1Schema,
  handoffStateV1Schema,
  suppressionRecordV1Schema,
  validateHandoffSignalContract,
  validateHandoffStateContract,
  validateSuppressionRecordContract,
  type HandoffSignalContract,
  type HandoffStateContract,
  type SuppressionRecordContract,
} from "./schemas/handoff.js";

import { agentContextEnvelopeSchema } from "./schemas/context.js";
import { agentActInvocationSchema, approvalGateSchema, questionSchema } from "./schemas/agent-act.js";
import { credentialDeliveryObservationV1Schema } from "./schemas/credential-delivery.js";
import { outputSchema } from "./schemas/output.js";
import { actResultEnvelopeSchema, resolutionRequestSchema, resolutionResponseSchema } from "./schemas/resolution.js";
import { doctorV1Schema } from "./schemas/doctor.js";
import { listV1Schema } from "./schemas/list.js";
import { receiptV1Schema } from "./schemas/receipt.js";
import { fixtureV1Schema } from "./schemas/fixture.js";
import { toolManifestV1Schema } from "./schemas/tool-manifest.js";
import { packetIndexV1Schema } from "./schemas/packet-index.js";
import { actAssignmentV1Schema } from "./schemas/act-assignment.js";
import {
  artifactSchema,
  authoritySubsetProofSchema,
  decisionSchema,
  redactionSchema,
  referenceSchema,
  signalSchema,
} from "./schemas/spine.js";
import { ledgerRecordSchema } from "./schemas/ledger.js";
import { handoffSignalV1Schema, handoffStateV1Schema, suppressionRecordV1Schema } from "./schemas/handoff.js";
import { operationalPolicySchema } from "./schemas/operational-policy.js";
import { operationalProposalSchema } from "./schemas/operational-proposal.js";
import {
  externalJobContinuationV1Schema,
  externalJobScheduleV1Schema,
  externalJobScheduleIntentV1Schema,
  externalJobStageRequestV1Schema,
  externalJobStageResultV1Schema,
} from "./schemas/external-job.js";
import {
  paidSkillListingV1Schema,
  paidInvocationV1Schema,
  offerRevisionRefV1Schema,
  parentInvocationBindingV1Schema,
  quotePaidInvocationRequestV1Schema,
  quotePaidInvocationResultV1Schema,
  executePaidInvocationRequestV1Schema,
  executePaidInvocationResultV1Schema,
  getPaidInvocationRequestV1Schema,
  getPaidInvocationResultV1Schema,
  cancelPaidInvocationRequestV1Schema,
  cancelPaidInvocationResultV1Schema,
} from "./schemas/paid-invocation.js";
import { dataOperationResultV1Schema } from "./schemas/data-operation.js";
import { runxSchemaArtifacts } from "./schema-artifacts.js";

export const runxContractSchemas = {
  output: runxSchemaArtifacts["output.schema.json"],
  agentContextEnvelope: runxSchemaArtifacts["agent-context-envelope.schema.json"],
  agentActInvocation: runxSchemaArtifacts["agent-act-invocation.schema.json"],
  turnCall: runxSchemaArtifacts["turn-call.schema.json"],
  turnStatus: runxSchemaArtifacts["turn-status.schema.json"],
  runRead: runxSchemaArtifacts["run-read.schema.json"],
  process: runxSchemaArtifacts["process.schema.json"],
  processOutput: runxSchemaArtifacts["process-output.schema.json"],
  processList: runxSchemaArtifacts["process-list.schema.json"],
  runEvent: runxSchemaArtifacts["run-event.schema.json"],
  resume: runxSchemaArtifacts["resume.schema.json"],
  question: runxSchemaArtifacts["question.schema.json"],
  approvalGate: runxSchemaArtifacts["approval-gate.schema.json"],
  resolutionRequest: runxSchemaArtifacts["resolution-request.schema.json"],
  resolutionResponse: runxSchemaArtifacts["resolution-response.schema.json"],
  actResultEnvelope: runxSchemaArtifacts["act-result.schema.json"],
  credentialDeliveryObservation: runxSchemaArtifacts["credential-delivery-observation.schema.json"],
  dataOperationResult: dataOperationResultV1Schema,
  doctor: runxSchemaArtifacts["doctor.schema.json"],
  list: runxSchemaArtifacts["list.schema.json"],
  skillArchitectureDecision: runxSchemaArtifacts["skill-architecture-decision.schema.json"],
  skillArchitecturePlan: runxSchemaArtifacts["skill-architecture-plan.schema.json"],
  skillChangeDraft: runxSchemaArtifacts["skill-change-draft.schema.json"],
  skillChangeBundle: runxSchemaArtifacts["skill-change-bundle.schema.json"],
  skillValidationResult: runxSchemaArtifacts["skill-validation-result.schema.json"],
  skillApplyResult: runxSchemaArtifacts["skill-apply-result.schema.json"],
  receipt: runxSchemaArtifacts["receipt.schema.json"],
  fixture: runxSchemaArtifacts["fixture.schema.json"],
  toolManifest: runxSchemaArtifacts["tool-manifest.schema.json"],
  packetIndex: runxSchemaArtifacts["packet-index.schema.json"],
  actAssignment: runxSchemaArtifacts["act-assignment.schema.json"],
  reference: runxSchemaArtifacts["reference.schema.json"],
  referenceLink: runxSchemaArtifacts["reference-link.schema.json"],
  authoritySubsetProof: runxSchemaArtifacts["authority-subset-proof.schema.json"],
  signal: runxSchemaArtifacts["signal.schema.json"],
  decision: runxSchemaArtifacts["decision.schema.json"],
  artifact: runxSchemaArtifacts["artifact.schema.json"],
  redaction: runxSchemaArtifacts["redaction.schema.json"],
  ledgerEntry: runxSchemaArtifacts["ledger-entry.schema.json"],
  handoffSignal: runxSchemaArtifacts["handoff-signal.schema.json"],
  handoffState: runxSchemaArtifacts["handoff-state.schema.json"],
  suppressionRecord: runxSchemaArtifacts["suppression-record.schema.json"],
  operationalPolicy: runxSchemaArtifacts["operational-policy.schema.json"],
  operationalProposal: runxSchemaArtifacts["operational-proposal.schema.json"],
  externalJobContinuation: externalJobContinuationV1Schema,
  externalJobSchedule: externalJobScheduleV1Schema,
  externalJobScheduleIntent: externalJobScheduleIntentV1Schema,
  externalJobStageRequest: externalJobStageRequestV1Schema,
  externalJobStageResult: externalJobStageResultV1Schema,
  paidSkillListing: paidSkillListingV1Schema,
  paidInvocation: paidInvocationV1Schema,
  offerRevisionRef: offerRevisionRefV1Schema,
  parentInvocationBinding: parentInvocationBindingV1Schema,
  quotePaidInvocationRequest: quotePaidInvocationRequestV1Schema,
  quotePaidInvocationResult: quotePaidInvocationResultV1Schema,
  executePaidInvocationRequest: executePaidInvocationRequestV1Schema,
  executePaidInvocationResult: executePaidInvocationResultV1Schema,
  getPaidInvocationRequest: getPaidInvocationRequestV1Schema,
  getPaidInvocationResult: getPaidInvocationResultV1Schema,
  cancelPaidInvocationRequest: cancelPaidInvocationRequestV1Schema,
  cancelPaidInvocationResult: cancelPaidInvocationResultV1Schema,
} as const;

export const runxAuxiliarySchemas = {
  registryBinding: runxSchemaArtifacts["registry-binding.schema.json"],
  reviewReceiptOutput: runxSchemaArtifacts["review-receipt-output.schema.json"],
} as const;

export const runxGeneratedSchemaArtifacts = runxSchemaArtifacts;

export { skillOutputV1Schema, projectArtifactRefV1Schema, validateSkillOutputContract, validateProjectArtifactRefContract, type SkillOutputContract, type ProjectArtifactRefContract } from "./schemas/skill-output.js";
