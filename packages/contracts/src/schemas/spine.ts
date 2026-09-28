import {
  type DeepReadonly,
  type JsonSchema,
  type UnknownRecord,
  generatedSchema,
  generatedSchemaAt,
  validateContractSchema,
} from "../internal.js";

type ContractObject = DeepReadonly<Record<string, unknown>>;

function schemaAt<TStatic>(
  schema: JsonSchema,
  path: readonly (string | number)[],
  label: string,
): JsonSchema<TStatic> {
  return generatedSchemaAt<TStatic>(schema, path, label);
}

function enumValues(schema: JsonSchema, label: string): readonly string[] {
  const values = schema.enum;
  if (schema.type !== "string" || !Array.isArray(values)) {
    throw new Error(`generated enum fragment is not a string enum: ${label}`);
  }
  return values.map((value, index) => {
    if (typeof value !== "string") {
      throw new Error(`generated enum fragment has a non-string value: ${label}[${index}]`);
    }
    return value;
  });
}

export type ReferenceTypeContract = string;
export type SignalTypeContract = string;
export type SignalTrustLevelContract = string;
export type ClosureDispositionContract = string;
export type DecisionChoiceContract = string;
export type ActFormContract = string;
export type CriterionStatusContract = string;
export type VerificationStatusContract = string;
export type AuthorityResourceFamilyContract = string;
export type AuthorityVerbContract = string;
export type AuthorityCapabilityContract = string;
export type AuthorityConditionPredicateContract = string;
export type AuthorityEffectCredentialFormContract = string;
export type ProofKindContract = string;

export type ReferenceContract = DeepReadonly<{
  schema?: string;
  type: ReferenceTypeContract;
  uri: string;
  provider?: string;
  locator?: string;
  label?: string;
  observed_at?: string;
  proof_kind?: ProofKindContract;
}>;
export type ReferenceLinkContract = DeepReadonly<{
  role: string;
  ref: ReferenceContract;
}>;
export type ActReferenceContract = DeepReadonly<{
  receipt_ref: ReferenceContract;
  act_id: string;
}>;
export type HashCommitmentContract = ContractObject;
export type RedactionContract = ContractObject;
export type LinksContract = ContractObject;
export type SignalAuthenticityContract = ContractObject;
export type SignalContract = ContractObject;
export type AuthorityEffectLimitContract = ContractObject;
export type AuthorityBoundsContract = ContractObject;
export type AuthorityConditionContract = ContractObject;
export type AuthorityApprovalContract = ContractObject;
export type AuthorityTermContract = ContractObject;
export type AuthoritySubsetProofContract = ContractObject;
export type AuthorityContract = ContractObject;
export type SuccessCriterionContract = DeepReadonly<{
  criterion_id: string;
  statement: string;
  required: boolean;
}>;
export type IntentContract = DeepReadonly<{
  purpose: string;
  legitimacy: string;
  output?: unknown;
  success_criteria: readonly SuccessCriterionContract[];
  constraints: readonly string[];
  derived_from: readonly ReferenceContract[];
}>;
export type VerificationCheckContract = ContractObject;
export type VerificationContract = ContractObject;
export type TargetSurfaceContract = ContractObject;
export type ChangeRequestContract = ContractObject;
export type ChangePlanContract = ContractObject;
export type RevisionDetailsContract = ContractObject;
export type VerificationDetailsContract = ContractObject;
export type CriterionBindingContract = DeepReadonly<{
  criterion_id: string;
  status: CriterionStatusContract;
  evidence_refs: readonly ReferenceContract[];
  verification_refs: readonly ReferenceContract[];
  summary?: string;
}>;
export type ClosureRecordContract = DeepReadonly<{
  disposition: ClosureDispositionContract;
  reason_code: string;
  summary: string;
  closed_at: string;
}>;
export type ActContract = ContractObject & DeepReadonly<{
  id?: string;
  act_id?: string;
  form: ActFormContract;
  intent: IntentContract;
  criterion_bindings: readonly CriterionBindingContract[];
  context_ref?: ReferenceContract;
  artifact_refs: readonly ReferenceContract[];
  revision?: unknown;
  verification?: unknown;
}>;
export type DecisionInputsContract = ContractObject;
export type DecisionJustificationContract = DeepReadonly<{
  summary: string;
  evidence_refs?: readonly ReferenceContract[];
}>;
export type DecisionContract = ContractObject & DeepReadonly<{
  decision_id: string;
  choice: DecisionChoiceContract;
  proposed_intent: IntentContract;
  selected_act_id: string | null;
  justification: DecisionJustificationContract;
}>;
export type ReceiptVerificationSummaryContract = ContractObject;
export type ArtifactContract = ContractObject;
export type ReceiptIssuerContract = DeepReadonly<{
  type: string;
  kid: string;
  public_key_sha256: string;
}>;
export type ReceiptSignatureContract = DeepReadonly<{
  alg: "Ed25519";
  value: string;
}>;
export type FanoutReceiptSyncPointContract = ContractObject;

