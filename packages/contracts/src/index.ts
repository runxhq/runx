export {
  RUNX_SCOPE_GRANT_POLICY,
  missingGrantedScopes,
  scopeGrantAllows,
  type ScopeGrantPolicy,
} from "./scope-policy.js";

export {
  TemplateError,
  connectorBindingSchema,
  connectorPays,
  validateConnectorBinding,
  connectorImplements,
  hostedHttp,
  isRuntimeCapabilityConnector,
  RUNTIME_CAPABILITY_CONNECTORS,
  isCheckingRead,
  operationAccess,
  operationInputError,
  referenceKind,
  renderDeclaredRequest,
  renderOAuth2TokenEndpoint,
  renderTemplate,
  type ConnectorManifest,
  type PrincipalRequest,
  type ConnectorBinding,
  type ConnectorOperation,
  type ConnectorPlacementName,
  type PaymentCompatibility,
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
  type OperationLease,
  type OperationVerb,
  type ReferenceDeclaration,
  type ReferenceKind,
  type RenderedRequest,
  type Template,
  type TemplateInputs,
} from "./connector.js";

export { posixCommand, renderPosixCommand, type CopyableCommand } from "./copyable-command.js";
export { isRegistryVersion, REGISTRY_VERSION_PATTERN } from "./registry-version.js";
export {
  REGISTRY_MANIFEST_SIGNATURE_ALG,
  REGISTRY_MANIFEST_SIGNATURE_PREFIX,
  REGISTRY_SIGNED_MANIFEST_SCHEMA,
  isConnectorName,
  isRegistrySubjectName,
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
  REGISTRY_TIER_STATEMENT_SCHEMA,
  registryTierStatementPayload,
  validateRegistryTierStatement,
  type RegistryTierStatement,
  type UnsignedRegistryTierStatement,
} from "./registry-tier-statement.js";
export {
  RegistryCategoryError,
  UNCATEGORIZED,
  declaredRegistryCategory,
  registryCategories,
  registryCategory,
  registryTaxonomy,
  type RegistryTaxonomy,
  type RegistryTaxonomyCategory,
  type RegistryTaxonomyMark,
  type RegistryTaxonomySubject,
} from "./registry-taxonomy.js";
export {
  CATALOG_PRESENTATION_PATH,
  CATALOG_PRESENTATION_SCHEMA,
  RegistryPresentationError,
  assertPublicSkillCatalog,
  composedSkillId,
  readCatalogPresentation,
  type PublicSkillCatalogMaterial,
  type RegistryCatalogPresentation,
  type RegistryRelationshipKind,
} from "./catalog-presentation.js";


export {
  RUNX_STABLE_JSON_V1,
  canonicalJsonStringify,
  sha256Hex,
  sha256Prefixed,
} from "./canonical-json.js";

export {
  PaidSkillPriceError,
  type PaidSkillPriceErrorCode,
  paidSkillPriceAmount,
  paidSkillPricedArtifactRef,
  paidSkillPriceProblem,
  paidSkillRequiredLeaseProblem,
} from "./paid-skill-price.js";
export {
  X402_PAYMENT_REQUIRED_HEADER,
  X402_PAYMENT_SIGNATURE_HEADER,
  X402_PAYMENT_RESPONSE_HEADER,
  RUNX_X402_PURCHASE_EXTENSION_KEY,
  RUNX_X402_PURCHASE_SCHEMA_REFERENCE,
  X402_BAZAAR_EXTENSION_KEY,
  X402_JSON_MEDIA_TYPE,
  MAX_X402_HEADER_BYTES,
  MAX_X402_DECODED_BYTES,
  X402PresentationError,
  assembleExternalX402PaymentRequired,
  assembleX402PaymentRequired,
  parseRunxX402PurchaseDeclaration,
  declareExternalX402JsonPostDiscovery,
  x402ExternalDiscoveryHttpProjection,
  x402DiscoveryHttpProjection,
  bindX402PaymentRequiredChallenge,
  x402PaymentRequiredFromChallenge,
  validateX402PaymentRetry,
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

// A host's own schema may refer to the types runx publishes once; this
// validates it against runx's whole published set.
export {
  type JsonSchema,
  type Static,
  validateContractSchema,
} from "./internal.js";

// Every contract type, generated from the Rust type that owns its schema.
export type * from "./generated/contract-types.js";

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
} from "./schemas/skill-authoring.js";

export {
  storeResultV1Schema,
  validateStoreResultContract,
} from "./schemas/store-result.js";

export { outputSchema, validateOutputContract } from "./schemas/output.js";

export {
  agentContextEnvelopeSchema,
  validateAgentContextEnvelopeContract,
} from "./schemas/context.js";

export {
  turnInvocationSchema,
  questionSchema,
  approvalGateSchema,
  validateTurnInvocationContract,
  validateQuestionContract,
  validateApprovalGateContract,
} from "./schemas/agent-act.js";

export { skillInspectionSchema, validateSkillInspectionContract } from "./schemas/skill-inspection.js";

export {
  turnCallSchema,
  turnLoadSchema,
  turnStatusSchema,
  turnReadSchema,
  validateTurnCallContract,
  validateTurnLoadContract,
  validateTurnStatusContract,
  validateTurnReadContract,
} from "./schemas/turn.js";

export {
  processSchema,
  processOutputSchema,
  processListSchema,
  validateProcessContract,
  validateProcessOutputContract,
  validateProcessListContract,
} from "./schemas/process.js";

export {
  runReadSchema,
  runEventSchema,
  validateRunReadContract,
  validateRunEventContract,
} from "./schemas/run-read.js";

export { resumeDocumentSchema, validateResumeDocumentContract } from "./schemas/resume.js";

