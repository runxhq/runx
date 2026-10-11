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
/** A check the runtime runs and binds to a receipt: its own closed vocabulary. */
export type CriterionContract = string;
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
export type IntentContract = DeepReadonly<{
  purpose: string;
  legitimacy: string;
}>;
export type CriterionBindingContract = DeepReadonly<{
  criterion_id: CriterionContract;
  status: CriterionStatusContract;
  evidence_refs: readonly ReferenceContract[];
  verification_refs: readonly ReferenceContract[];
}>;
/** A decision states its choice and the act it selects; the intent, target
 * and artifacts are that act's. */
export type DecisionContract = DeepReadonly<{
  schema?: string;
  decision_id: string;
  choice: DecisionChoiceContract;
  selected_act_id: string;
}>;
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

export const referenceSchema = generatedSchema<ReferenceContract>("reference.schema.json");
export const referenceLinkSchema = generatedSchema<ReferenceLinkContract>("reference-link.schema.json");
export const redactionSchema = generatedSchema<RedactionContract>("redaction.schema.json");
export const signalSchema = generatedSchema<SignalContract>("signal.schema.json");
export const authoritySubsetProofSchema = generatedSchema<AuthoritySubsetProofContract>(
  "authority-subset-proof.schema.json",
);
export const authorityTermSchema = generatedSchema<AuthorityTermContract>("authority-term.schema.json");
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
const receiptActSchema = schemaAt<ContractObject>(
  receiptRootSchema,
  ["properties", "acts", "items"],
  "receipt.acts[]",
);
export const intentSchema = schemaAt<IntentContract>(
  receiptActSchema,
  ["properties", "intent"],
  "receipt.acts[].intent",
);
export const criterionBindingSchema = schemaAt<CriterionBindingContract>(
  receiptActSchema,
  ["properties", "criterion_bindings", "items"],
  "receipt.acts[].criterion_bindings[]",
);
export const criterionSchema = schemaAt<CriterionContract>(
  criterionBindingSchema,
  ["properties", "criterion_id"],
  "receipt.acts[].criterion_bindings[].criterion_id",
);
export const criterionStatusSchema = schemaAt<CriterionStatusContract>(
  criterionBindingSchema,
  ["properties", "status"],
  "receipt.acts[].criterion_bindings[].status",
);
export const closureDispositionSchema = schemaAt<ClosureDispositionContract>(
  receiptRootSchema,
  ["properties", "seal", "properties", "disposition"],
  "receipt.seal.disposition",
);
export const actFormSchema = schemaAt<ActFormContract>(
  receiptActSchema,
  ["properties", "form"],
  "receipt.acts[].form",
);
export const decisionChoiceSchema = schemaAt<DecisionChoiceContract>(
  decisionSchema,
  ["properties", "choice"],
  "decision.choice",
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
export const closureDispositions = enumValues(closureDispositionSchema, "receipt.seal.disposition");
export const decisionChoices = enumValues(decisionChoiceSchema, "decision.choice");
export const actForms = enumValues(actFormSchema, "receipt.acts[].form");
export const criterionStatuses = enumValues(criterionStatusSchema, "receipt.acts[].criterion_bindings[].status");
export const criteria = enumValues(criterionSchema, "receipt.acts[].criterion_bindings[].criterion_id");
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

export function validateAuthoritySubsetProofContract(
  value: unknown,
  label = "authority_subset_proof",
): AuthoritySubsetProofContract {
  return validateContractSchema(authoritySubsetProofSchema, value, label) as AuthoritySubsetProofContract;
}

export function validateDecisionContract(value: unknown, label = "decision"): DecisionContract {
  return validateContractSchema(decisionSchema, value, label) as DecisionContract;
}

export function validateSpineArtifactContract(value: unknown, label = "artifact"): ArtifactContract {
  return validateContractSchema(artifactSchema, value, label) as ArtifactContract;
}

export function validateRedactionContract(value: unknown, label = "redaction"): RedactionContract {
  return validateContractSchema(redactionSchema, value, label) as RedactionContract;
}