export const referenceSchema = generatedSchema<ReferenceContract>("reference.schema.json");
export const referenceLinkSchema = generatedSchema<ReferenceLinkContract>("reference-link.schema.json");
export const redactionSchema = generatedSchema<RedactionContract>("redaction.schema.json");
export const signalSchema = generatedSchema<SignalContract>("signal.schema.json");
export const authoritySubsetProofSchema = generatedSchema<AuthoritySubsetProofContract>(
  "authority-subset-proof.schema.json",
);
export const authoritySchema = generatedSchema<AuthorityContract>("authority.schema.json");
export const verificationSchema = generatedSchema<VerificationContract>("verification.schema.json");
export const actSchema = generatedSchema<ActContract>("act.schema.json");
export const decisionSchema = generatedSchema<DecisionContract>("decision.schema.json");
export const artifactSchema = generatedSchema<ArtifactContract>("artifact.schema.json");

const receiptRootSchema = generatedSchema<ContractObject>("receipt.schema.json");

export const referenceTypeSchema = schemaAt<ReferenceTypeContract>(
  referenceSchema,
  ["properties", "type"],
  "reference.type",
);
export const actReferenceSchema = schemaAt<ActReferenceContract>(
  artifactSchema,
  ["properties", "produced_by", "properties", "act_ref"],
  "artifact.produced_by.act_ref",
);
export const proofKindSchema = schemaAt<ProofKindContract>(
  referenceSchema,
  ["properties", "proof_kind"],
  "reference.proof_kind",
);
export const redactionCommitmentAlgorithmSchema = schemaAt<string>(
  redactionSchema,
  ["properties", "hash_commitments", "items", "properties", "algorithm"],
  "redaction.hash_commitments[].algorithm",
);
export const hashCommitmentSchema = schemaAt<HashCommitmentContract>(
  redactionSchema,
  ["properties", "hash_commitments", "items"],
  "redaction.hash_commitments[]",
);
export const signalTypeSchema = schemaAt<SignalTypeContract>(
  signalSchema,
  ["properties", "signal_type"],
  "signal.signal_type",
);
export const signalAuthenticitySchema = schemaAt<SignalAuthenticityContract>(
  signalSchema,
  ["properties", "authenticity"],
  "signal.authenticity",
);
export const signalTrustLevelSchema = schemaAt<SignalTrustLevelContract>(
  signalAuthenticitySchema,
  ["properties", "trust_level"],
  "signal.authenticity.trust_level",
);
export const linksSchema = schemaAt<LinksContract>(
  signalSchema,
  ["properties", "links"],
  "signal.links",
);
export const nullableReferenceSchema = schemaAt<ReferenceContract | null>(
  linksSchema,
  ["properties", "duplicate_of"],
  "links.duplicate_of",
);
export const duplicateCandidateSchema = schemaAt<ContractObject>(
  linksSchema,
  ["properties", "duplicate_candidates", "items"],
  "links.duplicate_candidates[]",
);