export {
  resolutionRequestSchema,
  resolutionResponseSchema,
  validateResolutionRequestContract,
  validateResolutionResponseContract,
} from "./schemas/resolution.js";

export {
  doctorRepairSchema,
  doctorLocationSchema,
  doctorDiagnosticSchema,
  doctorSummarySchema,
  doctorV1Schema,
  validateDoctorReportContract,
} from "./schemas/doctor.js";

export {
  runxListRequestedKindSchema,
  runxListItemKindSchema,
  runxListSourceSchema,
  runxListItemSchema,
  listV1Schema,
  validateRunxListReportContract,
} from "./schemas/list.js";

export {
  receiptV1Schema,
  receiptSubjectSchema,
  receiptEnforcementSchema,
  receiptAuthoritySchema,
  receiptCriterionSchema,
  receiptActSchema,
  receiptSealSchema,
  receiptLineageSchema,
  validateReceiptContract,
  RECEIPT_CANONICALIZATION,
  RECEIPT_TREE_LIMITS,
} from "./schemas/receipt.js";

export {
  paidSkillListingV1Schema,
  purchaseV1Schema,
  offerVersionBindingV1Schema,
  quotePurchaseResultV1Schema,
  quotePurchaseMediaType,
  purchaseReadV1Schema,
  paymentIdempotencyBindingSchema,
  purchasePaymentChallengeSchema,
  purchasePaymentOptionBindingSchema,
  validateRunxPrincipalId,
  principalReferenceFromRunxPrincipalId,
  validatePaidSkillListingContract,
  validatePurchaseContract,
  validateOfferVersionBindingContract,
  validatePaymentIdempotencyBindingContract,
  validatePurchasePaymentChallengeContract,
  validatePurchasePaymentOptionBindingContract,
  validateQuotePurchaseResultContract,
  validatePurchaseReadContract,
  selectPurchasePaymentOption,
  RUNX_BALANCE_SETTLEMENT_FAMILY,
  type RunxPrincipalId,
} from "./schemas/purchase.js";

export {
  X402_SCHEMA_IDS,
  x402ResourceInfoV2Schema,
  x402PaymentRequirementsV2Schema,
  x402PaymentRequiredV2Schema,
  x402PaymentPayloadV2Schema,
  x402SettleResponseV2Schema,
  runxX402PurchaseExtensionInfoV1Schema,
  validateX402ResourceInfoContract,
  validateX402PaymentRequirementsContract,
  validateX402PaymentRequiredContract,
  validateX402PaymentPayloadContract,
  validateX402SettleResponseContract,
  validateRunxX402PurchaseExtensionInfoContract,
  type RunxX402DiscoveryExtensionInfoContract,
  type RunxX402PurchaseExtensionContract,
} from "./schemas/x402.js";

export {
  operationalPolicySchema,
  validateOperationalPolicyContract,
} from "./schemas/operational-policy.js";

export { toolManifestV1Schema } from "./schemas/tool-manifest.js";

export {
  referenceTypeSchema,
  runOutcomeSchema,
  decisionChoiceSchema,
  actFormSchema,
  criterionStatusSchema,
  criterionSchema,
  authorityVerbSchema,
  proofKindSchema,
  referenceSchema,
  actAddressSchema,
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
  validateAuthoritySubsetProofContract,
  validateDecisionContract,
  validateSpineArtifactContract,
} from "./schemas/spine.js";

export {
  skillOutputV1Schema,
  validateSkillOutputContract,
} from "./schemas/skill-output.js";

import {
  paidSkillListingV1Schema,
  purchaseV1Schema,
  offerVersionBindingV1Schema,
  quotePurchaseResultV1Schema,
  purchaseReadV1Schema,
} from "./schemas/purchase.js";
import { storeResultV1Schema } from "./schemas/store-result.js";
import { runxSchemaArtifacts } from "./schema-artifacts.js";

export const runxContractSchemas = {
  output: runxSchemaArtifacts["output.schema.json"],
  agentContextEnvelope: runxSchemaArtifacts["agent-context-envelope.schema.json"],
  turnInvocation: runxSchemaArtifacts["agent-act-invocation.schema.json"],
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
  storeResult: storeResultV1Schema,
  doctor: runxSchemaArtifacts["doctor.schema.json"],
  list: runxSchemaArtifacts["list.schema.json"],
  skillArchitectureDecision: runxSchemaArtifacts["skill-architecture-decision.schema.json"],
  skillArchitecturePlan: runxSchemaArtifacts["skill-architecture-plan.schema.json"],
  skillChangeDraft: runxSchemaArtifacts["skill-change-draft.schema.json"],
  skillChangeBundle: runxSchemaArtifacts["skill-change-bundle.schema.json"],
  skillValidationResult: runxSchemaArtifacts["skill-validation-result.schema.json"],
  skillApplyResult: runxSchemaArtifacts["skill-apply-result.schema.json"],
  receipt: runxSchemaArtifacts["receipt.schema.json"],
  toolManifest: runxSchemaArtifacts["tool-manifest.schema.json"],
  reference: runxSchemaArtifacts["reference.schema.json"],
  authoritySubsetProof: runxSchemaArtifacts["authority-subset-proof.schema.json"],
  decision: runxSchemaArtifacts["decision.schema.json"],
  artifact: runxSchemaArtifacts["artifact.schema.json"],
  operationalPolicy: runxSchemaArtifacts["operational-policy.schema.json"],
  paidSkillListing: paidSkillListingV1Schema,
  purchase: purchaseV1Schema,
  offerVersionBinding: offerVersionBindingV1Schema,
  quotePurchaseResult: quotePurchaseResultV1Schema,
  purchaseRead: purchaseReadV1Schema,
} as const;

export const runxGeneratedSchemaArtifacts = runxSchemaArtifacts;