export const authorityTermSchema = schemaAt<AuthorityTermContract>(
  authoritySchema,
  ["properties", "terms", "items"],
  "authority.terms[]",
);
export const authorityBoundsSchema = schemaAt<AuthorityBoundsContract>(
  authorityTermSchema,
  ["properties", "bounds"],
  "authority.terms[].bounds",
);
export const authorityEffectLimitSchema = schemaAt<AuthorityEffectLimitContract>(
  authorityBoundsSchema,
  ["properties", "effect_limits", "items"],
  "authority.terms[].bounds.effect_limits[]",
);
export const authorityResourceFamilySchema = schemaAt<AuthorityResourceFamilyContract>(
  authorityTermSchema,
  ["properties", "resource_family"],
  "authority.terms[].resource_family",
);
export const authorityVerbSchema = schemaAt<AuthorityVerbContract>(
  authorityTermSchema,
  ["properties", "verbs", "items"],
  "authority.terms[].verbs[]",
);
export const authorityCapabilitySchema = schemaAt<AuthorityCapabilityContract>(
  authorityTermSchema,
  ["properties", "capabilities", "items"],
  "authority.terms[].capabilities[]",
);
export const authorityConditionSchema = schemaAt<AuthorityConditionContract>(
  authorityTermSchema,
  ["properties", "conditions", "items"],
  "authority.terms[].conditions[]",
);
export const authorityConditionPredicateSchema = schemaAt<AuthorityConditionPredicateContract>(
  authorityConditionSchema,
  ["properties", "predicate"],
  "authority.terms[].conditions[].predicate",
);
export const authorityApprovalSchema = schemaAt<AuthorityApprovalContract>(
  authorityTermSchema,
  ["properties", "approvals", "items"],
  "authority.terms[].approvals[]",
);
export const authoritySubsetComparisonSchema = schemaAt<ContractObject>(
  authoritySubsetProofSchema,
  ["properties", "compared_terms", "items"],
  "authority_subset_proof.compared_terms[]",
);
export const authorityEffectCredentialFormSchema = schemaAt<AuthorityEffectCredentialFormContract>(
  authorityEffectLimitSchema,
  ["properties", "authorization_form"],
  "authority.terms[].bounds.effect_limits[].authorization_form",
);
export const authorityAttenuationSchema = schemaAt<ContractObject>(
  authoritySchema,
  ["properties", "attenuation"],
  "authority.attenuation",
);

export const intentSchema = schemaAt<IntentContract>(actSchema, ["properties", "intent"], "act.intent");
export const successCriterionSchema = schemaAt<SuccessCriterionContract>(
  intentSchema,
  ["properties", "success_criteria", "items"],
  "act.intent.success_criteria[]",
);
export const targetSurfaceSchema = schemaAt<TargetSurfaceContract>(
  actSchema,
  ["properties", "revision", "properties", "change_request", "properties", "target_surfaces", "items"],
  "act.revision.change_request.target_surfaces[]",
);
export const changeRequestSchema = schemaAt<ChangeRequestContract>(
  actSchema,
  ["properties", "revision", "properties", "change_request"],
  "act.revision.change_request",
);
export const changePlanSchema = schemaAt<ChangePlanContract>(
  actSchema,
  ["properties", "revision", "properties", "change_plan"],
  "act.revision.change_plan",
);
export const revisionDetailsSchema = schemaAt<RevisionDetailsContract>(
  actSchema,
  ["properties", "revision"],
  "act.revision",
);
export const verificationDetailsSchema = schemaAt<VerificationDetailsContract>(
  actSchema,
  ["properties", "verification"],
  "act.verification",
);
export const criterionBindingSchema = schemaAt<CriterionBindingContract>(
  actSchema,
  ["properties", "criterion_bindings", "items"],
  "act.criterion_bindings[]",
);
export const criterionStatusSchema = schemaAt<CriterionStatusContract>(
  criterionBindingSchema,
  ["properties", "status"],
  "act.criterion_bindings[].status",
);
export const closureSchema = schemaAt<ClosureRecordContract>(
  actSchema,
  ["properties", "closure"],
  "act.closure",
);
export const closureDispositionSchema = schemaAt<ClosureDispositionContract>(
  closureSchema,
  ["properties", "disposition"],
  "act.closure.disposition",
);
export const actFormSchema = schemaAt<ActFormContract>(
  actSchema,
  ["properties", "form"],
  "act.form",
);
export const verificationCheckSchema = schemaAt<VerificationCheckContract>(
  verificationSchema,
  ["properties", "checks", "items"],
  "verification.checks[]",
);
export const verificationStatusSchema = schemaAt<VerificationStatusContract>(
  verificationSchema,
  ["properties", "status"],
  "verification.status",
);
export const decisionChoiceSchema = schemaAt<DecisionChoiceContract>(
  decisionSchema,
  ["properties", "choice"],
  "decision.choice",
);
export const decisionInputsSchema = schemaAt<DecisionInputsContract>(
  decisionSchema,
  ["properties", "inputs"],
  "decision.inputs",
);
export const decisionJustificationSchema = schemaAt<DecisionJustificationContract>(
  decisionSchema,
  ["properties", "justification"],
  "decision.justification",
);

export const receiptIssuerSchema = schemaAt<ReceiptIssuerContract>(
  receiptRootSchema,
  ["properties", "issuer"],
  "receipt.issuer",
);
export const receiptSignatureSchema = schemaAt<ReceiptSignatureContract>(
  receiptRootSchema,
  ["properties", "signature"],
  "receipt.signature",
);
export const fanoutReceiptSyncPointSchema = schemaAt<FanoutReceiptSyncPointContract>(
  receiptRootSchema,
  ["properties", "lineage", "properties", "sync", "items"],
  "receipt.lineage.sync[]",
);

export const referenceTypes = enumValues(referenceTypeSchema, "reference.type");
/**
 * Canonical signal type identifiers. The wire schema accepts any non-empty
 * string so adapters can publish their own identifier without a schema edit;
 * this list mirrors the Rust `signal_type` canonical module.
 */
export const signalTypes = [
  "issue_opened",
  "issue_comment",
  "pull_request_event",
  "review_event",
  "chat_message",
  "alert",
  "deployment_event",
  "effect_required",
  "schedule_tick",
  "operator_note",
  "system_event",
  "support_ticket",
] as const;
export const signalTrustLevels = enumValues(signalTrustLevelSchema, "signal.authenticity.trust_level");
export const closureDispositions = enumValues(closureDispositionSchema, "act.closure.disposition");
export const decisionChoices = enumValues(decisionChoiceSchema, "decision.choice");
export const actForms = enumValues(actFormSchema, "act.form");
export const criterionStatuses = enumValues(criterionStatusSchema, "act.criterion_bindings[].status");
export const verificationStatuses = enumValues(verificationStatusSchema, "verification.status");
export const authorityResourceFamilies = enumValues(authorityResourceFamilySchema, "authority.terms[].resource_family");
export const authorityVerbs = enumValues(authorityVerbSchema, "authority.terms[].verbs[]");
export const authorityCapabilities = enumValues(authorityCapabilitySchema, "authority.terms[].capabilities[]");
export const authorityConditionPredicates = enumValues(authorityConditionPredicateSchema, "authority.terms[].conditions[].predicate");
export const authorityEffectCredentialForms = enumValues(authorityEffectCredentialFormSchema, "authority.terms[].bounds.effect_limits[].authorization_form");
export const proofKinds = enumValues(proofKindSchema, "reference.proof_kind");
export const redactionCommitmentAlgorithms = enumValues(redactionCommitmentAlgorithmSchema, "redaction.hash_commitments[].algorithm");

export function validateReferenceContract(value: unknown, label = "reference"): ReferenceContract {
  return validateContractSchema(referenceSchema, value, label) as ReferenceContract;
}

export function validateSignalContract(value: unknown, label = "signal"): SignalContract {
  return validateContractSchema(signalSchema, value, label) as SignalContract;
}

export function validateAuthorityContract(value: unknown, label = "authority"): AuthorityContract {
  return validateContractSchema(authoritySchema, value, label) as AuthorityContract;
}

export function validateAuthoritySubsetProofContract(
  value: unknown,
  label = "authority_subset_proof",
): AuthoritySubsetProofContract {
  return validateContractSchema(authoritySubsetProofSchema, value, label) as AuthoritySubsetProofContract;
}

export function validateDecisionContract(value: unknown, label = "decision"): DecisionContract {
  return validateContractSchema(decisionSchema, value, label) as DecisionContract;
}

export function validateActContract(value: unknown, label = "act"): ActContract {
  const act = validateContractSchema(actSchema, value, label) as ActContract;
  assertActFormDetails(act, label);
  return act;
}

export function validateVerificationContract(value: unknown, label = "verification"): VerificationContract {
  return validateContractSchema(verificationSchema, value, label) as VerificationContract;
}

export function validateSpineArtifactContract(value: unknown, label = "artifact"): ArtifactContract {
  return validateContractSchema(artifactSchema, value, label) as ArtifactContract;
}

export function validateRedactionContract(value: unknown, label = "redaction"): RedactionContract {
  return validateContractSchema(redactionSchema, value, label) as RedactionContract;
}

function assertActFormDetails(act: ActContract, label: string): void {
  if (act.form === "revision") {
    if (!act.revision) {
      throw new Error(`${label}.revision is required when form is revision.`);
    }
    if (act.verification) {
      throw new Error(`${label}.verification must be omitted when form is revision.`);
    }
    return;
  }
  if (act.form === "verification") {
    if (!act.verification) {
      throw new Error(`${label}.verification is required when form is verification.`);
    }
    if (act.revision) {
      throw new Error(`${label}.revision must be omitted when form is verification.`);
    }
    return;
  }
  if (act.revision || act.verification) {
    throw new Error(`${label} must not carry revision or verification details when form is ${act.form}.`);
  }
}
